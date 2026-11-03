# CLAUDE.md

OpenDeck plugin (`com.jfms7s.utilities`, Rust on the `openaction` crate; Linux
x86_64 and Apple Silicon macOS) with five actions: Audio (`pactl` on Linux,
CoreAudio on macOS), Device Brightness and Switch Profile (`opendeck
--process-message`), Do Not Disturb and Media (session D-Bus, Linux only).
Design docs, plans, reviews and issue write-ups live in the Obsidian vault at
`~/git/obsidian-vault/personal/projects/opendeck-utilities/`, not in this repo.

## Checks

```bash
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
node --test tests/*.test.mjs       # property-inspector helpers
cargo clippy --target aarch64-apple-darwin --all-targets --locked -- -D warnings  # macOS code, type-checked from Linux
cargo build --release --locked --target x86_64-unknown-linux-gnu && node build.mjs
```

## Rules

- `src/audio/pactl/` is the only code that runs `pactl`. Every device name
  goes through `is_safe_name`, and every `pactl` runs in the C locale
  (`pactl_command`). Long-lived children get `die_with_parent` (Linux-only:
  `PR_SET_PDEATHSIG`; a no-op elsewhere).
- `src/audio/coreaudio/backend.rs` (macOS only) is the only code that calls CoreAudio; pure
  conversions live in `src/audio/coreaudio/hal.rs` so Linux tests cover them. macOS has no
  per-app audio (`Snapshot::caps`, `Capabilities::per_app`). Linux-only code is `cfg(target_os = "linux")`,
  macOS-only code `cfg(target_os = "macos")`; keep both targets clippy-clean.
- `src/audio/backend.rs` (the `AudioBackend` contract) and `src/audio/model.rs` (the
  snapshot types) are backend-neutral: no backend's code goes there.
- `src/notifications/dbus.rs` (Do Not Disturb) and `src/media/mpris.rs` (MPRIS
  players) are the only code that talks to the session bus; every call has a
  timeout and none is awaited on the event loop. All D-Bus code and `zbus` are
  `cfg(target_os = "linux")`. Their tests run a private `dbus-daemon`
  (`src/test_bus.rs`, fakes next to each adapter), never the real bus.
- Platform-specific helpers live in `src/platform.rs` (backends in their own folders);
  logic that must be tested for both platforms takes the platform as a parameter
  (`platform::MACOS`).
- OpenDeck's config files are read, never written (`src/opendeck_state.rs`).
  Device ids are path components: check `is_safe_device_id` before joining.
- Anything handed to another program is validated first: audio targets must exist
  in the current snapshot, a profile switch must name a profile OpenDeck has for a
  safe device id.
- Host requests (brightness, profile) go through `src/dispatch.rs`: never await
  `opendeck` on the event loop, and keep "state unreadable" (log once) separate from
  "OpenDeck ignored it" (alert + `warn_ignored_once`).
- Settings are read field by field (`src/lenient.rs`): a bad field keeps the
  struct's documented default and numbers are clamped. Defaults, bounds and option
  values in `assets/propertyInspector/*.html` must match the Rust types;
  `src/pi_pages.rs` checks this.
- Action handlers stay thin: each `Action` impl turns the `Instance` into a
  `Control` and calls methods that reach the deck only through `ui::Ui` and OpenDeck
  only through `host::Host`, so they run under test with the fakes.
- Feature layout: a small feature is one pure module (`brightness.rs`, `profile.rs`)
  plus its action; it becomes a package (like `audio/`) once it outgrows that.
  View-model types live in `render/`.
- `Cargo.toml` and `assets/manifest.json` versions move together; binary names come
  from the manifest's `CodePaths`.
- `assets/propertyInspector/pi.css` is shared byte-for-byte with the other jfms7s
  OpenDeck plugins; page-specific rules go in the page.

## Scope (frozen surfaces)

Ruled 2026-11-01 after the 2026-10-07 feature-fit review. Frozen means code and tests
stay and bugs get fixed, but nothing is added: no new slots, operations, targets,
options or key modes without lifting the freeze here first (owner decision). Nothing
is cut for being unplaced: the plugin is public.

- Audio gesture matrix (`src/audio/settings.rs` slots/operations, `audio.html`
  gestures): four slots (press, long press, strip tap, strip hold) since the
  owner-approved strip hold and half taps landed; the strip tap modes of
  Brightness and Switch Profile likewise.
- Audio targets (`src/audio/target.rs`): output, input, specific device, App.
  Exception: "All inputs" (owner-approved), the last new target.
- Operations and settings no placed control uses (volume up/down, set volume, switch
  to device, rotate "switch device"/"nothing", step, maximum volume, device subset,
  label, accent, show percent): no new display options. A state the view shows
  without a new setting (e.g. "an app is recording") is not an option.
- Key modes: Brightness key operations, Switch Profile key and device override,
  profile subset, the key tile renderers.
- macOS: no new capability until the Mac checklist has run once and a release with
  that record is published. Bugs and performance fixes may proceed; features that need
  no new CoreAudio code are not macOS work.

New actions the owner approves (the Power Profile port) are outside this list.
