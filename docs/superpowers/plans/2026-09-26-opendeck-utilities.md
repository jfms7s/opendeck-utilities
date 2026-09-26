# OpenDeck Utilities Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build `com.jfms7s.utilities`, an OpenDeck plugin with three customizable actions: Audio (volume, mute, device switching for an output, input or app), Device Brightness and Switch Profile.

**Architecture:** Rust binary using the `openaction` crate. Audio talks to PipeWire through `pactl` subprocesses (JSON snapshots, `pactl subscribe` for live updates) behind an `AudioBackend` trait. All decision logic (target resolution, gesture mapping, volume math, cycling, view building) is pure and unit-tested. Brightness and Switch Profile send OpenDeck host events and read OpenDeck's own config files (read-only) to show state and detect that the host ignored them.

**Tech Stack:** Rust 2024, `openaction` 2.7, `tokio`, `serde`/`serde_json`, `dashmap`, `async-trait`, `thiserror` 2, `log`/`simplelog`, `base64`. Property inspectors are plain HTML/JS. Build assembly with `node build.mjs`.

**Spec:** `docs/superpowers/specs/2026-09-26-opendeck-utilities-design.md`

## Global Constraints

- Repo root: `~/git/opendeck-utilities`. Crate `opendeck-utilities`, edition 2024, version `0.1.0`, MIT.
- Plugin UUID `com.jfms7s.utilities`; category `Utilities`; action UUIDs `com.jfms7s.utilities.audio`, `com.jfms7s.utilities.brightness`, `com.jfms7s.utilities.profile`.
- Binaries `opendeck-utilities-x86_64-unknown-linux-gnu` and `opendeck-utilities-aarch64-unknown-linux-gnu`.
- No libpulse/C bindings — audio only via the `pactl` executable, never through a shell.
- OpenDeck config is **never written**; looked up at `~/.config/opendeck` then `~/.var/app/me.amankhanna.opendeck/config/opendeck`.
- No `unwrap`/`expect` on external data (pactl output, config files, settings, events). `expect` is allowed only for logger init in `main`.
- Settings structs use `#[serde(default)]` and the `lenient` field helper so malformed values fall back instead of failing.
- Long-press threshold 500 ms; audio re-render debounce 50 ms; OpenDeck state poll 1 s; host-event confirm timeout 1.5 s; `pactl subscribe` restart backoff 1, 2, 4… s capped at 30 s.
- Audio defaults: step 5 (clamped 1–20), max volume 100 (clamped 100–150), accent `#4fc3f7`, show percent true.
- Every task ends green on: `cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test`.
- Commit messages follow Conventional Commits and end with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.

## Review Focus

1. **Accent colour is arbitrary user text** — a value like `red" onload="x` or `url(#a)` must never reach SVG attributes; anything but `#rgb`/`#rrggbb` renders with the default colour. (Test: Task 6 `sanitize_color_*`.)
2. **Device/app names that look like pactl options** — a free-text target such as `-h` or `--server=x` must be refused, not passed as an option to `pactl`. (Test: Task 4 `names_starting_with_dash_are_refused`.)
3. **Volume already above max** — a device at 130% with max 100% must not drop to 100% on a volume-*up* tick; down still works. (Test: Task 3 `volume_up_never_lowers_an_over_max_level`.)
4. **Old or garbled settings** — unknown enum strings or wrong types (e.g. `"step":"x"`, `"op":"explode"`) must yield defaults, not a dead instance. (Test: Task 3 `garbled_settings_fall_back_to_defaults`.)
5. **Profile device id from settings used in a path** — `../../.ssh` or `a/b` must not read outside OpenDeck's profiles dir. (Test: Task 8 `unsafe_device_ids_read_nothing`.)

---

## File Structure

```
Cargo.toml, rustfmt.toml, .gitignore, LICENSE, build.mjs, README.md
.github/workflows/{ci.yml,release.yml}
assets/manifest.json
assets/icons/{icon.png,actionDefaultImage.png}
assets/layouts/{level.json,profile.json}
assets/propertyInspector/{audio.html,brightness.html,profile.html}
src/main.rs                 wiring only
src/lenient.rs              serde helper: bad field value -> Default
src/cycle.rs                step through a list with wrap-around (shared)
src/host.rs                 OpenDeck host events + confirm-or-warn helpers
src/opendeck_state.rs       read-only view of OpenDeck config + change watcher
src/brightness.rs           pure brightness settings/requests/view
src/profile.rs              pure profile settings/cycling/view
src/audio/mod.rs
src/audio/model.rs          Snapshot/Device/Stream + pactl JSON parsing
src/audio/fixtures/*.json   captured-shape pactl output for tests
src/audio/target.rs         TargetKind + resolve()
src/audio/settings.rs       AudioSettings, GestureSetting, OpKind, RotateKind
src/audio/gesture.rs        Controller, Operation, effective(), gesture -> Operation
src/audio/ops.rs            volume math, cycle candidates
src/audio/backend.rs        AudioBackend trait, PactlBackend, subscriber
src/audio/exec.rs           execute(Operation) against a backend
src/audio/fake.rs           (cfg(test)) recording FakeBackend
src/audio/view.rs           Resolved -> LevelView
src/audio/pi.rs             property-inspector choices payload
src/render/mod.rs           show_level / show_profile (key vs dial)
src/render/tile.rs          SVG card, text lines, data URIs
src/render/text.rs          shorten(), sanitize_color()
src/render/icons.rs         speaker/mic/sun/letter glyphs
src/render/level.rs         LevelView -> strip feedback / key tile
src/render/profile.rs       ProfileView -> strip feedback / key tile
src/actions/mod.rs
src/actions/audio.rs
src/actions/brightness.rs
src/actions/profile.rs
```

---

### Task 1: Project scaffold

**Files:**
- Create: `Cargo.toml`, `rustfmt.toml`, `.gitignore`, `LICENSE`, `build.mjs`, `.github/workflows/ci.yml`, `.github/workflows/release.yml`, `assets/manifest.json`, `assets/icons/icon.png`, `assets/icons/actionDefaultImage.png`, `src/main.rs`

**Interfaces:**
- Produces: a building crate; `assets/manifest.json` with an `Actions` array later tasks append to; `build.mjs` that copies `assets/{icons,layouts,propertyInspector}` when present.

- [ ] **Step 1: Write `Cargo.toml`**

```toml
[package]
name = "opendeck-utilities"
version = "0.1.0"
edition = "2024"
license = "MIT"
description = "OpenDeck plugin: customizable audio, device brightness and profile switching controls"
repository = "https://github.com/jfms7s/opendeck-utilities"

[dependencies]
openaction = "2.7"
tokio = { version = "1.48", features = ["rt-multi-thread", "macros", "process", "time", "sync", "io-util"] }
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
dashmap = "6"
async-trait = "0.1"
thiserror = "2"
log = "0.4"
simplelog = "0.12"
base64 = "0.22"

[dev-dependencies]
tokio = { version = "1.48", features = ["test-util", "macros", "rt"] }
tempfile = "3"
```

- [ ] **Step 2: Copy conventions from `opendeck-power-profile`**

```bash
cd ~/git/opendeck-utilities
cp ../opendeck-power-profile/rustfmt.toml ../opendeck-power-profile/.gitignore ../opendeck-power-profile/LICENSE .
mkdir -p .github/workflows assets/icons assets/layouts assets/propertyInspector src
cp ../opendeck-power-profile/.github/workflows/ci.yml .github/workflows/ci.yml
sed -e 's/opendeck-power-profile/opendeck-utilities/g' -e 's/com\.jfms7s\.powerprofile/com.jfms7s.utilities/g' \
  ../opendeck-power-profile/.github/workflows/release.yml > .github/workflows/release.yml
cp ../opendeck-power-profile/assets/icons/icon.png ../opendeck-power-profile/assets/icons/actionDefaultImage.png assets/icons/
grep -rn "power" .github && echo "LEFTOVER NAMES - fix them" || echo ok
```

Expected: `ok`.

- [ ] **Step 3: Write `build.mjs`**

```js
#!/usr/bin/env node
// Assembles dist/<uuid>.sdPlugin/ from assets/ + a release binary for one target.
// Usage: node build.mjs <target-triple>
// Requires: cargo build --release --target <target-triple> already run for that triple.
import { cpSync, copyFileSync, mkdirSync, rmSync, existsSync, readFileSync } from "node:fs";
import { join } from "node:path";

const UUID = "com.jfms7s.utilities";
const BIN_NAME = "opendeck-utilities";

const target = process.argv[2];
if (!target) {
	console.error("usage: node build.mjs <target-triple>");
	process.exit(1);
}

// Cargo.toml's version and manifest.json's "Version" have nothing keeping
// them in sync - catch drift here rather than shipping a mismatch.
const cargoVersion = readFileSync("Cargo.toml", "utf8").match(/^version\s*=\s*"([^"]+)"/m)?.[1];
const manifestVersion = JSON.parse(readFileSync("assets/manifest.json", "utf8")).Version;
if (!cargoVersion || cargoVersion !== manifestVersion) {
	console.error(`version mismatch: Cargo.toml is ${cargoVersion} but assets/manifest.json is ${manifestVersion} - bump them together`);
	process.exit(1);
}

const binPath = join("target", target, "release", BIN_NAME);
if (!existsSync(binPath)) {
	console.error(`missing release binary: ${binPath} (run: cargo build --release --target ${target})`);
	process.exit(1);
}

const outDir = join("dist", `${UUID}.sdPlugin`);
rmSync(outDir, { recursive: true, force: true });
mkdirSync(outDir, { recursive: true });

cpSync("assets/manifest.json", join(outDir, "manifest.json"));
for (const dir of ["icons", "layouts", "propertyInspector"]) {
	if (existsSync(join("assets", dir))) {
		cpSync(join("assets", dir), join(outDir, dir), { recursive: true });
	}
}
copyFileSync(binPath, join(outDir, `${BIN_NAME}-${target}`));

console.log(`built ${outDir} for ${target}`);
```

- [ ] **Step 4: Write `assets/manifest.json`** (actions are appended by Tasks 7, 9, 10)

```json
{
	"Name": "Utilities",
	"Author": "jfms7s",
	"Version": "0.1.0",
	"Category": "Utilities",
	"Icon": "icons/icon",
	"OS": [{ "Platform": "linux" }],
	"CodePaths": {
		"x86_64-unknown-linux-gnu": "opendeck-utilities-x86_64-unknown-linux-gnu",
		"aarch64-unknown-linux-gnu": "opendeck-utilities-aarch64-unknown-linux-gnu"
	},
	"CodePathLin": "opendeck-utilities-x86_64-unknown-linux-gnu",
	"Actions": []
}
```

- [ ] **Step 5: Write the failing test in `src/main.rs`**

```rust
use openaction::{OpenActionResult, run};

#[tokio::main]
async fn main() -> OpenActionResult<()> {
    simplelog::SimpleLogger::init(log::LevelFilter::Info, simplelog::Config::default())
        .expect("logger init");
    run(std::env::args().collect()).await
}

#[cfg(test)]
mod tests {
    #[test]
    fn manifest_version_matches_cargo() {
        let manifest: serde_json::Value =
            serde_json::from_str(include_str!("../assets/manifest.json")).unwrap();
        assert_eq!(manifest["Version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(
            manifest["CodePathLin"],
            "opendeck-utilities-x86_64-unknown-linux-gnu"
        );
    }
}
```

- [ ] **Step 6: Run and verify**

Run: `cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test`
Expected: PASS, 1 test.

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "chore: scaffold the utilities plugin crate, build script and CI

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: Audio model, pactl JSON parsing and target resolution

**Files:**
- Create: `src/audio/mod.rs`, `src/audio/model.rs`, `src/audio/target.rs`, `src/audio/fixtures/{info,sinks,sources,sink_inputs}.json`
- Modify: `src/main.rs` (add `#[allow(dead_code)] mod audio;` — the allow is removed in Task 7)

**Interfaces:**
- Produces:
  - `enum DeviceKind { Output, Input }`
  - `struct Device { index: u32, name: String, description: String, volume: u16, muted: bool }`
  - `struct Stream { index: u32, sink: u32, app_name: String, binary: String, volume: u16, muted: bool }`
  - `struct Snapshot { default_sink, default_source: String, sinks, sources: Vec<Device>, streams: Vec<Stream> }` with `devices(kind) -> &[Device]`, `default_name(kind) -> &str`, `sink_by_index(u32) -> Option<&Device>`
  - `struct ParseError`; `fn build_snapshot(info, sinks, sources, sink_inputs: &str) -> Result<Snapshot, ParseError>`
  - `#[cfg(test)] pub(crate) fn test_support::fixture_snapshot() -> Snapshot`
  - `enum TargetKind { DefaultOutput, DefaultInput, Output, Input, App }` (serde snake_case, default `DefaultOutput`)
  - `enum Resolved { Device { kind, device }, App { streams }, Unavailable { label }, NotPlaying { label } }` with `label() -> String`
  - `fn resolve(kind: TargetKind, name: &str, snap: &Snapshot) -> Resolved`

