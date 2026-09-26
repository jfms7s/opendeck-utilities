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
