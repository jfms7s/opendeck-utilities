# OpenDeck Utilities — Design

Date: 2026-09-26
Status: approved in conversation, pending written-spec review

## 1. Purpose

Recreate three utilities that already exist for OpenDeck — sound control
(`com.sfgrimes.pipewire-audio`), Device Brightness and Switch Profile
(`com.amansprojects.starterpack`) — as a new plugin that can be customized
the way the existing ones cannot.

What the user said is missing from the existing utilities:

- **Visuals / feedback** — live level/state on the key and touch strip,
  custom label and colour, drawn like the other `jfms7s` plugins.
- **Targeting** — pick a specific output, input or app stream, match by
  name, or follow the system default.
- **Multi-action per control** — one control doing several things (e.g.
  rotate = volume, press = mute, long-press = switch device).

Success: on the user's Stream Deck XL+ (6 dials, 1200x100 touch strip,
32 keys) the Audio action can replace every `pipewire-audio` action they use,
with live feedback and per-gesture configuration; Brightness and Switch
Profile exist with the same UX, subject to the host limitation in §2.

## 2. Known host limitation (accepted)

OpenDeck only honours the `switchProfile` and `deviceBrightness` plugin
events when the sender's plugin UUID is `com.amansprojects.starterpack.sdPlugin`
(or its internal Elgato shim) — see OpenDeck
`src-tauri/src/events/inbound/mod.rs` (the `matches!(decoded,
InboundEventType::SwitchProfile(_) | InboundEventType::DeviceBrightness(_))`
guard). Events from any other plugin are silently dropped.

Decision (user): build Brightness and Switch Profile anyway. On stock
OpenDeck they will not take effect; the plugin detects this (§6.3) and says
so visibly, and the README documents the limitation. No OpenDeck fork or
upstream PR is part of this project.

## 3. Project

| Item | Value |
|---|---|
| Location | `~/git/opendeck-utilities` (standalone repo) |
| Crate | `opendeck-utilities`, Rust edition 2024 |
| Plugin UUID | `com.jfms7s.utilities` |
| Category | `Utilities` |
| Licence | MIT |

Conventions copied from `opendeck-power-profile`: `build.mjs` (assembles
`dist/<uuid>.sdPlugin/`, fails if `Cargo.toml` and `assets/manifest.json`
versions differ), `rustfmt.toml`, `.github/workflows/{ci,release}.yml`,
`assets/{manifest.json,icons/,layouts/}`, per-target binaries for
`x86_64-unknown-linux-gnu` and `aarch64-unknown-linux-gnu`.

Dependencies: `openaction` 2.7, `tokio` (rt-multi-thread, macros, process,
time, sync), `serde`/`serde_json`, `dashmap`, `async-trait`, `thiserror` 2,
`log`, `simplelog`, `base64`. No libpulse/C bindings.

## 4. Actions

| Action | UUID | Controllers |
|---|---|---|
| Audio | `com.jfms7s.utilities.audio` | Keypad, Encoder |
| Device Brightness | `com.jfms7s.utilities.brightness` | Keypad, Encoder |
| Switch Profile | `com.jfms7s.utilities.profile` | Keypad, Encoder |

## 5. Architecture

```
src/
  main.rs                 register actions, logging, start shared tasks
  host.rs                 send switchProfile / deviceBrightness via send_arbitrary_json
  opendeck_state.rs       read-only view of OpenDeck's config files (§6.1)
  audio/
    backend.rs            AudioBackend trait + PactlBackend (only code that runs pactl)
    model.rs              Snapshot, Device, Stream, Volume types; pactl JSON parsing
    target.rs             Target enum + resolve(target, &Snapshot) -> Resolved   (pure)
    gesture.rs            Gesture + Operation + settings -> Operation            (pure)
  render/
    strip.rs              touch-strip feedback payloads + layout
    tile.rs               key icons as SVG data URIs
  actions/
    audio.rs
    brightness.rs
    profile.rs
  pi/                     (assets) property inspector HTML/JS per action
```

Each unit has one job:

- `audio/backend.rs` — the only code that spawns `pactl`. Exposes
  `snapshot()`, `set_volume(obj, pct)`, `set_mute(obj, bool|toggle)`,
  `set_default(kind, name)`, `move_stream(id, sink)`. Owns one shared
  `pactl subscribe` child, publishing a `tokio::sync::watch` "changed"
  tick; if the child exits it restarts with backoff 1, 2, 4… s (cap 30 s).
  Hidden behind the `AudioBackend` trait so action logic can be tested
  against a fake.
- `audio/target.rs`, `audio/gesture.rs` — pure functions, no I/O.
- `render/*` — pure functions from state to payload/SVG string.
- `actions/*` — thin `openaction` handlers: deserialize settings, map
  events → operations → backend/host calls, re-render.

## 6. Behaviour

### 6.1 OpenDeck state (`opendeck_state.rs`)

Config dir: first existing of `~/.config/opendeck`,
`~/.var/app/me.amankhanna.opendeck/config/opendeck`. Never written.

- Brightness: `settings.json` → `brightness` (0–100).
- Profiles for device `D`: file stems of `profiles/D/*.json`, sorted.
- Active profile for `D`: `profiles/D.json` → `selected_profile`.

