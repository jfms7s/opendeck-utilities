//! The brightness schedule: up to two local times of day, each with a
//! brightness to set then. Plugin-wide (OpenDeck's global settings), so it
//! runs whatever profile is shown. Pure apart from `SystemClock`, the one
//! place that asks the OS for local time.

use crate::brightness::PERCENT;
use serde_json::{Value, json};

/// Minutes in a day: a slot's time is a minute past local midnight.
const DAY_MINUTES: u16 = 24 * 60;
/// The key under OpenDeck's global settings object.
pub const GLOBAL_KEY: &str = "brightness_schedule";

/// One scheduled change. `time: None` means the slot is off.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Slot {
    pub time: Option<u16>,
    pub value: u8,
}

/// Two slots, kept in place (an empty first slot stays first) so the page
/// shows them where the user put them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Schedule {
    pub slots: [Slot; 2],
}

impl Default for Schedule {
    /// Both off; the values are what the page offers once a time is set
    /// (dim at night, brighter in the morning).
    fn default() -> Self {
        Self {
            slots: [
                Slot {
                    time: None,
                    value: 10,
                },
                Slot {
                    time: None,
                    value: 70,
                },
            ],
        }
    }
}

/// `"HH:MM"` (24 h) as minutes past midnight; anything else is `None`.
pub fn parse_time(text: &str) -> Option<u16> {
    let (h, m) = text.trim().split_once(':')?;
    if h.is_empty() || h.len() > 2 || m.len() != 2 {
        return None;
    }
    let (h, m): (u16, u16) = (h.parse().ok()?, m.parse().ok()?);
    (h < 24 && m < 60).then_some(h * 60 + m)
}

pub fn format_time(minute: u16) -> String {
    format!("{:02}:{:02}", minute / 60, minute % 60)
}

impl Schedule {
    /// Read leniently (`src/lenient.rs` rules): a slot with a missing or
    /// unparseable time is off, a value is rounded and clamped to 0-100 and
    /// keeps its default when it isn't a number; a non-array is the default.
    pub fn from_value(value: &Value) -> Self {
        let mut s = Self::default();
        let Some(items) = value.as_array() else {
            return s;
        };
        for (slot, item) in s.slots.iter_mut().zip(items) {
            slot.time = item["time"].as_str().and_then(parse_time);
            if let Some(n) = item["value"].as_f64().filter(|n| n.is_finite()) {
                let (lo, hi) = (*PERCENT.start(), *PERCENT.end());
                slot.value = n.round().clamp(lo.into(), hi.into()) as u8;
            }
        }
        s
    }

    /// The schedule in OpenDeck's global settings (`None`: none stored).
    pub fn from_global(global: &Value) -> Self {
        Self::from_value(&global[GLOBAL_KEY])
    }

    /// The page and storage form: `[{ "time": "HH:MM" | "", "value": n }; 2]`.
    pub fn to_value(self) -> Value {
        Value::Array(
            self.slots
                .iter()
                .map(|s| json!({ "time": s.time.map(format_time).unwrap_or_default(), "value": s.value }))
                .collect(),
        )
    }

    fn active(&self) -> impl Iterator<Item = (u16, u8)> + '_ {
        self.slots
            .iter()
            .filter_map(|s| s.time.map(|t| (t.min(DAY_MINUTES - 1), s.value)))
    }

    /// The next change strictly after `after` (seconds since the epoch):
    /// when, and the value. `at(t, days, minute)` is the instant of local
    /// `minute` on the local date of `t` plus `days` (the clock's job, so
    /// DST is the OS's). Each slot fires once per local date, so a time a
    /// DST fall-back repeats fires once. Two slots due at once: the second.
    pub fn next_after(&self, after: i64, at: impl Fn(i64, i32, u16) -> i64) -> Option<(i64, u8)> {
        self.active()
            .map(|(minute, value)| {
                let today = at(after, 0, minute);
                let when = if today > after {
                    today
                } else {
                    at(after, 1, minute)
                };
                (when, value)
            })
            .reduce(|a, b| if b.0 <= a.0 { b } else { a })
    }

    /// The value of the period `now` is in: the slot whose last occurrence
    /// at or before `now` is the latest. Used once at start-up, so a plugin
    /// started at 01:00 applies the 23:00 value.
    pub fn value_at(&self, now: i64, at: impl Fn(i64, i32, u16) -> i64) -> Option<u8> {
        self.active()
            .map(|(minute, value)| {
                let today = at(now, 0, minute);
                let when = if today <= now {
                    today
                } else {
                    at(now, -1, minute)
                };
                (when, value)
            })
            .reduce(|a, b| if b.0 >= a.0 { b } else { a })
            .map(|(_, value)| value)
    }
}

