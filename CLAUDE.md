# CLAUDE.md

OpenDeck plugin (`com.jfms7s.utilities`, Rust on the `openaction` crate) with three
actions: Audio (`pactl`), Device Brightness and Switch Profile (`opendeck
--process-message`). Design docs, plans, reviews and issue write-ups live in the
Obsidian vault at `~/git/obsidian-vault/personal/projects/opendeck-utilities/`, not
in this repo.

## Checks

```bash
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
node --test tests/                 # property-inspector helpers
cargo build --release --locked --target x86_64-unknown-linux-gnu && node build.mjs
```

## Rules

- `src/audio/backend.rs` is the only code that runs `pactl`. Every device name
  goes through `is_safe_name`, and every `pactl` runs in the C locale
  (`pactl_command`). Long-lived children get `die_with_parent`.
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