A shared task polls these files' mtimes every 1 s and publishes a `watch`
tick on change. Missing/unparseable files yield `None` ("unknown"), never an
error that reaches the user.

### 6.2 Audio action

**Target** (settings):

| Kind | Resolution |
|---|---|
| Default output | current default sink; follows default changes |
| Default input | current default source; follows default changes |
| Specific output / input | by node `name` (e.g. `alsa_output.usb-Razer…`). Absent → state *Unavailable* (grey, "unavailable"); no fallback |
| App | sink-inputs whose `application.process.binary` or `application.name` equals the configured string, case-insensitive. All matching streams are acted on together; volume shown is the first match's. None → state *Not playing* (dimmed) |

Monitor sources (`*.monitor`) are excluded from input lists.

**Gestures → operations.** Each gesture has its own operation setting.

| Gesture | Dial default | Key default |
|---|---|---|
| Rotate (ticks) | Volume ± step | — |
| Press / key tap | Toggle mute | Toggle mute |
| Long-press (held ≥ 500 ms) | Cycle device | None |
| Touch-strip tap | Toggle mute | — |

Operations: `ToggleMute`, `VolumeUp`, `VolumeDown`, `SetVolume(pct)`,
`CycleDevice`, `SetDefaultDevice(name)`, `PushToTalk` (keys only: unmute on
down, mute on up), `None`.

Press vs long-press: on down record time; on up, elapsed < 500 ms → press
op, else long-press op. If long-press is `None`, the press op fires on up
regardless of duration. `PushToTalk` acts on down/up directly and disables
long-press for that control.

`CycleDevice`: for output/input targets, sets the next device (in the
configured subset, or all non-monitor devices, ordered by description) as
default. For App targets, moves the matching streams to the next sink.

Other settings: `step` 1–20 (default 5); `max_volume` 100 or up to 150
(default 100); rotate by `n` ticks changes volume by `n × step`, clamped to
`[0, max_volume]`.

**Visuals.**
- Touch strip (custom layout `assets/layouts/audio.json`): icon (speaker /
  mic / app initial), label, percent, level bar. Muted → red bar +
  struck-through icon. Unavailable / Not playing → grey.
- Key: SVG tile rendered into the image (not the native title): level arc,
  percent, label; same state colours.
- Settings: `label` override (default: device description or app name,
  shortened to fit), `accent` colour (default `#4fc3f7`), `show_percent`
  (default true).

**Live updates.** Every `changed` tick re-renders visible Audio instances;
ticks are debounced to 50 ms. Changes made elsewhere (GNOME, pavucontrol)
appear without interaction.

**Property inspector.** On `propertyInspectorDidAppear` the plugin sends
the current outputs, inputs and running apps; the app field also accepts
free text for apps not running.

### 6.3 Device Brightness

- Dial: rotate → `deviceBrightness {action: increase|decrease, value: step}`
  (step default 5). Press → toggle between two presets (default 0 and 50).
  Strip: bar + percent of brightness read from §6.1.
- Key: one operation — set X, increase, decrease, or toggle presets. Tile
  shows current percent.

### 6.4 Switch Profile

- Key: sends `switchProfile {device, profile}` for the configured profile.
  Device defaults to the key's own device. Picker populated from §6.1.
  Tile shows the target name, highlighted when already active.
- Dial: rotate moves a highlight through an ordered list (all profiles or a
  chosen subset) without switching; press or touch tap commits. Strip shows
  active profile and highlighted candidate.

### 6.5 Host-event failure detection (Brightness, Switch Profile)

After sending a host event, if the corresponding §6.1 value has not changed
within 1.5 s, the instance shows the alert icon once and logs a warning
pointing at the OpenDeck allowlist. The warning is logged at most once per
plugin run.

## 7. Error handling

- A failed `pactl` call: keep last rendered state, `show_alert`, log.
- `pactl` not on PATH: strip/tile shows "pactl not found"; no retries loop.
- All settings structs use `#[serde(default)]`; unknown/partial settings
  never crash an instance.
- No `unwrap`/`expect` on external data (pactl output, config files,
  settings, events).

## 8. Testing

Unit tests (no I/O):
- `pactl -f json` parsing against fixtures captured from the user's machine
  (sinks, sources incl. monitors, sink-inputs incl. an app with several
  streams).
- Target resolution for each kind, including absent device / no stream.
- Gesture mapping: press vs long-press threshold, `None` long-press,
  push-to-talk.
- Volume stepping and clamping to `max_volume`.
- `CycleDevice` ordering, subset, wrap-around.
- OpenDeck state parsing: present, missing, malformed files.
- Render: SVG tile / strip payload for normal, muted, unavailable states.

Action logic tested against a fake `AudioBackend`.

CI: `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo test`.

Manual: `cargo build --release` + `node build.mjs x86_64-unknown-linux-gnu`,
copy into `~/.config/opendeck/plugins/`, exercise on the XL+.

## 9. Out of scope

- An OpenDeck fork or upstream PR to lift the allowlist.
- Per-channel (L/R) volume, EQ, or effects.
- Non-PipeWire/Pulse audio systems.
- Recreating other Starter Pack actions (Run Command, Open URL, …).