/// Local wall-clock time, behind a trait so tests use a fixed zone.
pub trait LocalClock: Send + Sync {
    /// Seconds since the epoch.
    fn now(&self) -> i64;
    /// The instant of local `minute` past midnight on the local date of `t`
    /// plus `days`.
    fn at(&self, t: i64, days: i32, minute: u16) -> i64;
}

unsafe extern "C" {
    /// POSIX; the `libc` crate doesn't bind it. Re-reads TZ / the zone file.
    fn tzset();
}

/// The OS's local time zone (`localtime_r` / `mktime`, DST-aware). A time
/// a spring-forward skips is moved on by the OS (02:30 becomes 03:30).
pub struct SystemClock;

impl LocalClock for SystemClock {
    fn now(&self) -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs() as i64)
    }

    /// Deterministic across DST: of the readings of that local time (one
    /// normally, two when a fall-back repeats it), the earliest; when a
    /// spring-forward skips it, what `mktime` normalises it to (later).
    /// `mktime` with `tm_isdst = -1` alone is not deterministic for a
    /// repeated time on glibc (it depends on the previous call).
    fn at(&self, t: i64, days: i32, minute: u16) -> i64 {
        let fallback = t + i64::from(days) * 86_400;
        let (hour, min) = (i32::from(minute / 60), i32::from(minute % 60));
        let Some(mut day) = local_tm(t) else {
            return fallback;
        };
        day.tm_mday += days;
        day.tm_hour = hour;
        day.tm_min = min;
        day.tm_sec = 0;
        let reading = |isdst: i32| {
            let mut tm = day;
            tm.tm_isdst = isdst;
            // SAFETY: `tm` is a valid local struct; mktime normalises it.
            let out = unsafe { libc::mktime(&mut tm) };
            (out != -1).then_some(out)
        };
        let lands_on_time = |out: libc::time_t| {
            local_tm(out).is_some_and(|tm| tm.tm_hour == hour && tm.tm_min == min)
        };
        [0, 1]
            .into_iter()
            .filter_map(reading)
            .filter(|&out| lands_on_time(out))
            .min()
            .or_else(|| reading(-1))
            .unwrap_or(fallback)
    }
}

