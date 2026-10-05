use super::model::Device;
use crate::cycle::subset_or_all;

/// Applies a volume delta, clamped at 0. Going up never exceeds
/// `max` - but also never *lowers* a level that is already above `max`
/// (set elsewhere), which would feel like the dial going the wrong way.
pub fn adjust_volume(current: u16, delta: i32, max: u16) -> u16 {
    let target = i32::from(current) + delta;
    let clamped = if delta > 0 {
        target.min(i32::from(max.max(current)))
    } else {
        target.max(0)
    };
    u16::try_from(clamped).unwrap_or(0)
}

/// Device names to cycle through, ordered by description (case-insensitive,
/// then by name). A non-empty `subset` limits it to those names that exist;
/// if none of them exist any more, all devices are used.
pub fn cycle_candidates(devices: &[Device], subset: &[String]) -> Vec<String> {
    let names: Vec<String> = devices.iter().map(|d| d.name.clone()).collect();
    let names = subset_or_all(&names, subset);
    let mut chosen: Vec<&Device> = devices.iter().filter(|d| names.contains(&d.name)).collect();
    chosen.sort_by(|a, b| {
        a.description
            .to_lowercase()
            .cmp(&b.description.to_lowercase())
            .then_with(|| a.name.cmp(&b.name))
    });
    chosen.into_iter().map(|d| d.name.clone()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::model::test_support::fixture_snapshot;

    #[test]
    fn steps_and_clamps() {
        assert_eq!(adjust_volume(50, 5, 100), 55);
        assert_eq!(adjust_volume(98, 5, 100), 100);
        assert_eq!(adjust_volume(3, -5, 100), 0);
        assert_eq!(adjust_volume(140, 15, 150), 150);
    }

    #[test]
    fn volume_up_never_lowers_an_over_max_level() {
        assert_eq!(adjust_volume(130, 5, 100), 130);
        assert_eq!(adjust_volume(130, -5, 100), 125);
    }

    #[test]
    fn candidates_are_sorted_by_description() {
        let s = fixture_snapshot();
        assert_eq!(
            cycle_candidates(&s.sinks, &[]),
            vec![
                "alsa_output.usb-Razer_Razer_Leviathan_V2-00.analog-stereo".to_string(),
                "alsa_output.pci-0000_2f_00.4.iec958-stereo".to_string(),
            ]
        );
    }

    #[test]
    fn subset_limits_and_ignores_missing_names() {
        let s = fixture_snapshot();
        let subset = vec![
            "gone".to_string(),
            "alsa_output.pci-0000_2f_00.4.iec958-stereo".to_string(),
        ];
        assert_eq!(cycle_candidates(&s.sinks, &subset), vec![subset[1].clone()]);
    }

    #[test]
    fn stale_subset_falls_back_to_all_devices() {
        let s = fixture_snapshot();
        let subset = vec!["gone".to_string()];
        assert_eq!(
            cycle_candidates(&s.sinks, &subset),
            cycle_candidates(&s.sinks, &[])
        );
    }
}