- [ ] **Step 1: Write fixtures** (same shape as `pactl -f json` on the user's machine; extra fields are deliberately present and must be ignored)

`src/audio/fixtures/info.json`:
```json
{"server_name":"PulseAudio (on PipeWire 1.6.9)","default_sample_specification":"float32le 2ch 48000Hz","default_sink_name":"alsa_output.usb-Razer_Razer_Leviathan_V2-00.analog-stereo","default_source_name":"alsa_input.usb-webcam-02.mono-fallback"}
```

`src/audio/fixtures/sinks.json`:
```json
[
 {"index":60,"state":"SUSPENDED","name":"alsa_output.pci-0000_2f_00.4.iec958-stereo","description":"Starship/Matisse HD Audio Controller Digital Stereo (IEC958)","driver":"PipeWire","mute":false,"volume":{"front-left":{"value":65536,"value_percent":"100%","db":"0.00 dB"},"front-right":{"value":65536,"value_percent":"100%","db":"0.00 dB"}},"balance":0.0,"properties":{"device.class":"sound"},"ports":[],"active_port":"iec958-stereo-output"},
 {"index":61,"state":"RUNNING","name":"alsa_output.usb-Razer_Razer_Leviathan_V2-00.analog-stereo","description":"Razer Leviathan V2 Analog Stereo","driver":"PipeWire","mute":false,"volume":{"front-left":{"value":29491,"value_percent":"45%","db":"-20.81 dB"},"front-right":{"value":30801,"value_percent":"47%","db":"-19.68 dB"}},"balance":0.0,"properties":{},"ports":[]}
]
```

`src/audio/fixtures/sources.json`:
```json
[
 {"index":70,"name":"alsa_input.usb-webcam-02.mono-fallback","description":"Full HD webcam Mono","mute":true,"volume":{"mono":{"value":52429,"value_percent":"80%","db":"-5.81 dB"}},"properties":{}},
 {"index":71,"name":"alsa_output.usb-Razer_Razer_Leviathan_V2-00.analog-stereo.monitor","description":"Monitor of Razer Leviathan V2 Analog Stereo","mute":false,"volume":{"front-left":{"value":65536,"value_percent":"100%","db":"0.00 dB"}},"properties":{}}
]
```

`src/audio/fixtures/sink_inputs.json`:
```json
[
 {"index":100,"driver":"PipeWire","client":90,"sink":61,"mute":false,"volume":{"front-left":{"value":65536,"value_percent":"100%","db":"0.00 dB"},"front-right":{"value":65536,"value_percent":"100%","db":"0.00 dB"}},"properties":{"application.name":"Google Chrome","application.process.binary":"chrome","media.name":"Playback"}},
 {"index":101,"driver":"PipeWire","client":90,"sink":61,"mute":false,"volume":{"front-left":{"value":58982,"value_percent":"90%","db":"-2.75 dB"}},"properties":{"application.name":"Google Chrome","application.process.binary":"chrome"}},
 {"index":102,"driver":"PipeWire","client":91,"sink":60,"mute":true,"volume":{"front-left":{"value":19661,"value_percent":"30%","db":"-31.4 dB"}},"properties":{"application.name":"Spotify","application.process.binary":"spotify"}},
 {"index":103,"driver":"PipeWire","client":92,"sink":61,"mute":false,"volume":{"front-left":{"value":65536,"value_percent":"100%","db":"0.00 dB"}}}
]
```

- [ ] **Step 2: Write `src/audio/mod.rs`**

```rust
pub mod model;
pub mod target;
```

- [ ] **Step 3: Write `src/audio/model.rs` with failing tests first**

```rust
//! Plain data types for the audio system, and parsing of `pactl -f json`
//! output into them. Nothing here runs `pactl` - see `backend.rs`.

use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceKind {
    Output,
    Input,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Device {
    pub index: u32,
    /// Stable node name, e.g. `alsa_output.usb-...analog-stereo`.
    pub name: String,
    pub description: String,
    /// Loudest channel, in percent (may exceed 100).
    pub volume: u16,
    pub muted: bool,
}

/// One application playback stream (a pactl "sink-input").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stream {
    pub index: u32,
    /// Index of the sink this stream plays to.
    pub sink: u32,
    pub app_name: String,
    pub binary: String,
    pub volume: u16,
    pub muted: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Snapshot {
    pub default_sink: String,
    pub default_source: String,
    pub sinks: Vec<Device>,
    /// Inputs only - monitor sources are excluded.
    pub sources: Vec<Device>,
    pub streams: Vec<Stream>,
}

impl Snapshot {
    pub fn devices(&self, kind: DeviceKind) -> &[Device] {
        match kind {
            DeviceKind::Output => &self.sinks,
            DeviceKind::Input => &self.sources,
        }
    }

    pub fn default_name(&self, kind: DeviceKind) -> &str {
        match kind {
            DeviceKind::Output => &self.default_sink,
            DeviceKind::Input => &self.default_source,
        }
    }

    pub fn sink_by_index(&self, index: u32) -> Option<&Device> {
        self.sinks.iter().find(|d| d.index == index)
    }
}

#[derive(Debug, thiserror::Error)]
#[error("invalid pactl json: {0}")]
pub struct ParseError(#[from] serde_json::Error);

#[derive(Deserialize)]
struct RawChannel {
    value_percent: String,
}

#[derive(Deserialize)]
struct RawDevice {
    index: u32,
    name: String,
    #[serde(default)]
    description: String,
    mute: bool,
    #[serde(default)]
    volume: HashMap<String, RawChannel>,
}

#[derive(Deserialize)]
struct RawStream {
    index: u32,
    sink: u32,
    mute: bool,
    #[serde(default)]
    volume: HashMap<String, RawChannel>,
    #[serde(default)]
    properties: HashMap<String, Value>,
}

#[derive(Deserialize)]
struct RawInfo {
    #[serde(default)]
    default_sink_name: String,
    #[serde(default)]
    default_source_name: String,
}

fn percent(volume: &HashMap<String, RawChannel>) -> u16 {
    volume
        .values()
        .filter_map(|c| c.value_percent.trim().trim_end_matches('%').parse::<u16>().ok())
        .max()
        .unwrap_or(0)
}

fn prop(properties: &HashMap<String, Value>, key: &str) -> String {
    properties
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn parse_devices(json: &str) -> Result<Vec<Device>, ParseError> {
    let raw: Vec<RawDevice> = serde_json::from_str(json)?;
    Ok(raw
        .into_iter()
        .map(|d| Device {
            volume: percent(&d.volume),
            description: if d.description.is_empty() {
                d.name.clone()
            } else {
                d.description
            },
            index: d.index,
            name: d.name,
            muted: d.mute,
        })
        .collect())
}

fn parse_streams(json: &str) -> Result<Vec<Stream>, ParseError> {
    let raw: Vec<RawStream> = serde_json::from_str(json)?;
    Ok(raw
        .into_iter()
        .map(|s| Stream {
            index: s.index,
            sink: s.sink,
            app_name: prop(&s.properties, "application.name"),
            binary: prop(&s.properties, "application.process.binary"),
            volume: percent(&s.volume),
            muted: s.mute,
        })
        .collect())
}

pub fn is_monitor(name: &str) -> bool {
    name.ends_with(".monitor")
}

/// Combines the four `pactl -f json` outputs (`info`, `list sinks`,
/// `list sources`, `list sink-inputs`) into one snapshot.
pub fn build_snapshot(
    info: &str,
    sinks: &str,
    sources: &str,
    sink_inputs: &str,
) -> Result<Snapshot, ParseError> {
    let info: RawInfo = serde_json::from_str(info)?;
    let mut sources = parse_devices(sources)?;
    sources.retain(|d| !is_monitor(&d.name));
    Ok(Snapshot {
        default_sink: info.default_sink_name,
        default_source: info.default_source_name,
        sinks: parse_devices(sinks)?,
        sources,
        streams: parse_streams(sink_inputs)?,
    })
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;

    pub(crate) fn fixture_snapshot() -> Snapshot {
        build_snapshot(
            include_str!("fixtures/info.json"),
            include_str!("fixtures/sinks.json"),
            include_str!("fixtures/sources.json"),
            include_str!("fixtures/sink_inputs.json"),
        )
        .unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::fixture_snapshot;
    use super::*;

    #[test]
    fn parses_defaults_from_info() {
        let s = fixture_snapshot();
        assert_eq!(s.default_sink, "alsa_output.usb-Razer_Razer_Leviathan_V2-00.analog-stereo");
        assert_eq!(s.default_source, "alsa_input.usb-webcam-02.mono-fallback");
    }

    #[test]
    fn device_volume_is_the_loudest_channel() {
        let s = fixture_snapshot();
        assert_eq!(s.sinks.len(), 2);
        assert_eq!(s.sinks[1].volume, 47);
        assert_eq!(s.sinks[1].description, "Razer Leviathan V2 Analog Stereo");
    }

    #[test]
    fn monitor_sources_are_excluded() {
        let s = fixture_snapshot();
        assert_eq!(s.sources.len(), 1);
        assert!(s.sources[0].muted);
        assert_eq!(s.sources[0].volume, 80);
    }

    #[test]
    fn streams_carry_app_identity_and_sink() {
        let s = fixture_snapshot();
        assert_eq!(s.streams.len(), 4);
        assert_eq!(s.streams[0].binary, "chrome");
        assert_eq!(s.streams[0].app_name, "Google Chrome");
        assert_eq!(s.streams[2].sink, 60);
        assert!(s.streams[2].muted);
    }

    #[test]
    fn stream_without_properties_gets_empty_identity() {
        let s = fixture_snapshot();
        assert_eq!(s.streams[3].app_name, "");
        assert_eq!(s.streams[3].binary, "");
    }

    #[test]
    fn missing_description_falls_back_to_name() {
        let devices = parse_devices(r#"[{"index":1,"name":"n","mute":false}]"#).unwrap();
        assert_eq!(devices[0].description, "n");
        assert_eq!(devices[0].volume, 0);
    }

    #[test]
    fn malformed_json_is_an_error() {
        assert!(build_snapshot("{", "[]", "[]", "[]").is_err());
        assert!(build_snapshot("{}", "not json", "[]", "[]").is_err());
    }
}
```

- [ ] **Step 4: Add the module to `src/main.rs`** (above `use openaction...`)

```rust
#[allow(dead_code)]
mod audio;
```

- [ ] **Step 5: Run model tests**

Run: `cargo test audio::model`
Expected: PASS, 7 tests.

- [ ] **Step 6: Write `src/audio/target.rs` with tests**

```rust
//! Which device or app streams a control acts on, resolved against a
//! snapshot. Pure - no I/O.

use super::model::{Device, DeviceKind, Snapshot, Stream};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetKind {
    #[default]
    DefaultOutput,
    DefaultInput,
    Output,
    Input,
    App,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolved {
    Device { kind: DeviceKind, device: Device },
    /// Never empty.
    App { streams: Vec<Stream> },
    Unavailable { label: String },
    NotPlaying { label: String },
}

impl Resolved {
    pub fn label(&self) -> String {
        match self {
            Resolved::Device { device, .. } => device.description.clone(),
            Resolved::App { streams } => streams
                .first()
                .map(|s| {
                    if s.app_name.is_empty() {
                        s.binary.clone()
                    } else {
                        s.app_name.clone()
                    }
                })
                .unwrap_or_default(),
            Resolved::Unavailable { label } | Resolved::NotPlaying { label } => label.clone(),
        }
    }
}

pub fn app_matches(stream: &Stream, name: &str) -> bool {
    stream.binary.eq_ignore_ascii_case(name) || stream.app_name.eq_ignore_ascii_case(name)
}

fn by_name(kind: DeviceKind, name: &str, snap: &Snapshot, fallback: &str) -> Resolved {
    snap.devices(kind)
        .iter()
        .find(|d| d.name == name)
        .map(|d| Resolved::Device {
            kind,
            device: d.clone(),
        })
        .unwrap_or_else(|| Resolved::Unavailable {
            label: fallback.to_string(),
        })
}

pub fn resolve(kind: TargetKind, name: &str, snap: &Snapshot) -> Resolved {
    let name = name.trim();
    match kind {
        TargetKind::DefaultOutput => {
            by_name(DeviceKind::Output, &snap.default_sink, snap, "Default output")
        }
        TargetKind::DefaultInput => {
            by_name(DeviceKind::Input, &snap.default_source, snap, "Default input")
        }
        TargetKind::Output | TargetKind::Input if name.is_empty() => Resolved::Unavailable {
            label: "pick a device".to_string(),
        },
        TargetKind::Output => by_name(DeviceKind::Output, name, snap, name),
        TargetKind::Input => by_name(DeviceKind::Input, name, snap, name),
        TargetKind::App if name.is_empty() => Resolved::Unavailable {
            label: "pick an app".to_string(),
        },
        TargetKind::App => {
            let streams: Vec<Stream> = snap
                .streams
                .iter()
                .filter(|s| app_matches(s, name))
                .cloned()
                .collect();
            if streams.is_empty() {
                Resolved::NotPlaying {
                    label: name.to_string(),
                }
            } else {
                Resolved::App { streams }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::model::test_support::fixture_snapshot;

    #[test]
    fn default_output_follows_the_default_sink() {
        let r = resolve(TargetKind::DefaultOutput, "", &fixture_snapshot());
        let Resolved::Device { kind, device } = r else { panic!("{r:?}") };
        assert_eq!(kind, DeviceKind::Output);
        assert_eq!(device.index, 61);
    }

    #[test]
    fn default_input_follows_the_default_source() {
        let r = resolve(TargetKind::DefaultInput, "", &fixture_snapshot());
        assert!(matches!(r, Resolved::Device { kind: DeviceKind::Input, ref device } if device.index == 70));
    }

    #[test]
    fn specific_output_by_node_name() {
        let r = resolve(
            TargetKind::Output,
            " alsa_output.pci-0000_2f_00.4.iec958-stereo ",
            &fixture_snapshot(),
        );
        assert!(matches!(r, Resolved::Device { ref device, .. } if device.index == 60));
    }

    #[test]
    fn absent_device_is_unavailable_without_fallback() {
        let r = resolve(TargetKind::Output, "unplugged-headset", &fixture_snapshot());
        assert_eq!(r, Resolved::Unavailable { label: "unplugged-headset".into() });
    }

    #[test]
    fn a_monitor_is_not_a_valid_input() {
        let r = resolve(
            TargetKind::Input,
            "alsa_output.usb-Razer_Razer_Leviathan_V2-00.analog-stereo.monitor",
            &fixture_snapshot(),
        );
        assert!(matches!(r, Resolved::Unavailable { .. }));
    }

    #[test]
    fn empty_specific_name_asks_for_a_device() {
        let r = resolve(TargetKind::Input, "  ", &fixture_snapshot());
        assert_eq!(r, Resolved::Unavailable { label: "pick a device".into() });
    }

    #[test]
    fn app_matches_binary_or_name_case_insensitively_and_groups_streams() {
        for name in ["chrome", "CHROME", "google chrome"] {
            let r = resolve(TargetKind::App, name, &fixture_snapshot());
            let Resolved::App { streams } = r else { panic!("{name}: {r:?}") };
            assert_eq!(streams.iter().map(|s| s.index).collect::<Vec<_>>(), vec![100, 101]);
        }
    }

    #[test]
    fn app_with_no_stream_is_not_playing() {
        let r = resolve(TargetKind::App, "firefox", &fixture_snapshot());
        assert_eq!(r, Resolved::NotPlaying { label: "firefox".into() });
    }

    #[test]
    fn empty_app_name_never_matches_nameless_streams() {
        let r = resolve(TargetKind::App, "", &fixture_snapshot());
        assert_eq!(r, Resolved::Unavailable { label: "pick an app".into() });
    }

    #[test]
    fn labels() {
        let s = fixture_snapshot();
        assert_eq!(resolve(TargetKind::DefaultOutput, "", &s).label(), "Razer Leviathan V2 Analog Stereo");
        assert_eq!(resolve(TargetKind::App, "spotify", &s).label(), "Spotify");
    }
}
```

Add `pub mod target;` is already in `mod.rs` from Step 2.

- [ ] **Step 7: Run and verify**

Run: `cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test`
Expected: PASS (1 + 7 + 10 tests).

- [ ] **Step 8: Commit**

```bash
git add -A
git commit -m "feat(audio): parse pactl JSON snapshots and resolve control targets

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 3: Audio settings, gesture mapping, volume math and cycling

**Files:**
- Create: `src/lenient.rs`, `src/cycle.rs`, `src/audio/settings.rs`, `src/audio/gesture.rs`, `src/audio/ops.rs`
- Modify: `src/audio/mod.rs`, `src/main.rs` (add `mod cycle;` and `mod lenient;`, both `#[allow(dead_code)]` until Task 7)

**Interfaces:**
- Consumes: `TargetKind`, `Device` (Task 2).
- Produces:
  - `lenient::lenient` — serde `deserialize_with` helper.
  - `cycle::step_in(list: &[String], current: &str, steps: i64) -> Option<String>`
  - `settings::{OpKind, RotateKind, GestureSetting { op, volume: u16, device: String }, AudioSettings}`; `AudioSettings::step() -> u16`, `AudioSettings::max_volume() -> u16`; `DEFAULT_ACCENT: &str`
  - `gesture::{LONG_PRESS, Controller { Keypad, Encoder }, Controller::from_openaction(&str), Operation, Effective { press, long_press, touch_tap }, effective(&AudioSettings, Controller) -> Effective, gesture_operation(&GestureSetting, step: u16) -> Operation, rotate_operation(RotateKind, ticks: i16, step: u16) -> Operation, down_operation(&Effective) -> Operation, release_operation(&Effective, held: Duration, step: u16) -> Operation}`
  - `Operation { None, ToggleMute, SetMute(bool), AdjustVolume(i32), SetVolume(u16), CycleDevice(i32), SetDefaultDevice(String) }`
  - `ops::{adjust_volume(current: u16, delta: i32, max: u16) -> u16, cycle_candidates(&[Device], subset: &[String]) -> Vec<String>}`

- [ ] **Step 1: Write `src/lenient.rs`**

```rust
//! Serde helper: a settings field whose stored value no longer parses
//! (an old enum name, a string where a number belongs) falls back to its
//! type's default instead of failing the whole settings object - a failed
//! parse would otherwise leave the control dead until reconfigured.

use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer};
use serde_json::Value;

pub fn lenient<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: DeserializeOwned + Default,
{
    let value = Value::deserialize(deserializer)?;
    Ok(serde_json::from_value(value).unwrap_or_default())
}
```

- [ ] **Step 2: Write `src/cycle.rs` with tests**

```rust
//! Stepping through an ordered list with wrap-around - shared by audio
//! device cycling and the profile dial.

/// Moves `steps` places from `current` (negative = backwards), wrapping.
/// If `current` isn't in the list, stepping forward starts at the first
/// entry and stepping backward at the last. `None` only for an empty list.
pub fn step_in(list: &[String], current: &str, steps: i64) -> Option<String> {
    if list.is_empty() {
        return None;
    }
    let len = list.len() as i64;
    let base = match list.iter().position(|x| x == current) {
        Some(i) => i as i64,
        None if steps > 0 => -1,
        None => 0,
    };
    let next = (base + steps).rem_euclid(len);
    list.get(next as usize).cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list() -> Vec<String> {
        ["a", "b", "c"].map(String::from).to_vec()
    }

    #[test]
    fn steps_forward_and_wraps() {
        assert_eq!(step_in(&list(), "b", 1).as_deref(), Some("c"));
        assert_eq!(step_in(&list(), "c", 1).as_deref(), Some("a"));
        assert_eq!(step_in(&list(), "a", 4).as_deref(), Some("b"));
    }

    #[test]
    fn steps_backward_and_wraps() {
        assert_eq!(step_in(&list(), "a", -1).as_deref(), Some("c"));
        assert_eq!(step_in(&list(), "b", -5).as_deref(), Some("c"));
    }

    #[test]
    fn unknown_current_starts_at_an_end() {
        assert_eq!(step_in(&list(), "zzz", 1).as_deref(), Some("a"));
        assert_eq!(step_in(&list(), "zzz", -1).as_deref(), Some("c"));
    }

    #[test]
    fn empty_list_is_none() {
        assert_eq!(step_in(&[], "a", 1), None);
    }
}
```

- [ ] **Step 3: Write `src/audio/settings.rs` with the Review Focus #4 test**

```rust
use super::target::TargetKind;
use crate::lenient::lenient;
use serde::{Deserialize, Serialize};

pub const DEFAULT_ACCENT: &str = "#4fc3f7";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OpKind {
    #[default]
    None,
    ToggleMute,
    VolumeUp,
    VolumeDown,
    SetVolume,
    CycleDevice,
    SetDefaultDevice,
    PushToTalk,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RotateKind {
    #[default]
    Volume,
    CycleDevice,
    None,
}

/// One gesture's configured operation. `volume` is used by `SetVolume`,
/// `device` by `SetDefaultDevice`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct GestureSetting {
    #[serde(deserialize_with = "lenient")]
    pub op: OpKind,
    #[serde(deserialize_with = "lenient")]
    pub volume: u16,
    #[serde(deserialize_with = "lenient")]
    pub device: String,
}

/// `None` gesture fields mean "use the controller's default" (see
/// `gesture::effective`) - the same settings behave sensibly on a key or a dial.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioSettings {
    #[serde(deserialize_with = "lenient")]
    pub target: TargetKind,
    #[serde(deserialize_with = "lenient")]
    pub target_name: String,
    #[serde(deserialize_with = "lenient")]
    pub step: u16,
    #[serde(deserialize_with = "lenient")]
    pub max_volume: u16,
    #[serde(deserialize_with = "lenient")]
    pub rotate: RotateKind,
    #[serde(deserialize_with = "lenient")]
    pub press: Option<GestureSetting>,
    #[serde(deserialize_with = "lenient")]
    pub long_press: Option<GestureSetting>,
    #[serde(deserialize_with = "lenient")]
    pub touch_tap: Option<GestureSetting>,
    /// Device node names to cycle through; empty = all.
    #[serde(deserialize_with = "lenient")]
    pub cycle_devices: Vec<String>,
    #[serde(deserialize_with = "lenient")]
    pub label: String,
    #[serde(deserialize_with = "lenient")]
    pub accent: String,
    #[serde(deserialize_with = "lenient")]
    pub show_percent: bool,
}

impl Default for AudioSettings {
    fn default() -> Self {
        Self {
            target: TargetKind::DefaultOutput,
            target_name: String::new(),
            step: 5,
            max_volume: 100,
            rotate: RotateKind::Volume,
            press: None,
            long_press: None,
            touch_tap: None,
            cycle_devices: Vec::new(),
            label: String::new(),
            accent: DEFAULT_ACCENT.to_string(),
            show_percent: true,
        }
    }
}

impl AudioSettings {
    pub fn step(&self) -> u16 {
        self.step.clamp(1, 20)
    }

    pub fn max_volume(&self) -> u16 {
        self.max_volume.clamp(100, 150)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn empty_object_is_the_default() {
        let s: AudioSettings = serde_json::from_value(json!({})).unwrap();
        assert_eq!(s, AudioSettings::default());
    }

    #[test]
    fn round_trips() {
        let s = AudioSettings {
            target: TargetKind::App,
            target_name: "chrome".into(),
            press: Some(GestureSetting { op: OpKind::SetVolume, volume: 30, device: String::new() }),
            ..AudioSettings::default()
        };
        let back: AudioSettings = serde_json::from_value(serde_json::to_value(&s).unwrap()).unwrap();
        assert_eq!(back, s);
    }

    #[test]
    fn garbled_settings_fall_back_to_defaults() {
        let s: AudioSettings = serde_json::from_value(json!({
            "target": "bogus",
            "step": "x",
            "max_volume": -3,
            "press": { "op": "explode", "volume": 20 },
            "cycle_devices": "not a list",
            "show_percent": "yes"
        }))
        .unwrap();
        assert_eq!(s.target, TargetKind::DefaultOutput);
        assert_eq!(s.step(), 1);
        assert_eq!(s.max_volume(), 100);
        assert_eq!(s.press, Some(GestureSetting { op: OpKind::None, volume: 20, device: String::new() }));
        assert!(s.cycle_devices.is_empty());
        assert!(!s.show_percent);
    }

    #[test]
    fn step_and_max_are_clamped() {
        let s = AudioSettings { step: 99, max_volume: 500, ..AudioSettings::default() };
        assert_eq!(s.step(), 20);
        assert_eq!(s.max_volume(), 150);
    }
}
```

- [ ] **Step 4: Write `src/audio/gesture.rs` with tests**

```rust
//! Maps a physical gesture to an audio operation using the instance's
//! settings. Pure - timing is passed in, not measured here.

use super::settings::{AudioSettings, GestureSetting, OpKind, RotateKind};
use std::time::Duration;

pub const LONG_PRESS: Duration = Duration::from_millis(500);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Controller {
    Keypad,
    Encoder,
}

impl Controller {
    /// OpenDeck reports `"Keypad"` or `"Encoder"`.
    pub fn from_openaction(controller: &str) -> Self {
        if controller == "Keypad" {
            Controller::Keypad
        } else {
            Controller::Encoder
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Operation {
    None,
    ToggleMute,
    SetMute(bool),
    AdjustVolume(i32),
    SetVolume(u16),
    CycleDevice(i32),
    SetDefaultDevice(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Effective {
    pub press: GestureSetting,
    pub long_press: GestureSetting,
    pub touch_tap: GestureSetting,
}

fn op(op: OpKind) -> GestureSetting {
    GestureSetting { op, ..GestureSetting::default() }
}

/// Fills unset gestures with the controller's defaults (spec §6.2).
pub fn effective(settings: &AudioSettings, controller: Controller) -> Effective {
    let (press, long_press, touch_tap) = match controller {
        Controller::Keypad => (OpKind::ToggleMute, OpKind::None, OpKind::None),
        Controller::Encoder => (OpKind::ToggleMute, OpKind::CycleDevice, OpKind::ToggleMute),
    };
    Effective {
        press: settings.press.clone().unwrap_or_else(|| op(press)),
        long_press: settings.long_press.clone().unwrap_or_else(|| op(long_press)),
        touch_tap: settings.touch_tap.clone().unwrap_or_else(|| op(touch_tap)),
    }
}

/// Push-to-talk is not a one-shot operation; it is handled by
/// `down_operation`/`release_operation` and maps to `None` here.
pub fn gesture_operation(gesture: &GestureSetting, step: u16) -> Operation {
    let step = i32::from(step);
    match gesture.op {
        OpKind::None | OpKind::PushToTalk => Operation::None,
        OpKind::ToggleMute => Operation::ToggleMute,
        OpKind::VolumeUp => Operation::AdjustVolume(step),
        OpKind::VolumeDown => Operation::AdjustVolume(-step),
        OpKind::SetVolume => Operation::SetVolume(gesture.volume),
        OpKind::CycleDevice => Operation::CycleDevice(1),
        OpKind::SetDefaultDevice => Operation::SetDefaultDevice(gesture.device.trim().to_string()),
    }
}

pub fn rotate_operation(kind: RotateKind, ticks: i16, step: u16) -> Operation {
    match kind {
        _ if ticks == 0 => Operation::None,
        RotateKind::Volume => Operation::AdjustVolume(i32::from(ticks) * i32::from(step)),
        RotateKind::CycleDevice => Operation::CycleDevice(i32::from(ticks.signum())),
        RotateKind::None => Operation::None,
    }
}

/// On key/dial down: only push-to-talk acts immediately (unmute).
pub fn down_operation(e: &Effective) -> Operation {
    if e.press.op == OpKind::PushToTalk {
        Operation::SetMute(false)
    } else {
        Operation::None
    }
}

/// On key/dial up. A hold counts as a long-press only when a long-press
/// operation is configured; otherwise any hold is a plain press.
pub fn release_operation(e: &Effective, held: Duration, step: u16) -> Operation {
    if e.press.op == OpKind::PushToTalk {
        return Operation::SetMute(true);
    }
    if e.long_press.op != OpKind::None && held >= LONG_PRESS {
        gesture_operation(&e.long_press, step)
    } else {
        gesture_operation(&e.press, step)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn eff(controller: Controller) -> Effective {
        effective(&AudioSettings::default(), controller)
    }

    #[test]
    fn controller_defaults() {
        let k = eff(Controller::Keypad);
        assert_eq!((k.press.op, k.long_press.op, k.touch_tap.op), (OpKind::ToggleMute, OpKind::None, OpKind::None));
        let d = eff(Controller::Encoder);
        assert_eq!((d.press.op, d.long_press.op, d.touch_tap.op), (OpKind::ToggleMute, OpKind::CycleDevice, OpKind::ToggleMute));
    }

    #[test]
    fn configured_gestures_override_defaults() {
        let s = AudioSettings { long_press: Some(op(OpKind::None)), ..AudioSettings::default() };
        assert_eq!(effective(&s, Controller::Encoder).long_press.op, OpKind::None);
    }

    #[test]
    fn short_press_runs_press_op() {
        let e = eff(Controller::Encoder);
        assert_eq!(release_operation(&e, Duration::from_millis(200), 5), Operation::ToggleMute);
    }

    #[test]
    fn hold_at_threshold_runs_long_press_op() {
        let e = eff(Controller::Encoder);
        assert_eq!(release_operation(&e, LONG_PRESS, 5), Operation::CycleDevice(1));
    }

    #[test]
    fn hold_without_long_press_op_is_a_press() {
        let e = eff(Controller::Keypad);
        assert_eq!(release_operation(&e, Duration::from_secs(3), 5), Operation::ToggleMute);
    }

    #[test]
    fn push_to_talk_unmutes_on_down_and_mutes_on_up_even_when_held() {
        let s = AudioSettings {
            press: Some(op(OpKind::PushToTalk)),
            long_press: Some(op(OpKind::CycleDevice)),
            ..AudioSettings::default()
        };
        let e = effective(&s, Controller::Keypad);
        assert_eq!(down_operation(&e), Operation::SetMute(false));
        assert_eq!(release_operation(&e, Duration::from_secs(2), 5), Operation::SetMute(true));
    }

    #[test]
    fn non_ptt_down_does_nothing() {
        assert_eq!(down_operation(&eff(Controller::Keypad)), Operation::None);
    }

    #[test]
    fn gesture_ops_carry_their_parameters() {
        let g = GestureSetting { op: OpKind::SetVolume, volume: 35, device: String::new() };
        assert_eq!(gesture_operation(&g, 5), Operation::SetVolume(35));
        let g = GestureSetting { op: OpKind::SetDefaultDevice, volume: 0, device: " hdmi ".into() };
        assert_eq!(gesture_operation(&g, 5), Operation::SetDefaultDevice("hdmi".into()));
        assert_eq!(gesture_operation(&op(OpKind::VolumeDown), 7), Operation::AdjustVolume(-7));
    }

    #[test]
    fn rotation() {
        assert_eq!(rotate_operation(RotateKind::Volume, -3, 5), Operation::AdjustVolume(-15));
        assert_eq!(rotate_operation(RotateKind::CycleDevice, 4, 5), Operation::CycleDevice(1));
        assert_eq!(rotate_operation(RotateKind::Volume, 0, 5), Operation::None);
        assert_eq!(rotate_operation(RotateKind::None, 2, 5), Operation::None);
    }

    #[test]
    fn controller_from_openaction() {
        assert_eq!(Controller::from_openaction("Keypad"), Controller::Keypad);
        assert_eq!(Controller::from_openaction("Encoder"), Controller::Encoder);
    }
}
```

- [ ] **Step 5: Write `src/audio/ops.rs` with the Review Focus #3 test**

```rust
use super::model::Device;

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
/// then by name). A non-empty `subset` limits it to those names that exist.
pub fn cycle_candidates(devices: &[Device], subset: &[String]) -> Vec<String> {
    let mut chosen: Vec<&Device> = devices
        .iter()
        .filter(|d| subset.is_empty() || subset.contains(&d.name))
        .collect();
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
        let subset = vec!["gone".to_string(), "alsa_output.pci-0000_2f_00.4.iec958-stereo".to_string()];
        assert_eq!(cycle_candidates(&s.sinks, &subset), vec![subset[1].clone()]);
    }
}
```

- [ ] **Step 6: Register modules**

`src/audio/mod.rs`:
```rust
pub mod gesture;
pub mod model;
pub mod ops;
pub mod settings;
pub mod target;
```

`src/main.rs` (next to `mod audio;`):
```rust
#[allow(dead_code)]
mod cycle;
#[allow(dead_code)]
mod lenient;
```

- [ ] **Step 7: Run and verify**

Run: `cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test`
Expected: PASS (all prior + 4 cycle + 4 settings + 10 gesture + 4 ops).

- [ ] **Step 8: Commit**

```bash
git add -A
git commit -m "feat(audio): add settings, gesture mapping, volume math and cycling

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 4: pactl backend and live-change subscriber

**Files:**
- Create: `src/audio/backend.rs`
- Modify: `src/audio/mod.rs` (add `pub mod backend;`)

**Interfaces:**
- Consumes: `build_snapshot`, `ParseError`, `DeviceKind`, `Snapshot` (Task 2).
- Produces:
  - `enum Node { Sink(String), Source(String), SinkInput(u32) }`, `enum Mute { On, Off, Toggle }`
  - `enum BackendError { NotInstalled, Failed { args, stderr }, Timeout, Parse(ParseError), NoTarget, UnsafeName(String), Io(std::io::Error) }`
  - `#[async_trait] trait AudioBackend: Send + Sync { snapshot, set_volume(&Node, u16), set_mute(&Node, Mute), set_default(DeviceKind, &str), move_stream(u32, &str) }` — all `-> Result<_, BackendError>`
  - `struct PactlBackend;`
  - `fn spawn_subscriber(tx: watch::Sender<u64>)`
  - pure: `is_safe_name`, `volume_args`, `mute_args`, `default_args`, `move_args`, `is_relevant_event`, `next_backoff`

- [ ] **Step 1: Write the failing tests** (bottom of new `src/audio/backend.rs`)

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn v(args: &[&str]) -> Vec<String> {
        args.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn volume_args_per_node() {
        assert_eq!(volume_args(&Node::Sink("s".into()), 55), v(&["set-sink-volume", "s", "55%"]));
        assert_eq!(volume_args(&Node::Source("m".into()), 0), v(&["set-source-volume", "m", "0%"]));
        assert_eq!(volume_args(&Node::SinkInput(7), 120), v(&["set-sink-input-volume", "7", "120%"]));
    }

    #[test]
    fn mute_args_per_mode() {
        assert_eq!(mute_args(&Node::Sink("s".into()), Mute::Toggle), v(&["set-sink-mute", "s", "toggle"]));
        assert_eq!(mute_args(&Node::Source("m".into()), Mute::On), v(&["set-source-mute", "m", "1"]));
        assert_eq!(mute_args(&Node::SinkInput(3), Mute::Off), v(&["set-sink-input-mute", "3", "0"]));
    }

    #[test]
    fn default_and_move_args() {
        assert_eq!(default_args(DeviceKind::Output, "s"), v(&["set-default-sink", "s"]));
        assert_eq!(default_args(DeviceKind::Input, "m"), v(&["set-default-source", "m"]));
        assert_eq!(move_args(9, "s"), v(&["move-sink-input", "9", "s"]));
    }

    #[test]
    fn names_starting_with_dash_are_refused() {
        assert!(!is_safe_name("-h"));
        assert!(!is_safe_name("--server=evil"));
        assert!(!is_safe_name(""));
        assert!(!is_safe_name("a\0b"));
        assert!(is_safe_name("alsa_output.usb-Razer-00.analog-stereo"));
    }

    #[tokio::test]
    async fn unsafe_names_never_reach_pactl() {
        let err = PactlBackend.set_default(DeviceKind::Output, "-h").await.unwrap_err();
        assert!(matches!(err, BackendError::UnsafeName(_)), "{err:?}");
        let err = PactlBackend.set_volume(&Node::Sink("--x".into()), 5).await.unwrap_err();
        assert!(matches!(err, BackendError::UnsafeName(_)), "{err:?}");
        let err = PactlBackend.move_stream(1, "-s").await.unwrap_err();
        assert!(matches!(err, BackendError::UnsafeName(_)), "{err:?}");
    }

    #[test]
    fn relevant_events() {
        assert!(is_relevant_event("Event 'change' on sink #60"));
        assert!(is_relevant_event("Event 'new' on sink-input #12"));
        assert!(is_relevant_event("Event 'change' on source #70"));
        assert!(is_relevant_event("Event 'change' on server #-1"));
        assert!(!is_relevant_event("Event 'new' on source-output #5"));
        assert!(!is_relevant_event("Event 'change' on client #90"));
        assert!(!is_relevant_event("garbage"));
    }

    #[test]
    fn backoff_doubles_to_a_cap() {
        assert_eq!(next_backoff(Duration::from_secs(1)), Duration::from_secs(2));
        assert_eq!(next_backoff(Duration::from_secs(16)), Duration::from_secs(30));
        assert_eq!(next_backoff(Duration::from_secs(30)), Duration::from_secs(30));
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test audio::backend`
Expected: FAIL to compile (`volume_args` etc. not defined) — after adding `pub mod backend;` to `src/audio/mod.rs`.

- [ ] **Step 3: Implement (top of `src/audio/backend.rs`)**

```rust
//! The only code that runs `pactl`. Commands are spawned directly (never
//! through a shell), and every name argument is checked by `is_safe_name`
//! first: pactl parses options anywhere on its command line, so a
//! free-text name like `--server=x` would otherwise be taken as an option.

use super::model::{self, DeviceKind, ParseError, Snapshot};
use async_trait::async_trait;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::watch;

const PACTL: &str = "pactl";
const COMMAND_TIMEOUT: Duration = Duration::from_secs(3);
const MAX_BACKOFF: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Node {
    Sink(String),
    Source(String),
    SinkInput(u32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mute {
    On,
    Off,
    Toggle,
}

#[derive(Debug, thiserror::Error)]
pub enum BackendError {
    #[error("pactl not found")]
    NotInstalled,
    #[error("pactl {args} failed: {stderr}")]
    Failed { args: String, stderr: String },
    #[error("pactl timed out")]
    Timeout,
    #[error(transparent)]
    Parse(#[from] ParseError),
    #[error("nothing to act on")]
    NoTarget,
    #[error("refusing unsafe name {0:?}")]
    UnsafeName(String),
    #[error("io: {0}")]
    Io(std::io::Error),
}

#[async_trait]
pub trait AudioBackend: Send + Sync {
    async fn snapshot(&self) -> Result<Snapshot, BackendError>;
    async fn set_volume(&self, node: &Node, percent: u16) -> Result<(), BackendError>;
    async fn set_mute(&self, node: &Node, mute: Mute) -> Result<(), BackendError>;
    async fn set_default(&self, kind: DeviceKind, name: &str) -> Result<(), BackendError>;
    async fn move_stream(&self, stream: u32, sink: &str) -> Result<(), BackendError>;
}

pub fn is_safe_name(name: &str) -> bool {
    !name.is_empty() && !name.starts_with('-') && !name.contains('\0')
}

fn check(name: &str) -> Result<(), BackendError> {
    if is_safe_name(name) {
        Ok(())
    } else {
        Err(BackendError::UnsafeName(name.to_string()))
    }
}

fn check_node(node: &Node) -> Result<(), BackendError> {
    match node {
        Node::Sink(n) | Node::Source(n) => check(n),
        Node::SinkInput(_) => Ok(()),
    }
}

fn node_parts<'a>(node: &Node, sink: &'a str, source: &'a str, input: &'a str) -> (&'a str, String) {
    match node {
        Node::Sink(n) => (sink, n.clone()),
        Node::Source(n) => (source, n.clone()),
        Node::SinkInput(i) => (input, i.to_string()),
    }
}

pub fn volume_args(node: &Node, percent: u16) -> Vec<String> {
    let (cmd, target) = node_parts(node, "set-sink-volume", "set-source-volume", "set-sink-input-volume");
    vec![cmd.to_string(), target, format!("{percent}%")]
}

pub fn mute_args(node: &Node, mute: Mute) -> Vec<String> {
    let (cmd, target) = node_parts(node, "set-sink-mute", "set-source-mute", "set-sink-input-mute");
    let value = match mute {
        Mute::On => "1",
        Mute::Off => "0",
        Mute::Toggle => "toggle",
    };
    vec![cmd.to_string(), target, value.to_string()]
}

pub fn default_args(kind: DeviceKind, name: &str) -> Vec<String> {
    let cmd = match kind {
        DeviceKind::Output => "set-default-sink",
        DeviceKind::Input => "set-default-source",
    };
    vec![cmd.to_string(), name.to_string()]
}

pub fn move_args(stream: u32, sink: &str) -> Vec<String> {
    vec!["move-sink-input".to_string(), stream.to_string(), sink.to_string()]
}

fn spawn_error(e: std::io::Error) -> BackendError {
    if e.kind() == std::io::ErrorKind::NotFound {
        BackendError::NotInstalled
    } else {
        BackendError::Io(e)
    }
}

pub struct PactlBackend;

impl PactlBackend {
    async fn run(args: &[String]) -> Result<String, BackendError> {
        let output = tokio::time::timeout(
            COMMAND_TIMEOUT,
            Command::new(PACTL).args(args).kill_on_drop(true).output(),
        )
        .await
        .map_err(|_| BackendError::Timeout)?
        .map_err(spawn_error)?;
        if !output.status.success() {
            return Err(BackendError::Failed {
                args: args.join(" "),
                stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
            });
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }

    async fn json(what: &[&str]) -> Result<String, BackendError> {
        let mut args = vec!["-f".to_string(), "json".to_string()];
        args.extend(what.iter().map(|s| s.to_string()));
        Self::run(&args).await
    }
}

#[async_trait]
impl AudioBackend for PactlBackend {
    async fn snapshot(&self) -> Result<Snapshot, BackendError> {
        let (info, sinks, sources, inputs) = tokio::try_join!(
            Self::json(&["info"]),
            Self::json(&["list", "sinks"]),
            Self::json(&["list", "sources"]),
            Self::json(&["list", "sink-inputs"]),
        )?;
        Ok(model::build_snapshot(&info, &sinks, &sources, &inputs)?)
    }

    async fn set_volume(&self, node: &Node, percent: u16) -> Result<(), BackendError> {
        check_node(node)?;
        Self::run(&volume_args(node, percent)).await.map(|_| ())
    }

    async fn set_mute(&self, node: &Node, mute: Mute) -> Result<(), BackendError> {
        check_node(node)?;
        Self::run(&mute_args(node, mute)).await.map(|_| ())
    }

    async fn set_default(&self, kind: DeviceKind, name: &str) -> Result<(), BackendError> {
        check(name)?;
        Self::run(&default_args(kind, name)).await.map(|_| ())
    }

    async fn move_stream(&self, stream: u32, sink: &str) -> Result<(), BackendError> {
        check(sink)?;
        Self::run(&move_args(stream, sink)).await.map(|_| ())
    }
}

/// `pactl subscribe` prints e.g. `Event 'change' on sink #60`. Only
/// devices, app streams and the server (default-device changes) matter;
/// clients and source-outputs (apps opening the mic) are noise.
pub fn is_relevant_event(line: &str) -> bool {
    let Some((_, rest)) = line.split_once(" on ") else {
        return false;
    };
    let facility = rest.split(" #").next().unwrap_or_default().trim();
    matches!(facility, "sink" | "source" | "sink-input" | "server")
}

pub fn next_backoff(current: Duration) -> Duration {
    (current * 2).min(MAX_BACKOFF)
}

fn bump(tx: &watch::Sender<u64>) {
    tx.send_modify(|n| *n = n.wrapping_add(1));
}

/// Runs one `pactl subscribe` until it exits. Returns whether it printed
/// anything (a healthy run resets the restart backoff).
async fn run_subscribe(tx: &watch::Sender<u64>) -> Result<bool, BackendError> {
    let mut child = Command::new(PACTL)
        .arg("subscribe")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(spawn_error)?;
    let stdout = child.stdout.take().ok_or(BackendError::NoTarget)?;
    let mut lines = BufReader::new(stdout).lines();
    let mut saw_output = false;
    while let Some(line) = lines.next_line().await.map_err(BackendError::Io)? {
        saw_output = true;
        if is_relevant_event(&line) {
            bump(tx);
        }
    }
    let _ = child.wait().await;
    Ok(saw_output)
}

/// Keeps one `pactl subscribe` alive forever, bumping `tx` on every
/// relevant change and once after each (re)start so listeners resync.
pub fn spawn_subscriber(tx: watch::Sender<u64>) {
    tokio::spawn(async move {
        let mut backoff = Duration::from_secs(1);
        loop {
            match run_subscribe(&tx).await {
                Ok(true) => backoff = Duration::from_secs(1),
                Ok(false) => log::warn!("pactl subscribe exited without output"),
                Err(e) => log::warn!("pactl subscribe failed: {e}"),
            }
            bump(&tx);
            tokio::time::sleep(backoff).await;
            backoff = next_backoff(backoff);
        }
    });
}
```

- [ ] **Step 4: Run and verify**

Run: `cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test`
Expected: PASS (7 new backend tests).

- [ ] **Step 5: Smoke-test against the real system** (not automated; confirms the JSON commands exist on this pactl)

Run: `pactl -f json info >/dev/null && pactl -f json list sink-inputs >/dev/null && echo ok`
Expected: `ok`.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat(audio): add the pactl backend and live change subscriber

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 5: Operation executor

**Files:**
- Create: `src/audio/exec.rs`, `src/audio/fake.rs`
- Modify: `src/audio/mod.rs` (add `pub mod exec;` and `#[cfg(test)] pub mod fake;`)

**Interfaces:**
- Consumes: `AudioBackend`, `Node`, `Mute`, `BackendError` (Task 4); `Resolved` (Task 2); `AudioSettings`, `Operation`, `adjust_volume`, `cycle_candidates` (Task 3); `cycle::step_in`.
- Produces:
  - `async fn execute(backend: &dyn AudioBackend, snap: &Snapshot, resolved: &Resolved, settings: &AudioSettings, op: &Operation) -> Result<(), BackendError>`
  - `#[cfg(test)] struct FakeBackend { calls: Mutex<Vec<String>>, snapshot: Snapshot }` implementing `AudioBackend`; `FakeBackend::new(Snapshot)`, `FakeBackend::calls() -> Vec<String>`

- [ ] **Step 1: Write `src/audio/fake.rs`**

```rust
use super::backend::{AudioBackend, BackendError, Mute, Node};
use super::model::{DeviceKind, Snapshot};
use async_trait::async_trait;
use std::sync::Mutex;

/// Records every call as a readable string; `snapshot` returns a fixed value.
#[derive(Default)]
pub struct FakeBackend {
    pub calls: Mutex<Vec<String>>,
    pub snapshot: Snapshot,
}

impl FakeBackend {
    pub fn new(snapshot: Snapshot) -> Self {
        Self { calls: Mutex::new(Vec::new()), snapshot }
    }

    pub fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }

    fn record(&self, call: String) {
        self.calls.lock().unwrap().push(call);
    }
}

#[async_trait]
impl AudioBackend for FakeBackend {
    async fn snapshot(&self) -> Result<Snapshot, BackendError> {
        Ok(self.snapshot.clone())
    }
    async fn set_volume(&self, node: &Node, percent: u16) -> Result<(), BackendError> {
        self.record(format!("volume {node:?} {percent}"));
        Ok(())
    }
    async fn set_mute(&self, node: &Node, mute: Mute) -> Result<(), BackendError> {
        self.record(format!("mute {node:?} {mute:?}"));
        Ok(())
    }
    async fn set_default(&self, kind: DeviceKind, name: &str) -> Result<(), BackendError> {
        self.record(format!("default {kind:?} {name}"));
        Ok(())
    }
    async fn move_stream(&self, stream: u32, sink: &str) -> Result<(), BackendError> {
        self.record(format!("move {stream} {sink}"));
        Ok(())
    }
}
```

- [ ] **Step 2: Write the failing tests** (bottom of `src/audio/exec.rs`)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::fake::FakeBackend;
    use crate::audio::model::test_support::fixture_snapshot;
    use crate::audio::target::{TargetKind, resolve};

    const RAZER: &str = "alsa_output.usb-Razer_Razer_Leviathan_V2-00.analog-stereo";
    const HDMI: &str = "alsa_output.pci-0000_2f_00.4.iec958-stereo";

    async fn run(kind: TargetKind, name: &str, settings: AudioSettings, op: Operation) -> (Result<(), BackendError>, Vec<String>) {
        let snap = fixture_snapshot();
        let fake = FakeBackend::new(snap.clone());
        let resolved = resolve(kind, name, &snap);
        let result = execute(&fake, &snap, &resolved, &settings, &op).await;
        (result, fake.calls())
    }

    #[tokio::test]
    async fn device_volume_adjusts_from_current_level() {
        let (r, calls) = run(TargetKind::DefaultOutput, "", AudioSettings::default(), Operation::AdjustVolume(10)).await;
        r.unwrap();
        assert_eq!(calls, vec![format!("volume Sink({RAZER:?}) 57")]);
    }

    #[tokio::test]
    async fn set_volume_is_capped_at_max() {
        let (_, calls) = run(TargetKind::DefaultInput, "", AudioSettings::default(), Operation::SetVolume(140)).await;
        assert_eq!(calls, vec![r#"volume Source("alsa_input.usb-webcam-02.mono-fallback") 100"#.to_string()]);
    }

    #[tokio::test]
    async fn toggle_mute_on_a_device() {
        let (_, calls) = run(TargetKind::DefaultOutput, "", AudioSettings::default(), Operation::ToggleMute).await;
        assert_eq!(calls, vec![format!("mute Sink({RAZER:?}) Toggle")]);
    }

    #[tokio::test]
    async fn cycle_device_moves_the_default_to_the_next_sink() {
        let (_, calls) = run(TargetKind::DefaultOutput, "", AudioSettings::default(), Operation::CycleDevice(1)).await;
        assert_eq!(calls, vec![format!("default Output {HDMI}")]);
    }

    #[tokio::test]
    async fn cycling_a_single_device_list_does_nothing() {
        let settings = AudioSettings { cycle_devices: vec![RAZER.to_string()], ..AudioSettings::default() };
        let (r, calls) = run(TargetKind::DefaultOutput, "", settings, Operation::CycleDevice(1)).await;
        r.unwrap();
        assert!(calls.is_empty());
    }

    #[tokio::test]
    async fn set_default_device_must_exist_in_that_kind() {
        let (r, calls) = run(TargetKind::DefaultOutput, "", AudioSettings::default(), Operation::SetDefaultDevice("alsa_input.usb-webcam-02.mono-fallback".into())).await;
        assert!(matches!(r, Err(BackendError::NoTarget)));
        assert!(calls.is_empty());
    }

    #[tokio::test]
    async fn app_mute_sets_every_stream_to_the_same_state() {
        let (_, calls) = run(TargetKind::App, "chrome", AudioSettings::default(), Operation::ToggleMute).await;
        assert_eq!(calls, vec!["mute SinkInput(100) On".to_string(), "mute SinkInput(101) On".to_string()]);
    }

    #[tokio::test]
    async fn app_volume_follows_the_first_stream() {
        let (_, calls) = run(TargetKind::App, "chrome", AudioSettings::default(), Operation::AdjustVolume(-10)).await;
        assert_eq!(calls, vec!["volume SinkInput(100) 90".to_string(), "volume SinkInput(101) 90".to_string()]);
    }

    #[tokio::test]
    async fn app_cycle_moves_its_streams_to_the_next_sink() {
        let (_, calls) = run(TargetKind::App, "chrome", AudioSettings::default(), Operation::CycleDevice(1)).await;
        assert_eq!(calls, vec![format!("move 100 {HDMI}"), format!("move 101 {HDMI}")]);
    }

    #[tokio::test]
    async fn unavailable_target_is_an_error_and_touches_nothing() {
        let (r, calls) = run(TargetKind::App, "firefox", AudioSettings::default(), Operation::ToggleMute).await;
        assert!(matches!(r, Err(BackendError::NoTarget)));
        assert!(calls.is_empty());
    }

    #[tokio::test]
    async fn none_is_a_no_op_even_without_a_target() {
        let (r, calls) = run(TargetKind::App, "firefox", AudioSettings::default(), Operation::None).await;
        r.unwrap();
        assert!(calls.is_empty());
    }
}
```

- [ ] **Step 3: Run to verify failure**

Run: `cargo test audio::exec`
Expected: FAIL to compile (`execute` not defined).

- [ ] **Step 4: Implement (top of `src/audio/exec.rs`)**

```rust
//! Carries out one `Operation` on a resolved target through a backend.

use super::backend::{AudioBackend, BackendError, Mute, Node};
use super::gesture::Operation;
use super::model::{DeviceKind, Snapshot, Stream};
use super::ops::{adjust_volume, cycle_candidates};
use super::settings::AudioSettings;
use super::target::Resolved;
use crate::cycle::step_in;

pub async fn execute(
    backend: &dyn AudioBackend,
    snap: &Snapshot,
    resolved: &Resolved,
    settings: &AudioSettings,
    op: &Operation,
) -> Result<(), BackendError> {
    if *op == Operation::None {
        return Ok(());
    }
    match resolved {
        Resolved::Device { kind, device } => {
            let node = match kind {
                DeviceKind::Output => Node::Sink(device.name.clone()),
                DeviceKind::Input => Node::Source(device.name.clone()),
            };
            execute_on_device(backend, snap, *kind, &node, device.volume, settings, op).await
        }
        Resolved::App { streams } => execute_on_app(backend, snap, streams, settings, op).await,
        Resolved::Unavailable { .. } | Resolved::NotPlaying { .. } => Err(BackendError::NoTarget),
    }
}

async fn execute_on_device(
    backend: &dyn AudioBackend,
    snap: &Snapshot,
    kind: DeviceKind,
    node: &Node,
    volume: u16,
    settings: &AudioSettings,
    op: &Operation,
) -> Result<(), BackendError> {
    let max = settings.max_volume();
    match op {
        Operation::None => Ok(()),
        Operation::ToggleMute => backend.set_mute(node, Mute::Toggle).await,
        Operation::SetMute(m) => backend.set_mute(node, if *m { Mute::On } else { Mute::Off }).await,
        Operation::AdjustVolume(delta) => backend.set_volume(node, adjust_volume(volume, *delta, max)).await,
        Operation::SetVolume(v) => backend.set_volume(node, (*v).min(max)).await,
        Operation::CycleDevice(dir) => {
            let current = snap.default_name(kind);
            let list = cycle_candidates(snap.devices(kind), &settings.cycle_devices);
            match step_in(&list, current, i64::from(*dir)) {
                Some(next) if next != current => backend.set_default(kind, &next).await,
                Some(_) => Ok(()),
                None => Err(BackendError::NoTarget),
            }
        }
        Operation::SetDefaultDevice(name) => {
            if !snap.devices(kind).iter().any(|d| &d.name == name) {
                return Err(BackendError::NoTarget);
            }
            backend.set_default(kind, name).await
        }
    }
}

async fn execute_on_app(
    backend: &dyn AudioBackend,
    snap: &Snapshot,
    streams: &[Stream],
    settings: &AudioSettings,
    op: &Operation,
) -> Result<(), BackendError> {
    let Some(first) = streams.first() else {
        return Err(BackendError::NoTarget);
    };
    let max = settings.max_volume();
    match op {
        Operation::None => Ok(()),
        // Streams are set to one explicit state rather than each toggled,
        // so an app whose streams disagree ends up consistent.
        Operation::ToggleMute => set_all_mute(backend, streams, !first.muted).await,
        Operation::SetMute(m) => set_all_mute(backend, streams, *m).await,
        Operation::AdjustVolume(delta) => {
            set_all_volume(backend, streams, adjust_volume(first.volume, *delta, max)).await
        }
        Operation::SetVolume(v) => set_all_volume(backend, streams, (*v).min(max)).await,
        Operation::CycleDevice(dir) => {
            let current = snap.sink_by_index(first.sink).map(|d| d.name.as_str()).unwrap_or_default();
            let list = cycle_candidates(&snap.sinks, &settings.cycle_devices);
            match step_in(&list, current, i64::from(*dir)) {
                Some(next) if next != current => move_all(backend, streams, &next).await,
                Some(_) => Ok(()),
                None => Err(BackendError::NoTarget),
            }
        }
        Operation::SetDefaultDevice(name) => {
            if !snap.sinks.iter().any(|d| &d.name == name) {
                return Err(BackendError::NoTarget);
            }
            move_all(backend, streams, name).await
        }
    }
}

async fn set_all_mute(backend: &dyn AudioBackend, streams: &[Stream], muted: bool) -> Result<(), BackendError> {
    let mute = if muted { Mute::On } else { Mute::Off };
    for s in streams {
        backend.set_mute(&Node::SinkInput(s.index), mute).await?;
    }
    Ok(())
}

async fn set_all_volume(backend: &dyn AudioBackend, streams: &[Stream], percent: u16) -> Result<(), BackendError> {
    for s in streams {
        backend.set_volume(&Node::SinkInput(s.index), percent).await?;
    }
    Ok(())
}

async fn move_all(backend: &dyn AudioBackend, streams: &[Stream], sink: &str) -> Result<(), BackendError> {
    for s in streams {
        backend.move_stream(s.index, sink).await?;
    }
    Ok(())
}
```

- [ ] **Step 5: Run and verify**

Run: `cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test`
Expected: PASS (11 new exec tests).

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat(audio): execute gesture operations on devices and app streams

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 6: Rendering — tiles, icons, level strip and the audio view

**Files:**
- Create: `src/render/mod.rs`, `src/render/tile.rs`, `src/render/text.rs`, `src/render/icons.rs`, `src/render/level.rs`, `src/audio/view.rs`, `assets/layouts/level.json`
- Modify: `src/audio/mod.rs` (add `pub mod view;`), `src/main.rs` (add `#[allow(dead_code)] mod render;` — removed in Task 7)

**Interfaces:**
- Consumes: `Resolved`, `DeviceKind`, `TargetKind`, `AudioSettings`, `DEFAULT_ACCENT`.
- Produces:
  - `render::tile::{card() -> String, text_line(y, size, bold, color, &str) -> String, data_uri(&str) -> String, escape_xml(&str) -> String, TEXT_COLOR, MUTED_TEXT_COLOR}`
  - `render::text::{shorten(&str, max_chars: usize) -> String, sanitize_color(&str, fallback: &str) -> String}`
  - `render::icons::{Icon { Speaker, Mic, Sun, Letter(char) }, icon_svg(&Icon, color, struck) -> String, icon_data_uri(&Icon, color, struck) -> String}`
  - `render::level::{LevelView { title, value_text, bar: f64, color, icon, struck }, strip_feedback(&LevelView) -> serde_json::Value, tile_image(&LevelView) -> String, MUTED_COLOR, INACTIVE_COLOR}`
  - `render::show_level(&Instance, &LevelView) -> OpenActionResult<()>`
  - `audio::view::{audio_view(&Resolved, &AudioSettings) -> LevelView, error_view(&AudioSettings, message: &str) -> LevelView}`

- [ ] **Step 1: Write `src/render/tile.rs`** (ported from `opendeck-claude-usage/src/tile.rs`)

```rust
//! Shared pieces for generated key images: the dark card, text colours and
//! text drawn *inside* the SVG. Text is baked into the image rather than
//! sent as the key's native title so size, weight and contrast stay under
//! our control whatever title settings the key has.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;

pub const CARD_COLOR: &str = "#111827";
pub const TEXT_COLOR: &str = "#f9fafb";
pub const MUTED_TEXT_COLOR: &str = "#d1d5db";

const MAX_TEXT_WIDTH: f64 = 94.0;
const REGULAR_CHAR_WIDTH: f64 = 0.58;
const BOLD_CHAR_WIDTH: f64 = 0.64;

pub fn card() -> String {
    format!(r#"<rect x="0" y="0" width="100" height="100" fill="{CARD_COLOR}" />"#)
}

/// One horizontally-centred line with its baseline at `y`; lines estimated
/// wider than the key are squeezed with `textLength` rather than clipped.
pub fn text_line(y: f64, size: f64, bold: bool, color: &str, content: &str) -> String {
    let char_width = if bold { BOLD_CHAR_WIDTH } else { REGULAR_CHAR_WIDTH };
    let estimated_width = content.chars().count() as f64 * size * char_width;
    let fit = if estimated_width > MAX_TEXT_WIDTH {
        format!(r#" textLength="{MAX_TEXT_WIDTH}" lengthAdjust="spacingAndGlyphs""#)
    } else {
        String::new()
    };
    let weight = if bold { "700" } else { "500" };
    let escaped = escape_xml(content);
    format!(
        r#"<text x="50" y="{y}" text-anchor="middle" font-family="sans-serif" font-size="{size}" font-weight="{weight}" fill="{color}"{fit}>{escaped}</text>"#
    )
}

pub fn escape_xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// `setImage` only treats `image` as inline data when it starts with `data:`.
pub fn data_uri(svg: &str) -> String {
    format!("data:image/svg+xml;base64,{}", STANDARD.encode(svg.as_bytes()))
}

#[cfg(test)]
pub(crate) fn decode(uri: &str) -> String {
    let b64 = uri.strip_prefix("data:image/svg+xml;base64,").unwrap();
    String::from_utf8(STANDARD.decode(b64).unwrap()).unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_lines_are_not_squeezed() {
        assert!(!text_line(80.0, 14.0, false, TEXT_COLOR, "45%").contains("textLength"));
    }

    #[test]
    fn long_lines_are_squeezed() {
        assert!(text_line(80.0, 14.0, false, TEXT_COLOR, "a very long device label").contains(r#"textLength="94""#));
    }

    #[test]
    fn text_is_escaped() {
        let line = text_line(80.0, 14.0, false, TEXT_COLOR, r#"<b> & "q""#);
        assert!(line.contains("&lt;b&gt; &amp; &quot;q&quot;"), "{line}");
    }

    #[test]
    fn data_uri_round_trips() {
        assert_eq!(decode(&data_uri("<svg/>")), "<svg/>");
    }
}
```

- [ ] **Step 2: Write `src/render/text.rs` with the Review Focus #1 tests**

```rust
/// Truncates to `max_chars` characters, ending in `…` when cut.
pub fn shorten(s: &str, max_chars: usize) -> String {
    let s = s.trim();
    if s.chars().count() <= max_chars || max_chars == 0 {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max_chars - 1).collect();
    out.push('…');
    out
}

/// Only `#rgb` / `#rrggbb` pass - anything else (it is user text that ends
/// up in SVG attributes and layout colours) becomes `fallback`.
pub fn sanitize_color(value: &str, fallback: &str) -> String {
    let v = value.trim();
    let hex = v.strip_prefix('#').unwrap_or("");
    if matches!(hex.len(), 3 | 6) && hex.chars().all(|c| c.is_ascii_hexdigit()) {
        v.to_string()
    } else {
        fallback.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shorten_keeps_short_and_cuts_long() {
        assert_eq!(shorten("Razer", 10), "Razer");
        assert_eq!(shorten("Razer Leviathan V2", 10), "Razer Lev…");
    }

    #[test]
    fn sanitize_color_accepts_hex() {
        assert_eq!(sanitize_color("#4fc3f7", "#000"), "#4fc3f7");
        assert_eq!(sanitize_color("#ABC", "#000"), "#ABC");
    }

    #[test]
    fn sanitize_color_rejects_everything_else() {
        for bad in [r#"red" onload="x"#, "url(#a)", "#12345", "#ggg", "", "4fc3f7", "#4fc3f7;x"] {
            assert_eq!(sanitize_color(bad, "#000"), "#000", "{bad}");
        }
    }
}
```

- [ ] **Step 3: Write `src/render/icons.rs`**

```rust
//! Small glyphs drawn in a 100x100 box: used inside key tiles and, wrapped
//! in their own `<svg>`, as the touch-strip `icon` pixmap.

use super::tile::{data_uri, escape_xml};

pub const STRIKE_COLOR: &str = "#ef4444";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Icon {
    Speaker,
    Mic,
    Sun,
    Letter(char),
}

pub fn icon_svg(icon: &Icon, color: &str, struck: bool) -> String {
    let body = match icon {
        Icon::Speaker => format!(
            r#"<path d="M14 38 H32 L54 18 V82 L32 62 H14 Z" fill="{color}"/><path d="M64 34 Q76 50 64 66" fill="none" stroke="{color}" stroke-width="7" stroke-linecap="round"/><path d="M74 22 Q94 50 74 78" fill="none" stroke="{color}" stroke-width="7" stroke-linecap="round"/>"#
        ),
        Icon::Mic => format!(
            r#"<rect x="37" y="10" width="26" height="48" rx="13" fill="{color}"/><path d="M26 44 Q26 72 50 72 Q74 72 74 44" fill="none" stroke="{color}" stroke-width="7" stroke-linecap="round"/><path d="M50 72 V88 M36 90 H64" stroke="{color}" stroke-width="7" stroke-linecap="round"/>"#
        ),
        Icon::Sun => {
            let rays: String = (0..8)
                .map(|i| {
                    let a = f64::from(i) * std::f64::consts::FRAC_PI_4;
                    let (x1, y1) = (50.0 + 26.0 * a.cos(), 50.0 + 26.0 * a.sin());
                    let (x2, y2) = (50.0 + 40.0 * a.cos(), 50.0 + 40.0 * a.sin());
                    format!(r#"<line x1="{x1:.1}" y1="{y1:.1}" x2="{x2:.1}" y2="{y2:.1}" stroke="{color}" stroke-width="7" stroke-linecap="round"/>"#)
                })
                .collect();
            format!(r#"<circle cx="50" cy="50" r="17" fill="{color}"/>{rays}"#)
        }
        Icon::Letter(c) => format!(
            r#"<circle cx="50" cy="50" r="42" fill="none" stroke="{color}" stroke-width="7"/><text x="50" y="66" text-anchor="middle" font-family="sans-serif" font-size="46" font-weight="700" fill="{color}">{}</text>"#,
            escape_xml(&c.to_string())
        ),
    };
    let strike = if struck {
        format!(r#"<line x1="14" y1="86" x2="86" y2="14" stroke="{STRIKE_COLOR}" stroke-width="9" stroke-linecap="round"/>"#)
    } else {
        String::new()
    };
    format!("{body}{strike}")
}

pub fn icon_data_uri(icon: &Icon, color: &str, struck: bool) -> String {
    data_uri(&format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100">{}</svg>"#,
        icon_svg(icon, color, struck)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn struck_icons_carry_the_strike_line() {
        assert!(icon_svg(&Icon::Speaker, "#fff", true).contains(STRIKE_COLOR));
        assert!(!icon_svg(&Icon::Speaker, "#fff", false).contains(STRIKE_COLOR));
    }

    #[test]
    fn letter_is_escaped() {
        assert!(icon_svg(&Icon::Letter('<'), "#fff", false).contains("&lt;"));
    }

    #[test]
    fn data_uri_wraps_a_standalone_svg() {
        let svg = super::super::tile::decode(&icon_data_uri(&Icon::Mic, "#fff", false));
        assert!(svg.starts_with("<svg xmlns="));
    }
}
```

- [ ] **Step 4: Write `assets/layouts/level.json`**

```json
{
	"$schema": "https://schemas.elgato.com/streamdeck/plugins/layout.json",
	"id": "com.jfms7s.utilities.level-layout",
	"items": [
		{ "key": "icon", "type": "pixmap", "rect": [8, 28, 44, 44] },
		{
			"key": "title",
			"type": "text",
			"rect": [60, 6, 136, 22],
			"alignment": "left",
			"color": "#d1d5db",
			"value": "",
			"font": { "size": 14, "weight": 500 }
		},
		{
			"key": "value",
			"type": "text",
			"rect": [60, 28, 136, 34],
			"alignment": "left",
			"color": "white",
			"value": "--",
			"font": { "size": 26, "weight": 700 }
		},
		{
			"key": "bar",
			"type": "bar",
			"rect": [60, 70, 130, 14],
			"value": 0,
			"bar_bg_c": "#374151",
			"bar_fill_c": "#4fc3f7",
			"bar_border_c": "#111827",
			"border_w": 1,
			"subtype": 0
		}
	]
}
```

- [ ] **Step 5: Write `src/render/level.rs` with tests**

```rust
//! One renderer for any "level" control (audio, brightness): a touch-strip
//! feedback payload for dials and a generated image for keys.

use super::icons::{Icon, icon_data_uri, icon_svg};
use super::text::sanitize_color;
use super::tile::{self, MUTED_TEXT_COLOR, TEXT_COLOR};
use serde_json::{Value, json};

pub const MUTED_COLOR: &str = "#ef4444";
pub const INACTIVE_COLOR: &str = "#6b7280";
const TRACK_COLOR: &str = "#374151";

#[derive(Debug, Clone, PartialEq)]
pub struct LevelView {
    pub title: String,
    pub value_text: String,
    /// 0-100, the fill of the bar.
    pub bar: f64,
    pub color: String,
    pub icon: Icon,
    pub struck: bool,
}

impl LevelView {
    fn safe_color(&self) -> String {
        sanitize_color(&self.color, INACTIVE_COLOR)
    }
}

pub fn strip_feedback(view: &LevelView) -> Value {
    let color = view.safe_color();
    json!({
        "icon": icon_data_uri(&view.icon, &color, view.struck),
        "title": view.title,
        "value": view.value_text,
        "bar": { "value": view.bar.clamp(0.0, 100.0), "bar_fill_c": color },
    })
}

pub fn tile_image(view: &LevelView) -> String {
    let color = view.safe_color();
    let bar_width = (view.bar.clamp(0.0, 100.0) * 0.8).round();
    let svg = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100">{card}<g transform="translate(32 4) scale(0.36)">{icon}</g>{value}{title}<rect x="10" y="90" width="80" height="5" rx="2.5" fill="{TRACK_COLOR}"/><rect x="10" y="90" width="{bar_width}" height="5" rx="2.5" fill="{color}"/></svg>"#,
        card = tile::card(),
        icon = icon_svg(&view.icon, &color, view.struck),
        value = tile::text_line(64.0, 24.0, true, TEXT_COLOR, &view.value_text),
        title = tile::text_line(82.0, 13.0, false, MUTED_TEXT_COLOR, &view.title),
    );
    tile::data_uri(&svg)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view() -> LevelView {
        LevelView {
            title: "Razer".into(),
            value_text: "45%".into(),
            bar: 45.0,
            color: "#4fc3f7".into(),
            icon: Icon::Speaker,
            struck: false,
        }
    }

    #[test]
    fn feedback_keys_match_the_shipped_layout() {
        let layout: Value = serde_json::from_str(include_str!("../../assets/layouts/level.json")).unwrap();
        let keys: Vec<&str> = layout["items"].as_array().unwrap().iter().map(|i| i["key"].as_str().unwrap()).collect();
        for k in strip_feedback(&view()).as_object().unwrap().keys() {
            assert!(keys.contains(&k.as_str()), "layout has no item keyed {k}");
        }
    }

    #[test]
    fn bar_is_clamped() {
        let v = LevelView { bar: 180.0, ..view() };
        assert_eq!(strip_feedback(&v)["bar"]["value"], 100.0);
    }

    #[test]
    fn an_unsafe_colour_never_reaches_the_output() {
        let v = LevelView { color: r#"x" onload="evil"#.into(), ..view() };
        assert!(!strip_feedback(&v).to_string().contains("evil"));
        assert!(!tile::decode(&tile_image(&v)).contains("evil"));
    }

    #[test]
    fn tile_shows_value_and_title() {
        let svg = tile::decode(&tile_image(&view()));
        assert!(svg.contains(">45%</text>") && svg.contains(">Razer</text>"), "{svg}");
    }
}
```

- [ ] **Step 6: Write `src/render/mod.rs`**

```rust
pub mod icons;
pub mod level;
pub mod text;
pub mod tile;

use level::{LevelView, strip_feedback, tile_image};
use openaction::{Instance, OpenActionResult};

pub const KEYPAD: &str = "Keypad";

/// Keys get a generated image (with the native title cleared so OpenDeck
/// doesn't paint a second copy); dials get touch-strip feedback.
pub async fn show_level(instance: &Instance, view: &LevelView) -> OpenActionResult<()> {
    if instance.controller == KEYPAD {
        instance.set_title(Some(String::new()), None).await?;
        instance.set_image(Some(tile_image(view)), None).await
    } else {
        instance.set_feedback(&strip_feedback(view)).await
    }
}
```

- [ ] **Step 7: Write `src/audio/view.rs` with tests**

```rust
//! What an Audio control shows for a resolved target.

use super::model::DeviceKind;
use super::settings::{AudioSettings, DEFAULT_ACCENT};
use super::target::{Resolved, TargetKind};
use crate::render::icons::Icon;
use crate::render::level::{INACTIVE_COLOR, LevelView, MUTED_COLOR};
use crate::render::text::{sanitize_color, shorten};

const TITLE_CHARS: usize = 20;

fn title(settings: &AudioSettings, fallback: &str) -> String {
    let label = settings.label.trim();
    shorten(if label.is_empty() { fallback } else { label }, TITLE_CHARS)
}

fn initial(label: &str) -> char {
    label
        .chars()
        .find(|c| c.is_alphanumeric())
        .map(|c| c.to_ascii_uppercase())
        .unwrap_or('?')
}

fn target_icon(settings: &AudioSettings, fallback_label: &str) -> Icon {
    match settings.target {
        TargetKind::DefaultInput | TargetKind::Input => Icon::Mic,
        TargetKind::App => Icon::Letter(initial(fallback_label)),
        TargetKind::DefaultOutput | TargetKind::Output => Icon::Speaker,
    }
}

fn level(settings: &AudioSettings, fallback: &str, volume: u16, muted: bool, icon: Icon) -> LevelView {
    let value_text = if muted {
        "muted".to_string()
    } else if settings.show_percent {
        format!("{volume}%")
    } else {
        String::new()
    };
    LevelView {
        title: title(settings, fallback),
        value_text,
        bar: (f64::from(volume) * 100.0 / f64::from(settings.max_volume())).clamp(0.0, 100.0),
        color: if muted {
            MUTED_COLOR.to_string()
        } else {
            sanitize_color(&settings.accent, DEFAULT_ACCENT)
        },
        icon,
        struck: muted,
    }
}

fn inactive(settings: &AudioSettings, label: &str, status: &str) -> LevelView {
    LevelView {
        title: title(settings, label),
        value_text: status.to_string(),
        bar: 0.0,
        color: INACTIVE_COLOR.to_string(),
        icon: target_icon(settings, label),
        struck: false,
    }
}

pub fn audio_view(resolved: &Resolved, settings: &AudioSettings) -> LevelView {
    let label = resolved.label();
    match resolved {
        Resolved::Device { kind, device } => {
            let icon = match kind {
                DeviceKind::Output => Icon::Speaker,
                DeviceKind::Input => Icon::Mic,
            };
            level(settings, &label, device.volume, device.muted, icon)
        }
        Resolved::App { streams } => match streams.first() {
            Some(first) => level(settings, &label, first.volume, first.muted, Icon::Letter(initial(&label))),
            None => inactive(settings, &label, "not playing"),
        },
        Resolved::Unavailable { .. } => inactive(settings, &label, "unavailable"),
        Resolved::NotPlaying { .. } => inactive(settings, &label, "not playing"),
    }
}

/// Shown when no snapshot could be read at all (e.g. `pactl` missing).
pub fn error_view(settings: &AudioSettings, message: &str) -> LevelView {
    LevelView {
        title: shorten(message, TITLE_CHARS),
        value_text: "—".to_string(),
        bar: 0.0,
        color: INACTIVE_COLOR.to_string(),
        icon: target_icon(settings, "?"),
        struck: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::model::test_support::fixture_snapshot;
    use crate::audio::target::resolve;

    fn view_for(settings: &AudioSettings) -> LevelView {
        audio_view(&resolve(settings.target, &settings.target_name, &fixture_snapshot()), settings)
    }

    #[test]
    fn device_shows_percent_in_accent() {
        let v = view_for(&AudioSettings::default());
        assert_eq!(v.value_text, "47%");
        assert_eq!(v.color, DEFAULT_ACCENT);
        assert_eq!(v.icon, Icon::Speaker);
        assert_eq!(v.title, "Razer Leviathan V2 …");
    }

    #[test]
    fn muted_mic_is_red_and_struck() {
        let v = view_for(&AudioSettings { target: TargetKind::DefaultInput, ..AudioSettings::default() });
        assert_eq!(v.value_text, "muted");
        assert_eq!(v.color, MUTED_COLOR);
        assert!(v.struck);
        assert_eq!(v.icon, Icon::Mic);
    }

    #[test]
    fn app_uses_its_initial_and_label_override() {
        let v = view_for(&AudioSettings {
            target: TargetKind::App,
            target_name: "chrome".into(),
            label: "Browser".into(),
            ..AudioSettings::default()
        });
        assert_eq!(v.icon, Icon::Letter('G'));
        assert_eq!(v.title, "Browser");
        assert_eq!(v.value_text, "100%");
    }

    #[test]
    fn not_playing_and_unavailable_are_grey() {
        let v = view_for(&AudioSettings { target: TargetKind::App, target_name: "firefox".into(), ..AudioSettings::default() });
        assert_eq!((v.value_text.as_str(), v.color.as_str()), ("not playing", INACTIVE_COLOR));
        let v = view_for(&AudioSettings { target: TargetKind::Output, target_name: "gone".into(), ..AudioSettings::default() });
        assert_eq!(v.value_text, "unavailable");
    }

    #[test]
    fn bar_is_relative_to_max_volume_and_percent_can_be_hidden() {
        let v = view_for(&AudioSettings { max_volume: 150, show_percent: false, ..AudioSettings::default() });
        assert!((v.bar - 47.0 * 100.0 / 150.0).abs() < 1e-9);
        assert_eq!(v.value_text, "");
    }

    #[test]
    fn bad_accent_falls_back() {
        let v = view_for(&AudioSettings { accent: "javascript:x".into(), ..AudioSettings::default() });
        assert_eq!(v.color, DEFAULT_ACCENT);
    }

    #[test]
    fn error_view_names_the_problem() {
        let v = error_view(&AudioSettings::default(), "pactl not found");
        assert_eq!((v.title.as_str(), v.value_text.as_str()), ("pactl not found", "—"));
    }
}
```

- [ ] **Step 8: Register modules** — `src/audio/mod.rs` add `pub mod view;`; `src/main.rs` add:

```rust
#[allow(dead_code)]
mod render;
```

- [ ] **Step 9: Run and verify**

Run: `cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test`
Expected: PASS.

- [ ] **Step 10: Commit**

```bash
git add -A
git commit -m "feat(render): draw level tiles and touch-strip feedback for audio

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 7: The Audio action, its property inspector and wiring

**Files:**
- Create: `src/actions/mod.rs`, `src/actions/audio.rs`, `src/audio/pi.rs`, `assets/propertyInspector/audio.html`
- Modify: `src/audio/mod.rs` (add `pub mod pi;`), `src/main.rs`, `assets/manifest.json`

**Interfaces:**
- Consumes: everything in `audio::*` and `render::*`.
- Produces:
  - `audio::pi::choices(&Snapshot, &AudioSettings, Controller) -> serde_json::Value` — `{"event":"audioChoices","controller","outputs":[{name,description}],"inputs":[…],"apps":[{match,app_name}],"effective":{press,long_press,touch_tap}}`
  - `actions::audio::{AudioAction, AudioAction::new(Arc<dyn AudioBackend>), AudioAction::run_watcher(self, watch::Receiver<u64>)}`; `Action::UUID = "com.jfms7s.utilities.audio"`

- [ ] **Step 1: Write `src/audio/pi.rs` with tests**

```rust
//! The data the Audio property inspector needs to offer choices.

use super::gesture::{Controller, effective};
use super::model::{Device, Snapshot};
use super::settings::AudioSettings;
use serde_json::{Value, json};
use std::collections::HashSet;

fn devices(list: &[Device]) -> Vec<Value> {
    list.iter()
        .map(|d| json!({ "name": d.name, "description": d.description }))
        .collect()
}

pub fn choices(snap: &Snapshot, settings: &AudioSettings, controller: Controller) -> Value {
    let mut seen = HashSet::new();
    let apps: Vec<Value> = snap
        .streams
        .iter()
        .filter_map(|s| {
            let key = if s.binary.is_empty() { &s.app_name } else { &s.binary };
            (!key.is_empty() && seen.insert(key.to_lowercase()))
                .then(|| json!({ "match": key, "app_name": s.app_name }))
        })
        .collect();
    let e = effective(settings, controller);
    json!({
        "event": "audioChoices",
        "controller": match controller { Controller::Keypad => "Keypad", Controller::Encoder => "Encoder" },
        "outputs": devices(&snap.sinks),
        "inputs": devices(&snap.sources),
        "apps": apps,
        "effective": { "press": e.press, "long_press": e.long_press, "touch_tap": e.touch_tap },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::model::test_support::fixture_snapshot;

    #[test]
    fn apps_are_deduplicated_and_nameless_streams_skipped() {
        let c = choices(&fixture_snapshot(), &AudioSettings::default(), Controller::Encoder);
        let apps: Vec<&str> = c["apps"].as_array().unwrap().iter().map(|a| a["match"].as_str().unwrap()).collect();
        assert_eq!(apps, vec!["chrome", "spotify"]);
    }

    #[test]
    fn lists_devices_without_monitors_and_reports_effective_gestures() {
        let c = choices(&fixture_snapshot(), &AudioSettings::default(), Controller::Encoder);
        assert_eq!(c["outputs"].as_array().unwrap().len(), 2);
        assert_eq!(c["inputs"].as_array().unwrap().len(), 1);
        assert_eq!(c["effective"]["long_press"]["op"], "cycle_device");
        assert_eq!(c["controller"], "Encoder");
    }
}
```

- [ ] **Step 2: Write `src/actions/mod.rs` and `src/actions/audio.rs`**

`src/actions/mod.rs`:
```rust
pub mod audio;
```

`src/actions/audio.rs`:
```rust
use crate::audio::backend::{AudioBackend, BackendError};
use crate::audio::exec::execute;
use crate::audio::gesture::{self, Controller, Operation};
use crate::audio::model::Snapshot;
use crate::audio::pi::choices;
use crate::audio::settings::AudioSettings;
use crate::audio::target::resolve;
use crate::audio::view::{audio_view, error_view};
use crate::render::show_level;
use async_trait::async_trait;
use dashmap::DashMap;
use openaction::{Action, Instance, OpenActionResult};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{RwLock, watch};

/// Coalesces a burst of `pactl subscribe` events (one dial turn can emit
/// several) into one refresh.
pub const DEBOUNCE: Duration = Duration::from_millis(50);

struct Shared {
    backend: Arc<dyn AudioBackend>,
    /// Last snapshot, or a short message for why there is none.
    snapshot: RwLock<Result<Snapshot, String>>,
    /// Visible instances and their latest settings.
    instances: DashMap<String, AudioSettings>,
    /// When each currently-held key/dial went down.
    pressed: DashMap<String, Instant>,
}

#[derive(Clone)]
pub struct AudioAction {
    shared: Arc<Shared>,
}

fn short_error(e: &BackendError) -> String {
    match e {
        BackendError::NotInstalled => "pactl not found".to_string(),
        _ => "audio error".to_string(),
    }
}

fn controller(instance: &Instance) -> Controller {
    Controller::from_openaction(&instance.controller)
}

impl AudioAction {
    pub fn new(backend: Arc<dyn AudioBackend>) -> Self {
        Self {
            shared: Arc::new(Shared {
                backend,
                snapshot: RwLock::new(Err("loading".to_string())),
                instances: DashMap::new(),
                pressed: DashMap::new(),
            }),
        }
    }

    async fn refresh(&self) {
        let result = self.shared.backend.snapshot().await.map_err(|e| {
            log::warn!("audio snapshot failed: {e}");
            short_error(&e)
        });
        *self.shared.snapshot.write().await = result;
    }

    async fn render(&self, instance: &Instance, settings: &AudioSettings) -> OpenActionResult<()> {
        let view = match &*self.shared.snapshot.read().await {
            Ok(snap) => audio_view(&resolve(settings.target, &settings.target_name, snap), settings),
            Err(message) => error_view(settings, message),
        };
        show_level(instance, &view).await
    }

    async fn render_all(&self) {
        // Collect first so no DashMap shard lock is held across an await.
        let entries: Vec<(String, AudioSettings)> = self
            .shared
            .instances
            .iter()
            .map(|e| (e.key().clone(), e.value().clone()))
            .collect();
        for (id, settings) in entries {
            let Some(instance) = openaction::get_instance(id).await else {
                continue;
            };
            if let Err(e) = self.render(&instance, &settings).await {
                log::warn!("audio render failed: {e}");
            }
        }
    }

    /// Runs forever: refresh + re-render on every (debounced) change tick.
    pub async fn run_watcher(self, mut changes: watch::Receiver<u64>) {
        self.refresh().await;
        self.render_all().await;
        while changes.changed().await.is_ok() {
            tokio::time::sleep(DEBOUNCE).await;
            changes.mark_unchanged();
            self.refresh().await;
            self.render_all().await;
        }
    }

    async fn perform(&self, instance: &Instance, settings: &AudioSettings, op: Operation) -> OpenActionResult<()> {
        if op == Operation::None {
            return Ok(());
        }
        // Fresh read: a cached level could lag a fast dial by one debounce.
        let snap = match self.shared.backend.snapshot().await {
            Ok(snap) => snap,
            Err(e) => {
                log::warn!("audio snapshot failed: {e}");
                return instance.show_alert().await;
            }
        };
        let resolved = resolve(settings.target, &settings.target_name, &snap);
        if let Err(e) = execute(self.shared.backend.as_ref(), &snap, &resolved, settings, &op).await {
            log::warn!("audio {op:?} failed: {e}");
            instance.show_alert().await?;
        }
        self.refresh().await;
        self.render_all().await;
        Ok(())
    }

    fn press_started(&self, id: &str) {
        self.shared.pressed.insert(id.to_string(), Instant::now());
    }

    fn press_released(&self, id: &str) -> Duration {
        self.shared
            .pressed
            .remove(id)
            .map(|(_, t)| t.elapsed())
            .unwrap_or_default()
    }

    async fn press_down(&self, instance: &Instance, settings: &AudioSettings) -> OpenActionResult<()> {
        self.press_started(&instance.instance_id);
        let e = gesture::effective(settings, controller(instance));
        self.perform(instance, settings, gesture::down_operation(&e)).await
    }

    async fn press_up(&self, instance: &Instance, settings: &AudioSettings) -> OpenActionResult<()> {
        let held = self.press_released(&instance.instance_id);
        let e = gesture::effective(settings, controller(instance));
        self.perform(instance, settings, gesture::release_operation(&e, held, settings.step()))
            .await
    }
}

#[async_trait]
impl Action for AudioAction {
    const UUID: &'static str = "com.jfms7s.utilities.audio";
    type Settings = AudioSettings;

    async fn will_appear(&self, instance: &Instance, settings: &AudioSettings) -> OpenActionResult<()> {
        self.shared.instances.insert(instance.instance_id.clone(), settings.clone());
        self.render(instance, settings).await
    }

    async fn will_disappear(&self, instance: &Instance, _settings: &AudioSettings) -> OpenActionResult<()> {
        self.shared.instances.remove(&instance.instance_id);
        self.shared.pressed.remove(&instance.instance_id);
        Ok(())
    }

    async fn did_receive_settings(&self, instance: &Instance, settings: &AudioSettings) -> OpenActionResult<()> {
        self.shared.instances.insert(instance.instance_id.clone(), settings.clone());
        self.render(instance, settings).await
    }

    async fn key_down(&self, instance: &Instance, settings: &AudioSettings) -> OpenActionResult<()> {
        self.press_down(instance, settings).await
    }

    async fn key_up(&self, instance: &Instance, settings: &AudioSettings) -> OpenActionResult<()> {
        self.press_up(instance, settings).await
    }

    async fn dial_down(&self, instance: &Instance, settings: &AudioSettings) -> OpenActionResult<()> {
        self.press_down(instance, settings).await
    }

    async fn dial_up(&self, instance: &Instance, settings: &AudioSettings) -> OpenActionResult<()> {
        self.press_up(instance, settings).await
    }

    async fn dial_rotate(&self, instance: &Instance, settings: &AudioSettings, ticks: i16, _pressed: bool) -> OpenActionResult<()> {
        let op = gesture::rotate_operation(settings.rotate, ticks, settings.step());
        self.perform(instance, settings, op).await
    }

    async fn touch_tap(&self, instance: &Instance, settings: &AudioSettings, _position: (u16, u16), _hold: bool) -> OpenActionResult<()> {
        let e = gesture::effective(settings, controller(instance));
        self.perform(instance, settings, gesture::gesture_operation(&e.touch_tap, settings.step()))
            .await
    }

    async fn property_inspector_did_appear(&self, instance: &Instance, settings: &AudioSettings) -> OpenActionResult<()> {
        let snap = self.shared.backend.snapshot().await.unwrap_or_default();
        instance
            .send_to_property_inspector(choices(&snap, settings, controller(instance)))
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::fake::FakeBackend;

    fn action() -> AudioAction {
        AudioAction::new(Arc::new(FakeBackend::default()))
    }

    #[test]
    fn uuid_is_in_the_manifest() {
        let manifest: serde_json::Value = serde_json::from_str(include_str!("../../assets/manifest.json")).unwrap();
        let uuids: Vec<&str> = manifest["Actions"].as_array().unwrap().iter().map(|a| a["UUID"].as_str().unwrap()).collect();
        assert!(uuids.contains(&<AudioAction as Action>::UUID));
    }

    #[test]
    fn release_without_press_counts_as_a_tap() {
        assert_eq!(action().press_released("never-pressed"), Duration::ZERO);
    }

    #[test]
    fn press_is_forgotten_after_release() {
        let a = action();
        a.press_started("k");
        a.press_released("k");
        assert!(a.shared.pressed.is_empty());
    }

    #[tokio::test]
    async fn refresh_caches_the_snapshot() {
        let a = action();
        a.refresh().await;
        assert!(a.shared.snapshot.read().await.is_ok());
    }

    #[test]
    fn missing_pactl_gets_a_readable_message() {
        assert_eq!(short_error(&BackendError::NotInstalled), "pactl not found");
    }
}
```

- [ ] **Step 3: Add the Audio action to `assets/manifest.json`** (replace `"Actions": []`)

```json
	"Actions": [
		{
			"UUID": "com.jfms7s.utilities.audio",
			"Name": "Audio",
			"Icon": "icons/icon",
			"Tooltip": "Volume, mute and device switching for an output, input or app, with a live level on the key or touch strip",
			"Controllers": ["Encoder", "Keypad"],
			"PropertyInspectorPath": "propertyInspector/audio.html",
			"States": [{ "Image": "icons/actionDefaultImage" }],
			"Encoder": { "layout": "layouts/level.json" }
		}
	]
```

- [ ] **Step 4: Write `assets/propertyInspector/audio.html`**

```html
<!doctype html>
<html lang="en">
<head>
	<meta charset="utf-8" />
	<meta name="viewport" content="width=device-width, initial-scale=1" />
	<style>
		body { font: 12px system-ui, sans-serif; margin: 0; padding: 8px 12px; }
		label { display: block; margin-top: 10px; font-size: 11px; opacity: 0.8; }
		input, select { width: 100%; box-sizing: border-box; margin-top: 2px; }
		input[type="checkbox"] { width: auto; }
		.hidden { display: none; }
		fieldset { border: 0; margin: 10px 0 0; padding: 0; }
		legend { padding: 0; font-size: 11px; opacity: 0.8; }
		.checks label, label.inline { display: flex; align-items: center; gap: 4px; margin: 4px 0 0; font-size: 12px; opacity: 1; }
	</style>
</head>
<body>
	<label for="target">Control</label>
	<select id="target">
		<option value="default_output">Default output (follows system)</option>
		<option value="default_input">Default input / mic (follows system)</option>
		<option value="output">Specific output device</option>
		<option value="input">Specific input device</option>
		<option value="app">Application</option>
	</select>

	<div id="target_name_row" class="hidden">
		<label for="target_name" id="target_name_label">Device</label>
		<input id="target_name" list="target_names" />
		<datalist id="target_names"></datalist>
	</div>

	<label for="step">Volume step (%)</label>
	<input id="step" type="number" min="1" max="20" />

	<label for="max_volume">Maximum volume</label>
	<select id="max_volume">
		<option value="100">100%</option>
		<option value="125">125%</option>
		<option value="150">150%</option>
	</select>

	<div class="encoder-only">
		<label for="rotate">Rotate</label>
		<select id="rotate">
			<option value="volume">Change volume</option>
			<option value="cycle_device">Switch device</option>
			<option value="none">Nothing</option>
		</select>
	</div>

	<div id="gestures"></div>

	<fieldset>
		<legend>Devices to switch between (none ticked = all)</legend>
		<div class="checks" id="cycle_devices"></div>
	</fieldset>

	<label for="label">Label (blank = device or app name)</label>
	<input id="label" />

	<label for="accent">Accent colour</label>
	<input id="accent" type="color" />

	<label class="inline"><input id="show_percent" type="checkbox" /> Show percent</label>

	<datalist id="device_names"></datalist>

	<script>
		const OPS = [
			["none", "Nothing"], ["toggle_mute", "Toggle mute"], ["volume_up", "Volume up"],
			["volume_down", "Volume down"], ["set_volume", "Set volume to…"], ["cycle_device", "Switch to next device"],
			["set_default_device", "Switch to device…"], ["push_to_talk", "Push to talk (hold)"],
		];
		const SLOTS = [["press", "Press"], ["long_press", "Long press (hold ½ s)"], ["touch_tap", "Touch-strip tap"]];
		const byId = (id) => document.getElementById(id);

		let websocket, uuid, controller = "Keypad", settings = {}, choices = null;

		window.connectOpenActionSocketData = new Promise((resolve) => {
			window.connectOpenActionSocket = (...args) => resolve(args);
			window.connectElgatoStreamDeckSocket = window.connectOpenActionSocket;
		});

		window.connectOpenActionSocketData.then(([inPort, inUUID, inRegisterEvent, inInfo, inActionInfo]) => {
			uuid = inUUID;
			const actionInfo = JSON.parse(inActionInfo);
			controller = actionInfo.payload.controller || "Keypad";
			settings = actionInfo.payload.settings || {};
			websocket = new WebSocket(`ws://127.0.0.1:${inPort}`);
			websocket.onopen = () => {
				websocket.send(JSON.stringify({ event: inRegisterEvent, uuid: inUUID }));
				render();
			};
			websocket.onmessage = (event) => {
				const message = JSON.parse(event.data);
				if (message.event === "didReceiveSettings") {
					settings = message.payload.settings || {};
					render();
				} else if (message.event === "sendToPropertyInspector" && message.payload?.event === "audioChoices") {
					choices = message.payload;
					controller = choices.controller || controller;
					render();
				}
			};
		});

		function gestureValue(slot) {
			return settings[slot] ?? choices?.effective?.[slot] ?? { op: "none", volume: 50, device: "" };
		}

		function field(slot, name) {
			return document.querySelector(`[data-slot="${slot}"][data-field="${name}"]`);
		}

		function buildGestures() {
			const root = byId("gestures");
			root.replaceChildren();
			for (const [slot, title] of SLOTS) {
				if (slot === "touch_tap" && controller !== "Encoder") continue;
				const label = document.createElement("label");
				label.textContent = title;
				const select = document.createElement("select");
				select.dataset.slot = slot;
				select.dataset.field = "op";
				for (const [value, text] of OPS) {
					if (value === "push_to_talk" && slot !== "press") continue;
					const option = document.createElement("option");
					option.value = value;
					option.textContent = text;
					select.appendChild(option);
				}
				const volume = document.createElement("input");
				Object.assign(volume, { type: "number", min: 0, max: 150, placeholder: "Volume %" });
				volume.dataset.slot = slot;
				volume.dataset.field = "volume";
				const device = document.createElement("input");
				device.setAttribute("list", "device_names");
				device.placeholder = "Device";
				device.dataset.slot = slot;
				device.dataset.field = "device";
				for (const el of [select, volume, device]) el.addEventListener("change", onChange);
				root.append(label, select, volume, device);
			}
		}

		function toggleGestureFields(slot) {
			const select = field(slot, "op");
			if (!select) return;
			field(slot, "volume").classList.toggle("hidden", select.value !== "set_volume");
			field(slot, "device").classList.toggle("hidden", select.value !== "set_default_device");
		}

		function isInputTarget() {
			return ["default_input", "input"].includes(byId("target").value);
		}

		function fillOptions(list, items) {
			list.replaceChildren();
			for (const [value, text] of items) {
				const option = document.createElement("option");
				option.value = value;
				option.label = text;
				list.appendChild(option);
			}
		}

		function updateTargetName() {
			const target = byId("target").value;
			byId("target_name_row").classList.toggle("hidden", !["output", "input", "app"].includes(target));
			byId("target_name_label").textContent = target === "app" ? "Application (process or app name)" : "Device";
			if (!choices) return;
			const devices = isInputTarget() ? choices.inputs : choices.outputs;
			const items = target === "app"
				? choices.apps.map((a) => [a.match, a.app_name])
				: ["output", "input"].includes(target) ? devices.map((d) => [d.name, d.description]) : [];
			fillOptions(byId("target_names"), items);
			// Apps can only be moved to outputs.
			const switchable = target === "app" ? choices.outputs : devices;
			fillOptions(byId("device_names"), switchable.map((d) => [d.name, d.description]));
		}

		function fillCycle() {
			const root = byId("cycle_devices");
			root.replaceChildren();
			if (!choices) return;
			const list = isInputTarget() ? choices.inputs : choices.outputs;
			const chosen = settings.cycle_devices || [];
			for (const d of list) {
				const label = document.createElement("label");
				const box = document.createElement("input");
				box.type = "checkbox";
				box.value = d.name;
				box.checked = chosen.includes(d.name);
				box.addEventListener("change", onChange);
				label.append(box, document.createTextNode(d.description));
				root.appendChild(label);
			}
		}

		function render() {
			byId("target").value = settings.target || "default_output";
			byId("target_name").value = settings.target_name || "";
			byId("step").value = settings.step ?? 5;
			byId("max_volume").value = String(settings.max_volume ?? 100);
			byId("rotate").value = settings.rotate || "volume";
			byId("label").value = settings.label || "";
			byId("accent").value = /^#[0-9a-fA-F]{6}$/.test(settings.accent || "") ? settings.accent : "#4fc3f7";
			byId("show_percent").checked = settings.show_percent ?? true;
			document.querySelectorAll(".encoder-only").forEach((el) => el.classList.toggle("hidden", controller !== "Encoder"));
			buildGestures();
			for (const [slot] of SLOTS) {
				const select = field(slot, "op");
				if (!select) continue;
				const g = gestureValue(slot);
				select.value = g.op;
				field(slot, "volume").value = g.volume ?? 50;
				field(slot, "device").value = g.device || "";
				toggleGestureFields(slot);
			}
			updateTargetName();
			fillCycle();
		}

		function collect() {
			const next = {
				target: byId("target").value,
				target_name: byId("target_name").value.trim(),
				step: parseInt(byId("step").value, 10) || 5,
				max_volume: parseInt(byId("max_volume").value, 10) || 100,
				rotate: byId("rotate").value,
				label: byId("label").value,
				accent: byId("accent").value,
				show_percent: byId("show_percent").checked,
				cycle_devices: Array.from(document.querySelectorAll("#cycle_devices input:checked")).map((b) => b.value),
			};
			for (const [slot] of SLOTS) {
				const select = field(slot, "op");
				if (!select) {
					// Not shown for this controller - keep whatever was stored.
					if (settings[slot]) next[slot] = settings[slot];
					continue;
				}
				next[slot] = {
					op: select.value,
					volume: parseInt(field(slot, "volume").value, 10) || 0,
					device: field(slot, "device").value.trim(),
				};
			}
			return next;
		}

		function onChange() {
			settings = collect();
			for (const [slot] of SLOTS) toggleGestureFields(slot);
			updateTargetName();
			fillCycle();
			websocket.send(JSON.stringify({ event: "setSettings", context: uuid, payload: settings }));
		}

		for (const id of ["target", "target_name", "step", "max_volume", "rotate", "label", "accent", "show_percent"]) {
			byId(id).addEventListener("change", onChange);
		}
	</script>
</body>
</html>
```

- [ ] **Step 5: Wire `src/main.rs`** (replace the whole file; the `dead_code` allows on `audio`, `cycle`, `lenient`, `render` are removed now that everything is used)

```rust
mod actions;
mod audio;
mod cycle;
mod lenient;
mod render;

use actions::audio::AudioAction;
use audio::backend::{AudioBackend, PactlBackend, spawn_subscriber};
use openaction::{OpenActionResult, register_action, run};
use std::sync::Arc;
use tokio::sync::watch;

#[tokio::main]
async fn main() -> OpenActionResult<()> {
    simplelog::SimpleLogger::init(log::LevelFilter::Info, simplelog::Config::default())
        .expect("logger init");

    let (audio_tx, audio_rx) = watch::channel(0u64);
    spawn_subscriber(audio_tx);
    let backend: Arc<dyn AudioBackend> = Arc::new(PactlBackend);
    let audio = AudioAction::new(backend);
    tokio::spawn(audio.clone().run_watcher(audio_rx));
    register_action(audio).await;

    run(std::env::args().collect()).await
}

#[cfg(test)]
mod tests {
    #[test]
    fn manifest_version_matches_cargo() {
        let manifest: serde_json::Value =
            serde_json::from_str(include_str!("../assets/manifest.json")).unwrap();
        assert_eq!(manifest["Version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(
            manifest["CodePathLin"],
            "opendeck-utilities-x86_64-unknown-linux-gnu"
        );
    }
}
```

If clippy now reports a genuinely unused item, delete that item rather than re-adding an `allow`.

- [ ] **Step 6: Run and verify**

Run: `cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test`
Expected: PASS.

- [ ] **Step 7: Manual check on the device**

```bash
cargo build --release --target x86_64-unknown-linux-gnu
node build.mjs x86_64-unknown-linux-gnu
rm -rf ~/.config/opendeck/plugins/com.jfms7s.utilities.sdPlugin
cp -r dist/com.jfms7s.utilities.sdPlugin ~/.config/opendeck/plugins/
```

Restart OpenDeck. Put **Audio** on a dial (default output) and on a key (target *Application*, `chrome`). Check: rotating changes volume and the strip follows; press mutes (red, struck icon); long-press switches the default output; changing volume in GNOME settings updates the strip within a second; a key for a non-running app shows "not playing".

- [ ] **Step 8: Commit**

```bash
git add -A
git commit -m "feat(audio): add the Audio action with live feedback and its property inspector

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 8: OpenDeck state reader and host events

**Files:**
- Create: `src/opendeck_state.rs`, `src/host.rs`
- Modify: `src/main.rs` (add `#[allow(dead_code)] mod host;` and `#[allow(dead_code)] mod opendeck_state;` — removed in Task 10)

**Interfaces:**
- Produces:
  - `opendeck_state::{OpenDeckState, OpenDeckState::discover(), OpenDeckState::at(PathBuf), brightness(&self) -> Option<u8>, profiles(&self, device) -> Vec<String>, active_profile(&self, device) -> Option<String>, fingerprint(&self) -> Vec<(PathBuf, Option<SystemTime>)>, find_config_dir(&Path) -> Option<PathBuf>, is_safe_device_id(&str) -> bool, spawn_watcher(OpenDeckState, watch::Sender<u64>)}`
  - `host::{BrightnessChange { Set, Increase, Decrease }, brightness_event(BrightnessChange, u8) -> Value, switch_profile_event(&str, &str) -> Value, send(Value) -> OpenActionResult<()>, CONFIRM_TIMEOUT, wait_for<T: PartialEq>(&T, impl FnMut() -> Option<T>, Duration) -> bool, warn_ignored_once()}`

- [ ] **Step 1: Write `src/opendeck_state.rs` with tests (tests first, at the bottom)**

```rust
//! Read-only view of OpenDeck's own config files: current brightness,
//! the profiles per device, and which one is active. Never written - these
//! files belong to OpenDeck.

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};
use tokio::sync::watch;

const CANDIDATES: [&str; 2] = [
    ".config/opendeck",
    ".var/app/me.amankhanna.opendeck/config/opendeck",
];
const POLL: Duration = Duration::from_secs(1);

pub fn find_config_dir(home: &Path) -> Option<PathBuf> {
    CANDIDATES.iter().map(|p| home.join(p)).find(|p| p.is_dir())
}

/// Device ids come from settings (user-editable) and are joined into
/// paths, so anything that could leave `profiles/` is refused.
pub fn is_safe_device_id(id: &str) -> bool {
    !id.is_empty()
        && id != "."
        && id != ".."
        && !id.contains(['/', '\\', '\0'])
}

fn read_json(path: &Path) -> Option<Value> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

fn mtime(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

#[derive(Debug, Clone)]
pub struct OpenDeckState {
    dir: Option<PathBuf>,
}

impl OpenDeckState {
    pub fn discover() -> Self {
        let dir = std::env::var_os("HOME").and_then(|h| find_config_dir(Path::new(&h)));
        if dir.is_none() {
            log::warn!("OpenDeck config directory not found; brightness/profile state unknown");
        }
        Self { dir }
    }

    pub fn at(dir: PathBuf) -> Self {
        Self { dir: Some(dir) }
    }

    pub fn brightness(&self) -> Option<u8> {
        let settings = read_json(&self.dir.as_ref()?.join("settings.json"))?;
        let value = settings.get("brightness")?.as_u64()?;
        u8::try_from(value.min(100)).ok()
    }

    pub fn profiles(&self, device: &str) -> Vec<String> {
        let Some(dir) = self.dir.as_ref().filter(|_| is_safe_device_id(device)) else {
            return Vec::new();
        };
        let Ok(entries) = std::fs::read_dir(dir.join("profiles").join(device)) else {
            return Vec::new();
        };
        let mut names: Vec<String> = entries
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.is_file() && p.extension().is_some_and(|x| x == "json"))
            .filter_map(|p| p.file_stem().and_then(|s| s.to_str()).map(str::to_string))
            .collect();
        names.sort();
        names
    }

    pub fn active_profile(&self, device: &str) -> Option<String> {
        if !is_safe_device_id(device) {
            return None;
        }
        let path = self.dir.as_ref()?.join("profiles").join(format!("{device}.json"));
        Some(read_json(&path)?.get("selected_profile")?.as_str()?.to_string())
    }

    /// Modification times of everything the readers above look at; any
    /// difference between two calls means something changed.
    pub fn fingerprint(&self) -> Vec<(PathBuf, Option<SystemTime>)> {
        let Some(dir) = &self.dir else {
            return Vec::new();
        };
        let settings = dir.join("settings.json");
        let mut out = vec![(settings.clone(), mtime(&settings))];
        if let Ok(entries) = std::fs::read_dir(dir.join("profiles")) {
            out.extend(entries.filter_map(Result::ok).map(|e| {
                let p = e.path();
                let t = mtime(&p);
                (p, t)
            }));
        }
        out.sort();
        out
    }
}

/// Polls `fingerprint` every second and bumps `tx` when it changes.
pub fn spawn_watcher(state: OpenDeckState, tx: watch::Sender<u64>) {
    tokio::spawn(async move {
        let mut last = state.fingerprint();
        let mut tick = tokio::time::interval(POLL);
        loop {
            tick.tick().await;
            let now = state.fingerprint();
            if now != last {
                last = now;
                tx.send_modify(|n| *n = n.wrapping_add(1));
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn fixture() -> (tempfile::TempDir, OpenDeckState) {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_path_buf();
        fs::write(dir.join("settings.json"), r#"{"brightness": 50, "language": "en"}"#).unwrap();
        fs::create_dir_all(dir.join("profiles/sd-1")).unwrap();
        for p in ["Default", "gaming", "claude"] {
            fs::write(dir.join(format!("profiles/sd-1/{p}.json")), "{}").unwrap();
        }
        fs::write(dir.join("profiles/sd-1/notes.txt"), "x").unwrap();
        fs::write(dir.join("profiles/sd-1.json"), r#"{"selected_profile": "gaming"}"#).unwrap();
        (tmp, OpenDeckState::at(dir))
    }

    #[test]
    fn reads_brightness() {
        let (_tmp, s) = fixture();
        assert_eq!(s.brightness(), Some(50));
    }

    #[test]
    fn lists_json_profiles_sorted() {
        let (_tmp, s) = fixture();
        assert_eq!(s.profiles("sd-1"), vec!["Default", "claude", "gaming"]);
    }

    #[test]
    fn reads_the_active_profile() {
        let (_tmp, s) = fixture();
        assert_eq!(s.active_profile("sd-1").as_deref(), Some("gaming"));
    }

    #[test]
    fn missing_or_malformed_files_are_unknown() {
        let (tmp, s) = fixture();
        fs::write(tmp.path().join("settings.json"), "{ not json").unwrap();
        assert_eq!(s.brightness(), None);
        assert!(s.profiles("sd-2").is_empty());
        assert_eq!(s.active_profile("sd-2"), None);
        let none = OpenDeckState { dir: None };
        assert_eq!(none.brightness(), None);
        assert!(none.fingerprint().is_empty());
    }

    #[test]
    fn unsafe_device_ids_read_nothing() {
        let (tmp, s) = fixture();
        // A profile-shaped file one level above profiles/ that a traversal would reach.
        fs::write(tmp.path().join("secret.json"), r#"{"selected_profile": "leaked"}"#).unwrap();
        for bad in ["..", "../secret", "a/b", "", ".", "sd-1\\x"] {
            assert!(!is_safe_device_id(bad), "{bad}");
            assert!(s.profiles(bad).is_empty(), "{bad}");
            assert_eq!(s.active_profile(bad), None, "{bad}");
        }
        assert_eq!(s.active_profile("../secret"), None);
    }

    #[test]
    fn fingerprint_changes_when_a_file_changes() {
        let (tmp, s) = fixture();
        let before = s.fingerprint();
        std::thread::sleep(Duration::from_millis(20));
        fs::write(tmp.path().join("profiles/sd-1.json"), r#"{"selected_profile": "Default"}"#).unwrap();
        assert_ne!(before, s.fingerprint());
    }

    #[test]
    fn finds_native_then_flatpak_config() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(find_config_dir(tmp.path()), None);
        let flatpak = tmp.path().join(CANDIDATES[1]);
        fs::create_dir_all(&flatpak).unwrap();
        assert_eq!(find_config_dir(tmp.path()), Some(flatpak));
        let native = tmp.path().join(CANDIDATES[0]);
        fs::create_dir_all(&native).unwrap();
        assert_eq!(find_config_dir(tmp.path()), Some(native));
    }
}
```

Run: `cargo test opendeck_state` (after adding the `mod` line in Step 3) — Expected: PASS, 7 tests.

- [ ] **Step 2: Write `src/host.rs` with tests**

```rust
//! Events sent to the OpenDeck host itself. Stock OpenDeck only honours
//! `switchProfile`/`deviceBrightness` from the Starter Pack plugin
//! (src-tauri/src/events/inbound/mod.rs), so callers confirm the effect by
//! re-reading OpenDeck's state and warn when it never arrives.

use serde::Serialize;
use serde_json::{Value, json};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::time::Instant;

pub const CONFIRM_TIMEOUT: Duration = Duration::from_millis(1500);
const CONFIRM_POLL: Duration = Duration::from_millis(100);

static WARNED: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum BrightnessChange {
    Set,
    Increase,
    Decrease,
}

pub fn brightness_event(change: BrightnessChange, value: u8) -> Value {
    json!({ "event": "deviceBrightness", "action": change, "value": value.min(100) })
}

pub fn switch_profile_event(device: &str, profile: &str) -> Value {
    json!({ "event": "switchProfile", "device": device, "profile": profile })
}

pub async fn send(event: Value) -> openaction::OpenActionResult<()> {
    openaction::send_arbitrary_json(event).await
}

/// Polls `read` until it returns `expected` or `timeout` passes.
pub async fn wait_for<T: PartialEq>(expected: &T, mut read: impl FnMut() -> Option<T>, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if read().as_ref() == Some(expected) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(CONFIRM_POLL).await;
    }
}

pub fn warn_ignored_once() {
    if !WARNED.swap(true, Ordering::SeqCst) {
        log::warn!(
            "OpenDeck ignored a brightness/profile request: stock OpenDeck only accepts these \
             from com.amansprojects.starterpack.sdPlugin (see README)"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn event_shapes_match_opendeck() {
        assert_eq!(
            brightness_event(BrightnessChange::Increase, 5),
            json!({"event": "deviceBrightness", "action": "increase", "value": 5})
        );
        assert_eq!(brightness_event(BrightnessChange::Set, 200)["value"], 100);
        assert_eq!(
            switch_profile_event("sd-1", "gaming"),
            json!({"event": "switchProfile", "device": "sd-1", "profile": "gaming"})
        );
    }

    #[tokio::test(start_paused = true)]
    async fn wait_for_sees_a_change_before_the_timeout() {
        let calls = Cell::new(0);
        let ok = wait_for(&7, || { calls.set(calls.get() + 1); (calls.get() >= 3).then_some(7) }, CONFIRM_TIMEOUT).await;
        assert!(ok);
    }

    #[tokio::test(start_paused = true)]
    async fn wait_for_gives_up_after_the_timeout() {
        let start = Instant::now();
        assert!(!wait_for(&7, || Some(1), CONFIRM_TIMEOUT).await);
        assert!(start.elapsed() >= CONFIRM_TIMEOUT);
    }
}
```

- [ ] **Step 3: Register modules in `src/main.rs`**

```rust
#[allow(dead_code)]
mod host;
#[allow(dead_code)]
mod opendeck_state;
```

- [ ] **Step 4: Run and verify**

Run: `cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat: read OpenDeck brightness/profile state and send host events

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 9: Device Brightness action

**Files:**
- Create: `src/brightness.rs`, `src/actions/brightness.rs`, `assets/propertyInspector/brightness.html`
- Modify: `src/actions/mod.rs`, `src/main.rs`, `assets/manifest.json`

**Interfaces:**
- Consumes: `host::*`, `OpenDeckState`, `render::{show_level, level::LevelView, icons::Icon}` (Tasks 6, 8).
- Produces:
  - `brightness::{KeyOp { Set, Increase, Decrease, TogglePresets }, BrightnessSettings { step, key_op, value, preset_a, preset_b }, Request { change, value, expected: Option<u8> }, apply(u8, BrightnessChange, u8) -> u8, rotate_request(Option<u8>, i16, &BrightnessSettings) -> Option<Request>, toggle_request(Option<u8>, &BrightnessSettings) -> Request, key_request(Option<u8>, &BrightnessSettings) -> Request, brightness_view(Option<u8>) -> LevelView}`
  - `actions::brightness::BrightnessAction` (`UUID = "com.jfms7s.utilities.brightness"`), `new(OpenDeckState)`, `run_watcher(self, watch::Receiver<u64>)`

- [ ] **Step 1: Write `src/brightness.rs` with tests**

```rust
//! Pure brightness logic: which host request a gesture makes, what the
//! resulting value should be, and what the control shows.

use crate::host::BrightnessChange;
use crate::lenient::lenient;
use crate::render::icons::Icon;
use crate::render::level::{INACTIVE_COLOR, LevelView};
use serde::{Deserialize, Serialize};

pub const BRIGHTNESS_COLOR: &str = "#fbbf24";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyOp {
    Set,
    #[default]
    Increase,
    Decrease,
    TogglePresets,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct BrightnessSettings {
    #[serde(deserialize_with = "lenient")]
    pub step: u8,
    #[serde(deserialize_with = "lenient")]
    pub key_op: KeyOp,
    /// Target for `KeyOp::Set`.
    #[serde(deserialize_with = "lenient")]
    pub value: u8,
    #[serde(deserialize_with = "lenient")]
    pub preset_a: u8,
    #[serde(deserialize_with = "lenient")]
    pub preset_b: u8,
}

impl Default for BrightnessSettings {
    fn default() -> Self {
        Self { step: 5, key_op: KeyOp::Increase, value: 50, preset_a: 0, preset_b: 50 }
    }
}

impl BrightnessSettings {
    fn step(&self) -> u8 {
        self.step.clamp(1, 50)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Request {
    pub change: BrightnessChange,
    pub value: u8,
    /// What OpenDeck's stored brightness should become; `None` if unknown.
    pub expected: Option<u8>,
}

/// Mirrors OpenDeck's own handling (SettingsView.svelte).
pub fn apply(current: u8, change: BrightnessChange, value: u8) -> u8 {
    match change {
        BrightnessChange::Set => value.min(100),
        BrightnessChange::Increase => current.saturating_add(value).min(100),
        BrightnessChange::Decrease => current.saturating_sub(value),
    }
}

fn request(current: Option<u8>, change: BrightnessChange, value: u8) -> Request {
    Request { change, value, expected: current.map(|c| apply(c, change, value)) }
}

pub fn rotate_request(current: Option<u8>, ticks: i16, s: &BrightnessSettings) -> Option<Request> {
    if ticks == 0 {
        return None;
    }
    let amount = (u32::from(ticks.unsigned_abs()) * u32::from(s.step())).min(100) as u8;
    let change = if ticks > 0 { BrightnessChange::Increase } else { BrightnessChange::Decrease };
    Some(request(current, change, amount))
}

/// Goes to preset B when currently at preset A, otherwise to preset A.
pub fn toggle_request(current: Option<u8>, s: &BrightnessSettings) -> Request {
    let (a, b) = (s.preset_a.min(100), s.preset_b.min(100));
    let target = if current == Some(a) { b } else { a };
    Request { change: BrightnessChange::Set, value: target, expected: Some(target) }
}

pub fn key_request(current: Option<u8>, s: &BrightnessSettings) -> Request {
    match s.key_op {
        KeyOp::Set => Request { change: BrightnessChange::Set, value: s.value.min(100), expected: Some(s.value.min(100)) },
        KeyOp::Increase => request(current, BrightnessChange::Increase, s.step()),
        KeyOp::Decrease => request(current, BrightnessChange::Decrease, s.step()),
        KeyOp::TogglePresets => toggle_request(current, s),
    }
}

pub fn brightness_view(current: Option<u8>) -> LevelView {
    match current {
        Some(v) => LevelView {
            title: "Brightness".to_string(),
            value_text: format!("{v}%"),
            bar: f64::from(v),
            color: BRIGHTNESS_COLOR.to_string(),
            icon: Icon::Sun,
            struck: false,
        },
        None => LevelView {
            title: "Brightness".to_string(),
            value_text: "unknown".to_string(),
            bar: 0.0,
            color: INACTIVE_COLOR.to_string(),
            icon: Icon::Sun,
            struck: false,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s() -> BrightnessSettings {
        BrightnessSettings::default()
    }

    #[test]
    fn rotation_scales_by_ticks_and_predicts_the_result() {
        let r = rotate_request(Some(50), 3, &s()).unwrap();
        assert_eq!((r.change, r.value, r.expected), (BrightnessChange::Increase, 15, Some(65)));
        let r = rotate_request(Some(3), -1, &s()).unwrap();
        assert_eq!((r.change, r.expected), (BrightnessChange::Decrease, Some(0)));
        assert_eq!(rotate_request(Some(3), 0, &s()), None);
    }

    #[test]
    fn unknown_current_has_no_expectation() {
        assert_eq!(rotate_request(None, 1, &s()).unwrap().expected, None);
    }

    #[test]
    fn increase_saturates_at_100() {
        assert_eq!(apply(98, BrightnessChange::Increase, 5), 100);
    }

    #[test]
    fn toggle_goes_between_presets() {
        assert_eq!(toggle_request(Some(0), &s()).value, 50);
        assert_eq!(toggle_request(Some(50), &s()).value, 0);
        assert_eq!(toggle_request(Some(73), &s()).value, 0);
        assert_eq!(toggle_request(None, &s()).value, 0);
    }

    #[test]
    fn key_ops() {
        let set = BrightnessSettings { key_op: KeyOp::Set, value: 120, ..s() };
        assert_eq!(key_request(Some(10), &set).expected, Some(100));
        let dec = BrightnessSettings { key_op: KeyOp::Decrease, ..s() };
        assert_eq!(key_request(Some(10), &dec).expected, Some(5));
    }

    #[test]
    fn garbled_settings_fall_back() {
        let parsed: BrightnessSettings = serde_json::from_value(serde_json::json!({"key_op": "melt", "step": "big"})).unwrap();
        assert_eq!(parsed.key_op, KeyOp::Increase);
        assert_eq!(parsed.step(), 1);
    }

    #[test]
    fn view() {
        assert_eq!(brightness_view(Some(40)).value_text, "40%");
        assert_eq!(brightness_view(None).color, INACTIVE_COLOR);
    }
}
```

- [ ] **Step 2: Write `src/actions/brightness.rs`**

```rust
use crate::brightness::{BrightnessSettings, Request, brightness_view, key_request, rotate_request, toggle_request};
use crate::host;
use crate::opendeck_state::OpenDeckState;
use crate::render::show_level;
use async_trait::async_trait;
use dashmap::DashSet;
use openaction::{Action, Instance, OpenActionResult};
use std::sync::Arc;
use tokio::sync::watch;

struct Shared {
    state: OpenDeckState,
    instances: DashSet<String>,
}

#[derive(Clone)]
pub struct BrightnessAction {
    shared: Arc<Shared>,
}

impl BrightnessAction {
    pub fn new(state: OpenDeckState) -> Self {
        Self { shared: Arc::new(Shared { state, instances: DashSet::new() }) }
    }

    async fn render(&self, instance: &Instance) -> OpenActionResult<()> {
        show_level(instance, &brightness_view(self.shared.state.brightness())).await
    }

    async fn render_all(&self) {
        let ids: Vec<String> = self.shared.instances.iter().map(|e| e.key().clone()).collect();
        for id in ids {
            let Some(instance) = openaction::get_instance(id).await else {
                continue;
            };
            if let Err(e) = self.render(&instance).await {
                log::warn!("brightness render failed: {e}");
            }
        }
    }

    pub async fn run_watcher(self, mut changes: watch::Receiver<u64>) {
        while changes.changed().await.is_ok() {
            self.render_all().await;
        }
    }

    /// Sends the request, then checks in the background that OpenDeck's
    /// stored brightness actually moved; alerts once if it didn't.
    async fn send(&self, instance: &Instance, request: Request) -> OpenActionResult<()> {
        let before = self.shared.state.brightness();
        if let Err(e) = host::send(host::brightness_event(request.change, request.value)).await {
            log::warn!("deviceBrightness send failed: {e}");
            return instance.show_alert().await;
        }
        let Some(expected) = request.expected.filter(|e| Some(*e) != before) else {
            return Ok(());
        };
        let state = self.shared.state.clone();
        let id = instance.instance_id.clone();
        tokio::spawn(async move {
            if !host::wait_for(&expected, || state.brightness(), host::CONFIRM_TIMEOUT).await {
                host::warn_ignored_once();
                if let Some(instance) = openaction::get_instance(id).await {
                    let _ = instance.show_alert().await;
                }
            }
        });
        Ok(())
    }
}

#[async_trait]
impl Action for BrightnessAction {
    const UUID: &'static str = "com.jfms7s.utilities.brightness";
    type Settings = BrightnessSettings;

    async fn will_appear(&self, instance: &Instance, _settings: &BrightnessSettings) -> OpenActionResult<()> {
        self.shared.instances.insert(instance.instance_id.clone());
        self.render(instance).await
    }

    async fn will_disappear(&self, instance: &Instance, _settings: &BrightnessSettings) -> OpenActionResult<()> {
        self.shared.instances.remove(&instance.instance_id);
        Ok(())
    }

    async fn key_up(&self, instance: &Instance, settings: &BrightnessSettings) -> OpenActionResult<()> {
        self.send(instance, key_request(self.shared.state.brightness(), settings)).await
    }

    async fn dial_rotate(&self, instance: &Instance, settings: &BrightnessSettings, ticks: i16, _pressed: bool) -> OpenActionResult<()> {
        match rotate_request(self.shared.state.brightness(), ticks, settings) {
            Some(request) => self.send(instance, request).await,
            None => Ok(()),
        }
    }

    async fn dial_up(&self, instance: &Instance, settings: &BrightnessSettings) -> OpenActionResult<()> {
        self.send(instance, toggle_request(self.shared.state.brightness(), settings)).await
    }

    async fn touch_tap(&self, instance: &Instance, settings: &BrightnessSettings, _position: (u16, u16), _hold: bool) -> OpenActionResult<()> {
        self.send(instance, toggle_request(self.shared.state.brightness(), settings)).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uuid_is_in_the_manifest() {
        let manifest: serde_json::Value = serde_json::from_str(include_str!("../../assets/manifest.json")).unwrap();
        let uuids: Vec<&str> = manifest["Actions"].as_array().unwrap().iter().map(|a| a["UUID"].as_str().unwrap()).collect();
        assert!(uuids.contains(&<BrightnessAction as Action>::UUID));
    }
}
```

`src/actions/mod.rs` add `pub mod brightness;`.

- [ ] **Step 3: Add to `assets/manifest.json` `Actions`**

```json
		{
			"UUID": "com.jfms7s.utilities.brightness",
			"Name": "Device Brightness",
			"Icon": "icons/icon",
			"Tooltip": "Step or set the Stream Deck's brightness (needs OpenDeck to accept the request - see README)",
			"Controllers": ["Encoder", "Keypad"],
			"PropertyInspectorPath": "propertyInspector/brightness.html",
			"States": [{ "Image": "icons/actionDefaultImage" }],
			"Encoder": { "layout": "layouts/level.json" }
		}
```

- [ ] **Step 4: Write `assets/propertyInspector/brightness.html`**

```html
<!doctype html>
<html lang="en">
<head>
	<meta charset="utf-8" />
	<style>
		body { font: 12px system-ui, sans-serif; margin: 0; padding: 8px 12px; }
		label { display: block; margin-top: 10px; font-size: 11px; opacity: 0.8; }
		input, select { width: 100%; box-sizing: border-box; margin-top: 2px; }
		.hidden { display: none; }
		p.note { font-size: 11px; opacity: 0.7; }
	</style>
</head>
<body>
	<div id="key_only">
		<label for="key_op">Key press</label>
		<select id="key_op">
			<option value="increase">Increase by step</option>
			<option value="decrease">Decrease by step</option>
			<option value="set">Set to value</option>
			<option value="toggle_presets">Toggle between presets</option>
		</select>
		<label for="value">Value (%)</label>
		<input id="value" type="number" min="0" max="100" />
	</div>
	<label for="step">Step (%)</label>
	<input id="step" type="number" min="1" max="50" />
	<label for="preset_a">Preset A (%)</label>
	<input id="preset_a" type="number" min="0" max="100" />
	<label for="preset_b">Preset B (%)</label>
	<input id="preset_b" type="number" min="0" max="100" />
	<p class="note">Dial: rotate steps, press toggles presets. Stock OpenDeck only accepts brightness changes from its Starter Pack; if nothing happens the control flashes an alert.</p>
	<script>
		const FIELDS = { key_op: "increase", value: 50, step: 5, preset_a: 0, preset_b: 50 };
		const byId = (id) => document.getElementById(id);
		let websocket, uuid;

		window.connectOpenActionSocketData = new Promise((resolve) => {
			window.connectOpenActionSocket = (...args) => resolve(args);
			window.connectElgatoStreamDeckSocket = window.connectOpenActionSocket;
		});
		window.connectOpenActionSocketData.then(([inPort, inUUID, inRegisterEvent, inInfo, inActionInfo]) => {
			uuid = inUUID;
			const actionInfo = JSON.parse(inActionInfo);
			byId("key_only").classList.toggle("hidden", actionInfo.payload.controller !== "Keypad");
			websocket = new WebSocket(`ws://127.0.0.1:${inPort}`);
			websocket.onopen = () => {
				websocket.send(JSON.stringify({ event: inRegisterEvent, uuid: inUUID }));
				apply(actionInfo.payload.settings || {});
			};
			websocket.onmessage = (event) => {
				const message = JSON.parse(event.data);
				if (message.event === "didReceiveSettings") apply(message.payload.settings || {});
			};
		});

		function apply(settings) {
			for (const [id, fallback] of Object.entries(FIELDS)) byId(id).value = settings[id] ?? fallback;
		}

		function send() {
			const payload = { key_op: byId("key_op").value };
			for (const id of ["value", "step", "preset_a", "preset_b"]) payload[id] = parseInt(byId(id).value, 10) || 0;
			websocket.send(JSON.stringify({ event: "setSettings", context: uuid, payload }));
		}
		for (const id of Object.keys(FIELDS)) byId(id).addEventListener("change", send);
	</script>
</body>
</html>
```

- [ ] **Step 5: Wire into `src/main.rs`**

Add modules (keep `#[allow(dead_code)]` on `opendeck_state` until Task 10 — `profiles`/`active_profile` are still unused; `host` is now fully used by brightness except `switch_profile_event`, so keep its allow too):

```rust
mod brightness;
```

Add to `main()` after the audio registration:

```rust
    let deck_state = opendeck_state::OpenDeckState::discover();
    let (deck_tx, deck_rx) = watch::channel(0u64);
    opendeck_state::spawn_watcher(deck_state.clone(), deck_tx);
    let brightness = actions::brightness::BrightnessAction::new(deck_state.clone());
    tokio::spawn(brightness.clone().run_watcher(deck_rx.clone()));
    register_action(brightness).await;
```

- [ ] **Step 6: Run and verify**

Run: `cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "feat(brightness): add the Device Brightness action

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 10: Switch Profile action

**Files:**
- Create: `src/profile.rs`, `src/render/profile.rs`, `src/actions/profile.rs`, `assets/layouts/profile.json`, `assets/propertyInspector/profile.html`
- Modify: `src/render/mod.rs`, `src/actions/mod.rs`, `src/main.rs`, `assets/manifest.json`

**Interfaces:**
- Consumes: `cycle::step_in`, `host::*`, `OpenDeckState`, `render::tile`, `render::text::shorten`.
- Produces:
  - `profile::{ProfileSettings { device, profile, cycle: Vec<String> }, target_device(&ProfileSettings, own: &str) -> String, cycle_list(all, subset) -> Vec<String>, ProfileView { active: Option<String>, shown: String, is_active: bool, hint: String }, key_view(active: Option<&str>, target: &str) -> ProfileView, dial_view(active: Option<&str>, highlighted: Option<&str>) -> ProfileView}`
  - `render::profile::{strip_feedback(&ProfileView) -> Value, tile_image(&ProfileView) -> String}`; `render::show_profile(&Instance, &ProfileView)`
  - `actions::profile::ProfileAction` (`UUID = "com.jfms7s.utilities.profile"`), `new(OpenDeckState)`, `run_watcher`

- [ ] **Step 1: Write `src/profile.rs` with tests**

```rust
use crate::lenient::lenient;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProfileSettings {
    /// Empty = the device this control is on.
    #[serde(deserialize_with = "lenient")]
    pub device: String,
    /// Key: profile to switch to.
    #[serde(deserialize_with = "lenient")]
    pub profile: String,
    /// Dial: profiles to cycle through, in order; empty = all.
    #[serde(deserialize_with = "lenient")]
    pub cycle: Vec<String>,
}

pub fn target_device(s: &ProfileSettings, own: &str) -> String {
    let d = s.device.trim();
    if d.is_empty() { own.to_string() } else { d.to_string() }
}

pub fn cycle_list(all: &[String], subset: &[String]) -> Vec<String> {
    if subset.is_empty() {
        all.to_vec()
    } else {
        subset.iter().filter(|p| all.contains(p)).cloned().collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileView {
    pub active: Option<String>,
    pub shown: String,
    pub is_active: bool,
    pub hint: String,
}

pub fn key_view(active: Option<&str>, target: &str) -> ProfileView {
    let target = target.trim();
    let is_active = !target.is_empty() && active == Some(target);
    let hint = if target.is_empty() {
        "set a profile"
    } else if is_active {
        "active"
    } else {
        "press to switch"
    };
    ProfileView {
        active: active.map(str::to_string),
        shown: if target.is_empty() { "—".to_string() } else { target.to_string() },
        is_active,
        hint: hint.to_string(),
    }
}

pub fn dial_view(active: Option<&str>, highlighted: Option<&str>) -> ProfileView {
    let shown = highlighted.or(active).unwrap_or("—");
    let is_active = active == Some(shown);
    ProfileView {
        active: active.map(str::to_string),
        shown: shown.to_string(),
        is_active,
        hint: if is_active { "active" } else { "press to switch" }.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all() -> Vec<String> {
        ["Default", "claude", "gaming"].map(String::from).to_vec()
    }

    #[test]
    fn device_defaults_to_own() {
        assert_eq!(target_device(&ProfileSettings::default(), "sd-1"), "sd-1");
        let s = ProfileSettings { device: " sd-2 ".into(), ..Default::default() };
        assert_eq!(target_device(&s, "sd-1"), "sd-2");
    }

    #[test]
    fn cycle_list_keeps_subset_order_and_drops_missing() {
        let subset = ["gaming", "deleted", "Default"].map(String::from).to_vec();
        assert_eq!(cycle_list(&all(), &subset), vec!["gaming", "Default"]);
        assert_eq!(cycle_list(&all(), &[]), all());
    }

    #[test]
    fn key_view_states() {
        assert!(key_view(Some("gaming"), "gaming").is_active);
        assert_eq!(key_view(Some("Default"), "gaming").hint, "press to switch");
        assert_eq!(key_view(Some("Default"), " ").hint, "set a profile");
    }

    #[test]
    fn dial_view_shows_highlight_or_active() {
        let v = dial_view(Some("Default"), Some("gaming"));
        assert_eq!((v.shown.as_str(), v.is_active), ("gaming", false));
        let v = dial_view(Some("Default"), None);
        assert_eq!((v.shown.as_str(), v.is_active), ("Default", true));
        assert_eq!(dial_view(None, None).shown, "—");
    }
}
```

- [ ] **Step 2: Write `assets/layouts/profile.json`**

```json
{
	"$schema": "https://schemas.elgato.com/streamdeck/plugins/layout.json",
	"id": "com.jfms7s.utilities.profile-layout",
	"items": [
		{
			"key": "active",
			"type": "text",
			"rect": [0, 4, 200, 20],
			"alignment": "center",
			"color": "#d1d5db",
			"value": "",
			"font": { "size": 13, "weight": 500 }
		},
		{
			"key": "candidate",
			"type": "text",
			"rect": [0, 28, 200, 40],
			"alignment": "center",
			"color": "white",
			"value": "--",
			"font": { "size": 24, "weight": 700 }
		},
		{
			"key": "hint",
			"type": "text",
			"rect": [0, 72, 200, 22],
			"alignment": "center",
			"color": "#9ca3af",
			"value": "",
			"font": { "size": 12, "weight": 500 }
		}
	]
}
```

- [ ] **Step 3: Write `src/render/profile.rs` with tests**

```rust
use super::text::shorten;
use super::tile::{self, MUTED_TEXT_COLOR, TEXT_COLOR};
use crate::profile::ProfileView;
use serde_json::{Value, json};

pub const ACTIVE_COLOR: &str = "#22c55e";

pub fn strip_feedback(view: &ProfileView) -> Value {
    let active = view.active.as_deref().unwrap_or("unknown");
    json!({
        "active": format!("active: {}", shorten(active, 20)),
        "candidate": {
            "value": shorten(&view.shown, 16),
            "color": if view.is_active { ACTIVE_COLOR } else { TEXT_COLOR },
        },
        "hint": view.hint,
    })
}

pub fn tile_image(view: &ProfileView) -> String {
    let border = if view.is_active {
        format!(r#"<rect x="3" y="3" width="94" height="94" rx="10" fill="none" stroke="{ACTIVE_COLOR}" stroke-width="5"/>"#)
    } else {
        String::new()
    };
    let svg = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100">{card}{border}{name}{hint}</svg>"#,
        card = tile::card(),
        name = tile::text_line(56.0, 20.0, true, TEXT_COLOR, &shorten(&view.shown, 12)),
        hint = tile::text_line(80.0, 12.0, false, MUTED_TEXT_COLOR, &view.hint),
    );
    tile::data_uri(&svg)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::key_view;

    #[test]
    fn feedback_keys_match_the_shipped_layout() {
        let layout: Value = serde_json::from_str(include_str!("../../assets/layouts/profile.json")).unwrap();
        let keys: Vec<&str> = layout["items"].as_array().unwrap().iter().map(|i| i["key"].as_str().unwrap()).collect();
        for k in strip_feedback(&key_view(Some("a"), "b")).as_object().unwrap().keys() {
            assert!(keys.contains(&k.as_str()), "layout has no item keyed {k}");
        }
    }

    #[test]
    fn active_tile_has_a_border_and_name_is_escaped() {
        let svg = tile::decode(&tile_image(&key_view(Some("<a>"), "<a>")));
        assert!(svg.contains(ACTIVE_COLOR));
        assert!(svg.contains("&lt;a&gt;"));
        let svg = tile::decode(&tile_image(&key_view(Some("x"), "y")));
        assert!(!svg.contains(ACTIVE_COLOR));
    }
}
```

`src/render/mod.rs`: add `pub mod profile;` and:

```rust
pub async fn show_profile(instance: &Instance, view: &crate::profile::ProfileView) -> OpenActionResult<()> {
    if instance.controller == KEYPAD {
        instance.set_title(Some(String::new()), None).await?;
        instance.set_image(Some(profile::tile_image(view)), None).await
    } else {
        instance.set_feedback(&profile::strip_feedback(view)).await
    }
}
```

- [ ] **Step 4: Write `src/actions/profile.rs`**

```rust
use crate::cycle::step_in;
use crate::host;
use crate::opendeck_state::OpenDeckState;
use crate::profile::{ProfileSettings, cycle_list, dial_view, key_view, target_device};
use crate::render::{KEYPAD, show_profile};
use async_trait::async_trait;
use dashmap::DashMap;
use openaction::{Action, Instance, OpenActionResult};
use serde_json::json;
use std::sync::Arc;
use tokio::sync::watch;

struct Shared {
    state: OpenDeckState,
    instances: DashMap<String, ProfileSettings>,
    /// Dial instances: the profile currently highlighted but not yet chosen.
    highlighted: DashMap<String, String>,
}

#[derive(Clone)]
pub struct ProfileAction {
    shared: Arc<Shared>,
}

impl ProfileAction {
    pub fn new(state: OpenDeckState) -> Self {
        Self {
            shared: Arc::new(Shared { state, instances: DashMap::new(), highlighted: DashMap::new() }),
        }
    }

    async fn render(&self, instance: &Instance, settings: &ProfileSettings) -> OpenActionResult<()> {
        let device = target_device(settings, &instance.device_id);
        let active = self.shared.state.active_profile(&device);
        let view = if instance.controller == KEYPAD {
            key_view(active.as_deref(), &settings.profile)
        } else {
            let highlighted = self.shared.highlighted.get(&instance.instance_id).map(|h| h.clone());
            dial_view(active.as_deref(), highlighted.as_deref())
        };
        show_profile(instance, &view).await
    }

    async fn render_all(&self) {
        let entries: Vec<(String, ProfileSettings)> =
            self.shared.instances.iter().map(|e| (e.key().clone(), e.value().clone())).collect();
        for (id, settings) in entries {
            let Some(instance) = openaction::get_instance(id).await else {
                continue;
            };
            if let Err(e) = self.render(&instance, &settings).await {
                log::warn!("profile render failed: {e}");
            }
        }
    }

    pub async fn run_watcher(self, mut changes: watch::Receiver<u64>) {
        while changes.changed().await.is_ok() {
            self.render_all().await;
        }
    }

    async fn switch_to(&self, instance: &Instance, settings: &ProfileSettings, profile: &str) -> OpenActionResult<()> {
        let device = target_device(settings, &instance.device_id);
        if profile.trim().is_empty() {
            return instance.show_alert().await;
        }
        let already = self.shared.state.active_profile(&device).as_deref() == Some(profile);
        if let Err(e) = host::send(host::switch_profile_event(&device, profile)).await {
            log::warn!("switchProfile send failed: {e}");
            return instance.show_alert().await;
        }
        if already {
            return Ok(());
        }
        let state = self.shared.state.clone();
        let expected = profile.to_string();
        let id = instance.instance_id.clone();
        tokio::spawn(async move {
            if !host::wait_for(&expected, || state.active_profile(&device), host::CONFIRM_TIMEOUT).await {
                host::warn_ignored_once();
                if let Some(instance) = openaction::get_instance(id).await {
                    let _ = instance.show_alert().await;
                }
            }
        });
        Ok(())
    }

    async fn commit_highlight(&self, instance: &Instance, settings: &ProfileSettings) -> OpenActionResult<()> {
        let Some((_, profile)) = self.shared.highlighted.remove(&instance.instance_id) else {
            return Ok(());
        };
        self.switch_to(instance, settings, &profile).await?;
        self.render(instance, settings).await
    }

    async fn send_choices(&self, instance: &Instance, settings: &ProfileSettings) -> OpenActionResult<()> {
        let device = target_device(settings, &instance.device_id);
        let mut devices: Vec<String> = openaction::get_connected_devices().await.into_keys().collect();
        devices.sort();
        instance
            .send_to_property_inspector(json!({
                "event": "profileChoices",
                "devices": devices,
                "device": device,
                "profiles": self.shared.state.profiles(&device),
            }))
            .await
    }
}

#[async_trait]
impl Action for ProfileAction {
    const UUID: &'static str = "com.jfms7s.utilities.profile";
    type Settings = ProfileSettings;

    async fn will_appear(&self, instance: &Instance, settings: &ProfileSettings) -> OpenActionResult<()> {
        self.shared.instances.insert(instance.instance_id.clone(), settings.clone());
        self.render(instance, settings).await
    }

    async fn will_disappear(&self, instance: &Instance, _settings: &ProfileSettings) -> OpenActionResult<()> {
        self.shared.instances.remove(&instance.instance_id);
        self.shared.highlighted.remove(&instance.instance_id);
        Ok(())
    }

    async fn did_receive_settings(&self, instance: &Instance, settings: &ProfileSettings) -> OpenActionResult<()> {
        self.shared.instances.insert(instance.instance_id.clone(), settings.clone());
        self.render(instance, settings).await?;
        // The device may have changed, which changes the profile list.
        self.send_choices(instance, settings).await
    }

    async fn key_up(&self, instance: &Instance, settings: &ProfileSettings) -> OpenActionResult<()> {
        self.switch_to(instance, settings, settings.profile.trim()).await
    }

    async fn dial_rotate(&self, instance: &Instance, settings: &ProfileSettings, ticks: i16, _pressed: bool) -> OpenActionResult<()> {
        let device = target_device(settings, &instance.device_id);
        let list = cycle_list(&self.shared.state.profiles(&device), &settings.cycle);
        let current = self
            .shared
            .highlighted
            .get(&instance.instance_id)
            .map(|h| h.clone())
            .or_else(|| self.shared.state.active_profile(&device))
            .unwrap_or_default();
        if let Some(next) = step_in(&list, &current, i64::from(ticks)) {
            self.shared.highlighted.insert(instance.instance_id.clone(), next);
        }
        self.render(instance, settings).await
    }

    async fn dial_up(&self, instance: &Instance, settings: &ProfileSettings) -> OpenActionResult<()> {
        self.commit_highlight(instance, settings).await
    }

    async fn touch_tap(&self, instance: &Instance, settings: &ProfileSettings, _position: (u16, u16), _hold: bool) -> OpenActionResult<()> {
        self.commit_highlight(instance, settings).await
    }

    async fn property_inspector_did_appear(&self, instance: &Instance, settings: &ProfileSettings) -> OpenActionResult<()> {
        self.send_choices(instance, settings).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uuid_is_in_the_manifest() {
        let manifest: serde_json::Value = serde_json::from_str(include_str!("../../assets/manifest.json")).unwrap();
        let uuids: Vec<&str> = manifest["Actions"].as_array().unwrap().iter().map(|a| a["UUID"].as_str().unwrap()).collect();
        assert!(uuids.contains(&<ProfileAction as Action>::UUID));
    }
}
```

`src/actions/mod.rs` add `pub mod profile;`.

- [ ] **Step 5: Add to `assets/manifest.json` `Actions`**

```json
		{
			"UUID": "com.jfms7s.utilities.profile",
			"Name": "Switch Profile",
			"Icon": "icons/icon",
			"Tooltip": "Switch to a profile, or pick one with the dial and press to switch (needs OpenDeck to accept the request - see README)",
			"Controllers": ["Encoder", "Keypad"],
			"PropertyInspectorPath": "propertyInspector/profile.html",
			"States": [{ "Image": "icons/actionDefaultImage" }],
			"Encoder": { "layout": "layouts/profile.json" }
		}
```

- [ ] **Step 6: Write `assets/propertyInspector/profile.html`**

```html
<!doctype html>
<html lang="en">
<head>
	<meta charset="utf-8" />
	<style>
		body { font: 12px system-ui, sans-serif; margin: 0; padding: 8px 12px; }
		label { display: block; margin-top: 10px; font-size: 11px; opacity: 0.8; }
		input, select { width: 100%; box-sizing: border-box; margin-top: 2px; }
		input[type="checkbox"] { width: auto; }
		.hidden { display: none; }
		fieldset { border: 0; margin: 10px 0 0; padding: 0; }
		legend { padding: 0; font-size: 11px; opacity: 0.8; }
		.checks label { display: flex; align-items: center; gap: 4px; margin: 2px 0; font-size: 12px; opacity: 1; }
	</style>
</head>
<body>
	<label for="device">Device (blank = this one)</label>
	<input id="device" list="devices" />
	<datalist id="devices"></datalist>

	<div id="key_only">
		<label for="profile">Profile</label>
		<select id="profile"></select>
	</div>

	<fieldset id="dial_only">
		<legend>Profiles on the dial (none ticked = all)</legend>
		<div class="checks" id="cycle"></div>
	</fieldset>

	<script>
		const byId = (id) => document.getElementById(id);
		let websocket, uuid, settings = {}, profiles = [];

		window.connectOpenActionSocketData = new Promise((resolve) => {
			window.connectOpenActionSocket = (...args) => resolve(args);
			window.connectElgatoStreamDeckSocket = window.connectOpenActionSocket;
		});
		window.connectOpenActionSocketData.then(([inPort, inUUID, inRegisterEvent, inInfo, inActionInfo]) => {
			uuid = inUUID;
			const actionInfo = JSON.parse(inActionInfo);
			const isKey = actionInfo.payload.controller === "Keypad";
			byId("key_only").classList.toggle("hidden", !isKey);
			byId("dial_only").classList.toggle("hidden", isKey);
			settings = actionInfo.payload.settings || {};
			websocket = new WebSocket(`ws://127.0.0.1:${inPort}`);
			websocket.onopen = () => {
				websocket.send(JSON.stringify({ event: inRegisterEvent, uuid: inUUID }));
				render();
			};
			websocket.onmessage = (event) => {
				const message = JSON.parse(event.data);
				if (message.event === "didReceiveSettings") {
					settings = message.payload.settings || {};
					render();
				} else if (message.event === "sendToPropertyInspector" && message.payload?.event === "profileChoices") {
					profiles = message.payload.profiles || [];
					const list = byId("devices");
					list.replaceChildren();
					for (const id of message.payload.devices || []) {
						const option = document.createElement("option");
						option.value = id;
						list.appendChild(option);
					}
					render();
				}
			};
		});

		function render() {
			byId("device").value = settings.device || "";
			const select = byId("profile");
			select.replaceChildren();
			const names = profiles.includes(settings.profile) || !settings.profile ? profiles : [settings.profile, ...profiles];
			for (const name of names) {
				const option = document.createElement("option");
				option.value = name;
				option.textContent = name;
				select.appendChild(option);
			}
			select.value = settings.profile || "";
			const root = byId("cycle");
			root.replaceChildren();
			const chosen = settings.cycle || [];
			for (const name of profiles) {
				const label = document.createElement("label");
				const box = document.createElement("input");
				box.type = "checkbox";
				box.value = name;
				box.checked = chosen.includes(name);
				box.addEventListener("change", send);
				label.append(box, document.createTextNode(name));
				root.appendChild(label);
			}
		}

		function send() {
			settings = {
				device: byId("device").value.trim(),
				profile: byId("profile").value,
				// Keep the user's dial order: previously chosen first, then newly ticked.
				cycle: (() => {
					const ticked = Array.from(document.querySelectorAll("#cycle input:checked")).map((b) => b.value);
					const kept = (settings.cycle || []).filter((p) => ticked.includes(p));
					return [...kept, ...ticked.filter((p) => !kept.includes(p))];
				})(),
			};
			websocket.send(JSON.stringify({ event: "setSettings", context: uuid, payload: settings }));
		}
		byId("device").addEventListener("change", send);
		byId("profile").addEventListener("change", send);
	</script>
</body>
</html>
```

- [ ] **Step 7: Final `src/main.rs`** (all `dead_code` allows gone)

```rust
mod actions;
mod audio;
mod brightness;
mod cycle;
mod host;
mod lenient;
mod opendeck_state;
mod profile;
mod render;

use actions::audio::AudioAction;
use actions::brightness::BrightnessAction;
use actions::profile::ProfileAction;
use audio::backend::{AudioBackend, PactlBackend, spawn_subscriber};
use opendeck_state::OpenDeckState;
use openaction::{OpenActionResult, register_action, run};
use std::sync::Arc;
use tokio::sync::watch;

#[tokio::main]
async fn main() -> OpenActionResult<()> {
    simplelog::SimpleLogger::init(log::LevelFilter::Info, simplelog::Config::default())
        .expect("logger init");

    let (audio_tx, audio_rx) = watch::channel(0u64);
    spawn_subscriber(audio_tx);
    let backend: Arc<dyn AudioBackend> = Arc::new(PactlBackend);
    let audio = AudioAction::new(backend);
    tokio::spawn(audio.clone().run_watcher(audio_rx));
    register_action(audio).await;

    let deck_state = OpenDeckState::discover();
    let (deck_tx, deck_rx) = watch::channel(0u64);
    opendeck_state::spawn_watcher(deck_state.clone(), deck_tx);

    let brightness = BrightnessAction::new(deck_state.clone());
    tokio::spawn(brightness.clone().run_watcher(deck_rx.clone()));
    register_action(brightness).await;

    let profile = ProfileAction::new(deck_state);
    tokio::spawn(profile.clone().run_watcher(deck_rx));
    register_action(profile).await;

    run(std::env::args().collect()).await
}

#[cfg(test)]
mod tests {
    #[test]
    fn manifest_version_matches_cargo() {
        let manifest: serde_json::Value =
            serde_json::from_str(include_str!("../assets/manifest.json")).unwrap();
        assert_eq!(manifest["Version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(
            manifest["CodePathLin"],
            "opendeck-utilities-x86_64-unknown-linux-gnu"
        );
    }

    #[test]
    fn every_manifest_action_is_registered() {
        let manifest: serde_json::Value =
            serde_json::from_str(include_str!("../assets/manifest.json")).unwrap();
        let mut uuids: Vec<&str> = manifest["Actions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a["UUID"].as_str().unwrap())
            .collect();
        uuids.sort();
        assert_eq!(
            uuids,
            vec![
                "com.jfms7s.utilities.audio",
                "com.jfms7s.utilities.brightness",
                "com.jfms7s.utilities.profile",
            ]
        );
    }
}
```

- [ ] **Step 8: Run and verify**

Run: `cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test`
Expected: PASS.

- [ ] **Step 9: Commit**

```bash
git add -A
git commit -m "feat(profile): add the Switch Profile action

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 11: README, install and end-to-end check

**Files:**
- Create: `README.md`

- [ ] **Step 1: Write `README.md`**

````markdown
# OpenDeck Utilities

An [OpenDeck](https://github.com/nekename/OpenDeck) plugin with three
customizable actions for keys and dials:

- **Audio** - volume, mute and device switching for the default output, the
  default input (mic), a specific device, or an application's streams.
  Every gesture is configurable (rotate, press, long-press ≥ ½ s, touch-strip
  tap): toggle mute, volume up/down, set volume, switch to the next device,
  switch to a specific device, push-to-talk. The key or touch strip shows a
  live level, mute state and label, and follows changes made elsewhere.
- **Device Brightness** - dial steps brightness and press toggles between two
  presets; a key can increase, decrease, set or toggle.
- **Switch Profile** - a key jumps to a profile; a dial scrolls a list of
  profiles on the touch strip and switches on press.

## Requirements

- Linux with PipeWire (or PulseAudio) and `pactl` on `PATH`.

## Known limitation: brightness and profiles

Stock OpenDeck only accepts brightness and profile-switch requests from its
own Starter Pack plugin (`src-tauri/src/events/inbound/mod.rs`), and silently
drops them from any other plugin. These two actions are included anyway:
they show the current brightness / active profile (read from OpenDeck's
config files, never written), and if OpenDeck ignores a request the control
flashes an alert and the plugin log says why. They start working as soon as
OpenDeck accepts the events from other plugins.

## Build and install

```bash
cargo build --release --target x86_64-unknown-linux-gnu
node build.mjs x86_64-unknown-linux-gnu
cp -r dist/com.jfms7s.utilities.sdPlugin ~/.config/opendeck/plugins/
```

Then restart OpenDeck.
````

- [ ] **Step 2: Full check**

Run: `cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test`
Expected: PASS.

- [ ] **Step 3: Install and exercise on the XL+**

```bash
cargo build --release --target x86_64-unknown-linux-gnu
node build.mjs x86_64-unknown-linux-gnu
rm -rf ~/.config/opendeck/plugins/com.jfms7s.utilities.sdPlugin
cp -r dist/com.jfms7s.utilities.sdPlugin ~/.config/opendeck/plugins/
```

Restart OpenDeck and check:
1. All three actions appear under **Utilities**.
2. Audio: the Task 7 manual checks still pass; mic on a key with push-to-talk unmutes only while held.
3. Brightness dial shows the current brightness from OpenDeck; rotating on stock OpenDeck flashes an alert after ~1.5 s and the log has one allowlist warning.
4. Switch Profile dial shows the active profile; rotating scrolls through the profiles without switching; pressing on stock OpenDeck flashes an alert. Changing the profile in OpenDeck's UI updates the strip within a second.

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "docs: add the README with the host limitation and install steps

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```