/// The local calendar fields of `t`. 64-bit `time_t` on every supported
/// target (Linux x86_64, macOS arm64).
fn local_tm(t: i64) -> Option<libc::tm> {
    let time: libc::time_t = t;
    // SAFETY: `tm` is a plain C struct that `localtime_r` fills in (all-zero
    // is a valid starting value); `tzset` re-reads TZ as `localtime` would.
    unsafe {
        tzset();
        let mut tm: libc::tm = std::mem::zeroed();
        (!libc::localtime_r(&time, &mut tm).is_null()).then_some(tm)
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::LocalClock;

    /// A zone with one DST change: local = UTC + `before` seconds until
    /// `switch` (UTC seconds), UTC + `after` from then on. `now` is set by
    /// the test (e.g. from tokio's paused clock).
    pub struct FakeZone {
        pub before: i64,
        pub after: i64,
        pub switch: i64,
        pub now: Box<dyn Fn() -> i64 + Send + Sync>,
    }

    impl FakeZone {
        pub fn fixed(now: impl Fn() -> i64 + Send + Sync + 'static) -> Self {
            Self {
                before: 0,
                after: 0,
                switch: i64::MAX,
                now: Box::new(now),
            }
        }

        fn offset_at_utc(&self, utc: i64) -> i64 {
            if utc < self.switch {
                self.before
            } else {
                self.after
            }
        }
    }

    impl LocalClock for FakeZone {
        fn now(&self) -> i64 {
            (self.now)()
        }

        fn at(&self, t: i64, days: i32, minute: u16) -> i64 {
            let local = t + self.offset_at_utc(t);
            let day = local.div_euclid(86_400) + i64::from(days);
            let wall = day * 86_400 + i64::from(minute) * 60;
            // Try the offset before the change first, like mktime with
            // tm_isdst = -1 picks one reading of an ambiguous time.
            let guess = wall - self.before;
            if self.offset_at_utc(guess) == self.before {
                guess
            } else {
                wall - self.after
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::FakeZone;
    use super::*;

    const H: i64 = 3600;

    fn sched(a: Option<&str>, av: u8, b: Option<&str>, bv: u8) -> Schedule {
        Schedule {
            slots: [
                Slot {
                    time: a.and_then(parse_time),
                    value: av,
                },
                Slot {
                    time: b.and_then(parse_time),
                    value: bv,
                },
            ],
        }
    }

    fn utc() -> FakeZone {
        FakeZone::fixed(|| 0)
    }

    #[test]
    fn times_parse_strictly() {
        assert_eq!(parse_time("23:00"), Some(23 * 60));
        assert_eq!(parse_time("8:05"), Some(8 * 60 + 5));
        assert_eq!(parse_time("00:00"), Some(0));
        for bad in [
            "24:00", "12:60", "", "12", "12:5", "ab:cd", "-1:00", "12:00:00",
        ] {
            assert_eq!(parse_time(bad), None, "{bad}");
        }
        assert_eq!(format_time(8 * 60 + 5), "08:05");
    }

    #[test]
    fn stored_schedules_are_read_leniently_and_round_trip() {
        assert_eq!(Schedule::from_value(&Value::Null), Schedule::default());
        assert_eq!(Schedule::from_global(&json!({})), Schedule::default());
        let s = Schedule::from_value(&json!([
            { "time": "23:00", "value": 300 },
            { "time": "nope", "value": "x" }
        ]));
        assert_eq!(s, sched(Some("23:00"), 100, None, 70));
        let stored = json!([{ "time": "23:00", "value": 10 }, { "time": "", "value": 70 }]);
        assert_eq!(Schedule::from_value(&stored).to_value(), stored);
    }

    #[test]
    fn both_times_set_alternate() {
        let z = utc();
        let s = sched(Some("23:00"), 10, Some("08:00"), 70);
        let at = |t, d, m| z.at(t, d, m);
        // Day 0, 12:00 → 23:00 the same day.
        assert_eq!(s.next_after(12 * H, at), Some((23 * H, 10)));
        // Right at 23:00 → the next 08:00.
        assert_eq!(s.next_after(23 * H, at), Some((32 * H, 70)));
        assert_eq!(s.value_at(12 * H, at), Some(70));
        assert_eq!(s.value_at(23 * H, at), Some(10));
    }

    #[test]
    fn one_time_set_fires_daily() {
        let z = utc();
        let s = sched(None, 10, Some("08:00"), 70);
        let at = |t, d, m| z.at(t, d, m);
        assert_eq!(s.next_after(9 * H, at), Some((32 * H, 70)));
        assert_eq!(s.value_at(9 * H, at), Some(70));
        assert_eq!(s.value_at(7 * H, at), Some(70), "yesterday's 08:00");
        assert_eq!(Schedule::default().next_after(0, at), None);
        assert_eq!(Schedule::default().value_at(0, at), None);
    }

    /// Started at 01:00, between 23:00 and 08:00: the night value applies.
    #[test]
    fn an_overnight_window_catches_up() {
        let z = utc();
        let s = sched(Some("23:00"), 10, Some("08:00"), 70);
        let at = |t, d, m| z.at(t, d, m);
        let one_am = 24 * H + H;
        assert_eq!(s.value_at(one_am, at), Some(10));
        assert_eq!(s.next_after(one_am, at), Some((32 * H, 70)));
    }

    /// A 23-hour day: clocks go 01:00 → 02:00 local at 01:00 UTC (UTC+0 →
    /// UTC+1). From 22:00 the evening before, 08:00 is 9 real hours away.
    #[test]
    fn a_spring_forward_day_is_23_hours() {
        let z = FakeZone {
            before: 0,
            after: H,
            switch: 24 * H + H,
            now: Box::new(|| 0),
        };
        let s = sched(Some("23:00"), 10, Some("08:00"), 70);
        let at = |t, d, m| z.at(t, d, m);
        let ten_pm = 22 * H;
        assert_eq!(s.next_after(ten_pm, at), Some((23 * H, 10)));
        let (morning, v) = s.next_after(23 * H, at).unwrap();
        assert_eq!(v, 70);
        assert_eq!(morning - ten_pm, 9 * H);
        // The next 23:00 is 23 h after the last one, not 24.
        let (night, _) = s.next_after(morning, at).unwrap();
        assert_eq!(night - 23 * H, 23 * H);
    }

    /// A 25-hour day: UTC+1 → UTC+0 at 01:00 UTC (02:00 → 01:00 local), so
    /// 01:30 happens twice. A 01:30 slot fires once that day.
    #[test]
    fn a_fall_back_day_is_25_hours_and_fires_once() {
        let z = FakeZone {
            before: H,
            after: 0,
            switch: 24 * H + H,
            now: Box::new(|| 0),
        };
        let s = sched(Some("01:30"), 10, Some("08:00"), 70);
        let at = |t, d, m| z.at(t, d, m);
        // 23:00 local on day 0 is 22:00 UTC.
        let evening = 22 * H;
        let (first, v) = s.next_after(evening, at).unwrap();
        assert_eq!((first, v), (24 * H + H / 2, 10), "the first 01:30 (UTC+1)");
        // After it fires: 08:00, not the repeated 01:30 an hour later.
        let (next, v) = s.next_after(first, at).unwrap();
        assert_eq!(v, 70);
        assert_eq!(next, 24 * H + 8 * H);
        // 08:00 to the next day's 08:00 across nothing: 24 h; the night of
        // the change, 23:00 → 08:00 is 10 real hours.
        let night = sched(Some("23:00"), 10, Some("08:00"), 70);
        let (morning, _) = night.next_after(evening, at).unwrap();
        assert_eq!(morning - evening, 10 * H);
    }

    /// Run in a child process with TZ set (see the test below), so no test
    /// changes this process's environment.
    #[test]
    #[ignore = "run by the_system_clock_handles_a_repeated_hour with TZ set"]
    fn system_clock_fall_back_probe() {
        let c = SystemClock;
        // 2026-10-31 12:00 EDT; 01:30 on 2026-11-01 happens twice (EDT, EST).
        let noon = 1_793_462_400;
        let first = 1_793_511_000; // 05:30 UTC = 01:30 EDT
        for _ in 0..3 {
            assert_eq!(c.at(noon, 1, 90), first);
            assert_eq!(c.at(first, 0, 90), first, "the same reading from itself");
            assert_eq!(c.at(first + 3600, 0, 90), first, "and from the repeat");
        }
        let s = Schedule {
            slots: [
                Slot {
                    time: Some(90),
                    value: 10,
                },
                Slot {
                    time: None,
                    value: 70,
                },
            ],
        };
        let at = |t, d, m| c.at(t, d, m);
        assert_eq!(s.next_after(noon, at), Some((first, 10)));
        let (next, _) = s.next_after(first, at).unwrap();
        assert!(
            next > first + 20 * 3600,
            "fires once that date, not at the repeat"
        );
    }

    /// Adversarial review: the repeated 01:30 of a fall-back night must be
    /// read the same way every time (America/New_York, 2026-11-01).
    #[test]
    fn the_system_clock_handles_a_repeated_hour() {
        if !std::path::Path::new("/usr/share/zoneinfo/America/New_York").exists() {
            println!("no tzdata: skipped");
            return;
        }
        let out = std::process::Command::new(std::env::current_exe().unwrap())
            .env("TZ", "America/New_York")
            .args([
                "--ignored",
                "--exact",
                "schedule::tests::system_clock_fall_back_probe",
            ])
            .output()
            .unwrap();
        let log = String::from_utf8_lossy(&out.stdout);
        assert!(out.status.success() && log.contains("1 passed"), "{log}");
    }

    #[test]
    fn the_system_clock_lands_on_the_asked_local_time() {
        let c = SystemClock;
        let now = c.now();
        for minute in [0, 61, 23 * 60 + 59] {
            let t = c.at(now, 1, minute);
            assert!(t > now, "{minute}");
            let time: libc::time_t = t;
            // SAFETY: as in `SystemClock::at`.
            let tm = unsafe {
                let mut tm: libc::tm = std::mem::zeroed();
                libc::localtime_r(&time, &mut tm);
                tm
            };
            assert_eq!(
                (tm.tm_hour * 60 + tm.tm_min) as u16,
                minute,
                "{minute} (unless a DST gap swallowed it)"
            );
        }
    }
}
