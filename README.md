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
- OpenDeck installed natively (config in `~/.config/opendeck`). Tested on
  OpenDeck 2.14.0 (native, Fedora). The Flatpak (config in
  `~/.var/app/me.amankhanna.opendeck/config/opendeck`) is supported in the code
  but untested; inside its sandbox `pactl` and `opendeck` may not be on `PATH`.

## How brightness and profile switching work

Stock OpenDeck drops brightness and profile-switch requests sent over a
plugin's websocket unless they come from its own Starter Pack plugin
(`src-tauri/src/events/inbound/mod.rs`). This plugin sends them through
OpenDeck's command line instead, `opendeck --process-message <json>`, which
hands the request to the running OpenDeck without that check (the flag
exists since OpenDeck 2.6.0; tested on 2.14.0). It runs the OpenDeck binary
that started the plugin, or `opendeck` on `PATH`, and logs which one once.

Each action re-reads OpenDeck's config files (never written) to confirm the
change. If it doesn't land within 1.5 s, the control flashes an alert and
the plugin log says why. If those files can't be read at all (config folder
not found, or a format change in a newer OpenDeck), requests are still sent
but can't be confirmed: the plugin logs that once instead of alerting on
every press.

## Installing

Download the latest `.streamDeckPlugin` from
[Releases](https://github.com/jfms7s/opendeck-utilities/releases). Then either
double-click it (if your file manager associates the extension with OpenDeck) or unzip it
into `~/.config/opendeck/plugins/` (Flatpak:
`~/.var/app/me.amankhanna.opendeck/config/opendeck/plugins/`) and restart OpenDeck
(OpenDeck only loads plugins at startup).

Each release also has a `SHA256SUMS` file; check the download with
`sha256sum -c SHA256SUMS` in the folder holding both files.

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
      output to the next device (devices are ordered by their description and wrap
      around). Holding
      the dial while turning it changes the volume and does not switch device on release.
- [ ] Ticking a subset under "Devices to switch between" limits switching to those devices.
      Unplugging a ticked device and then changing the Label keeps it ticked once it is
      plugged back in.
- [ ] With Maximum volume at 100%, rotating up stops at 100%; at 150%, it goes past 100%.
- [ ] Control set to "Default input / mic" shows a mic icon and follows the system's
      default source. With Press set to "Push to talk (hold)", holding the key unmutes
      the mic and releasing mutes it again; switching profile while holding it also
      mutes it again.
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
      otherwise to Preset A. Typing 300 into a value field saves 100, not 0.
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

Last verified: not recorded yet. At each release, replace this line with the date,
the OpenDeck, `pactl` and PipeWire versions and the device the checklist passed on.

## Development

```bash
cargo test --locked                          # unit tests (no live audio or OpenDeck needed)
node --test tests/                           # property-inspector helpers (pi.js)
cargo build --release --locked --target x86_64-unknown-linux-gnu
node build.mjs                               # packages every built target into dist/<uuid>.sdPlugin
```

To try a build, **quit OpenDeck first**: copying over the binary of a running
plugin fails with "Text file busy" and leaves new assets next to the old binary.
Then replace the installed copy (`--delete` also drops files removed from `assets/`)
and start OpenDeck again:

```bash
rsync -a --delete dist/com.jfms7s.utilities.sdPlugin/ \
  ~/.config/opendeck/plugins/com.jfms7s.utilities.sdPlugin/
```

For the Flatpak, the plugins folder is
`~/.var/app/me.amankhanna.opendeck/config/opendeck/plugins/`.

The plugin log (`~/.local/share/opendeck/logs/plugins/`) starts with the plugin's
version, so you can confirm which build the checklist ran against.

Icon sources (SVG) live in `assets/icon-src/`; only the PNGs in `assets/icons/` are
shipped in the plugin bundle.

## Releasing

1. Bump `version` in `Cargo.toml` and `Version` in `assets/manifest.json` together
   (a test and `build.mjs` both fail if they differ), and merge that.
2. Push a `vX.Y.Z` tag matching that version. The release workflow re-runs the
   checks, builds both architectures and creates a **draft** release with the bundle
   and `SHA256SUMS`.
3. Install the draft's bundle, run the smoke-test checklist, update the "Last
   verified" line above, then publish the draft.

## License

MIT — see [LICENSE](LICENSE).
