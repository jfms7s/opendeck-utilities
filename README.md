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

- Linux with PipeWire (or PulseAudio) and `pactl` 16 or newer on `PATH`
  (the plugin reads `pactl -f json` output, added in version 16).
- OpenDeck installed natively (config in `~/.config/opendeck`) or as the
  Flatpak (config in `~/.var/app/me.amankhanna.opendeck/config/opendeck`).

## How brightness and profile switching work

Stock OpenDeck drops brightness and profile-switch requests sent over a
plugin's websocket unless they come from its own Starter Pack plugin
(`src-tauri/src/events/inbound/mod.rs`). This plugin sends them through
OpenDeck's command line instead, `opendeck --process-message <json>`, which
hands the request to the running OpenDeck without that check (OpenDeck
2.14.0 and later). It runs the OpenDeck binary that started the plugin, or
`opendeck` on `PATH`.

Each action re-reads OpenDeck's config files (never written) to confirm the
change. If it doesn't land within 1.5 s, the control flashes an alert and
the plugin log says why.

## Build and install

```bash
cargo build --release --target x86_64-unknown-linux-gnu
node build.mjs x86_64-unknown-linux-gnu
cp -r dist/com.jfms7s.utilities.sdPlugin ~/.config/opendeck/plugins/
```

For the Flatpak, copy it to
`~/.var/app/me.amankhanna.opendeck/config/opendeck/plugins/` instead.

Then restart OpenDeck.
