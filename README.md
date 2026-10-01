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

## Installing

Download the latest `.streamDeckPlugin` from
[Releases](https://github.com/jfms7s/opendeck-utilities/releases). Then either
double-click it (if your file manager associates the extension with OpenDeck) or unzip it
into `~/.config/opendeck/plugins/` (Flatpak:
`~/.var/app/me.amankhanna.opendeck/config/opendeck/plugins/`) and restart OpenDeck
(OpenDeck only loads plugins at startup).

## Manual smoke-test checklist

Run this in a live OpenDeck + Stream Deck session (with PipeWire/PulseAudio running and
at least two output devices) before cutting a release:

**Audio**

- [ ] A new Audio key shows a speaker icon, the default output's name and its volume
      as a percentage. Pressing it shows "muted" in red with the icon struck through;
      pressing again unmutes.
- [ ] Running `pactl set-sink-volume @DEFAULT_SINK@ 30%` in a terminal updates the key
      to 30% without touching it.
- [ ] On a dial, rotating changes the volume by the step (5% by default). Pressing the
      dial and tapping the touch strip both toggle mute.
- [ ] On a dial, holding the press for ½ s or longer and releasing switches the default
      output to the next device (devices are ordered by name and wrap around). Holding
      the dial while turning it changes the volume and does not switch device on release.
- [ ] Ticking a subset under "Devices to switch between" limits switching to those devices.
- [ ] With Maximum volume at 100%, rotating up stops at 100%; at 150%, it goes past 100%.
- [ ] Control set to "Default input / mic" shows a mic icon and follows the system's
      default source. With Press set to "Push to talk (hold)", holding the key unmutes
      the mic and releasing mutes it again.
- [ ] Control set to "Application" offers the apps currently playing audio. A playing
      app shows its initial letter and volume; rotating or pressing changes only that
      app. When the app stops playing, the control shows "not playing" in grey and a
      press flashes an alert.
- [ ] Control set to "Specific output device" with a device that isn't present shows
      "unavailable".
- [ ] A gesture set to "Set volume to…" with 25 sets the volume to 25%. A gesture set
      to "Switch to device…" with another output makes that the default output.
- [ ] Setting a Label, changing the Accent colour and unticking "Show percent" change
      the title, the bar colour and hide the percentage respectively.

**Device Brightness**

- [ ] A new Device Brightness key shows a sun icon, "Brightness" and OpenDeck's current
      brightness. Pressing it raises the brightness by the step (5%) and the Stream Deck
      gets brighter.
- [ ] Key press set to "Decrease by step", "Set to value" (e.g. 30) and "Toggle between
      presets" each do that on press; toggling goes to Preset B when at Preset A,
      otherwise to Preset A.
- [ ] On a dial, rotating steps the brightness up and down; a quick spin settles on the
      final value. Pressing the dial or tapping the strip toggles between the presets.
- [ ] Changing the brightness in OpenDeck's own settings updates every brightness
      control within a second.

**Switch Profile**

- [ ] On a Switch Profile key, the Profile list shows the device's profiles. With none
      picked, the key shows "—" / "set a profile" and a press flashes an alert.
- [ ] After picking a profile, the key shows its name and "press to switch"; pressing
      it switches the Stream Deck to that profile. A key showing the active profile has
      a green border and "active".
- [ ] Entering another connected device's id in "Device" makes the key switch that
      device's profile instead.
- [ ] On a dial, the touch strip shows "active: <profile>". Rotating highlights the next
      or previous profile (wrapping around) with "press to switch"; pressing the dial or
      tapping the strip switches to it.
- [ ] Ticking profiles under "Profiles on the dial" limits the dial to those profiles,
      in the order they were ticked.
- [ ] Switching profile from OpenDeck's own UI updates every Switch Profile control.

## Development

```bash
cargo test                                   # unit tests (no live audio or OpenDeck needed)
cargo build --release --target <triple>
node build.mjs <triple>                      # assembles dist/<uuid>.sdPlugin
cp -r dist/com.jfms7s.utilities.sdPlugin ~/.config/opendeck/plugins/
# restart OpenDeck, then work through the smoke-test checklist above
```

For the Flatpak, copy it to
`~/.var/app/me.amankhanna.opendeck/config/opendeck/plugins/` instead.

Icon sources (SVG) live in `assets/icon-src/`; only the PNGs in `assets/icons/` are
shipped in the plugin bundle.

## License

MIT — see [LICENSE](LICENSE).
