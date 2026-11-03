# OpenDeck Utilities

An [OpenDeck](https://github.com/nekename/OpenDeck) plugin with customizable
actions for keys and dials:

- **Audio** - volume, mute and device switching for the default output, the
  default input (mic), a specific device, an application's streams, or all
  inputs at once ("All inputs": mute and push-to-talk act on every mic, volume
  and switching on the default one; it reads as muted only when every mic is,
  and shows "1/2 live" when some are not).
  Every gesture is configurable (rotate, press, long-press ≥ ½ s, touch-strip
  tap, touch-strip hold): toggle mute, volume up/down, set volume, switch to the
  next device, switch to a specific device, push-to-talk (press only), and for
  the strip tap "left half: volume down · right half: up". A push-to-talk
  control still held when OpenDeck quits or stops the plugin (SIGTERM/SIGINT)
  re-mutes before the plugin exits, given about a second; a plugin that is
  killed outright (SIGKILL) can't, and the mic stays as it was. The key or touch strip shows a
  live level, mute state and label, and follows changes made elsewhere. On a
  mic control, the title shows "● <app>" (or "● 2 apps") while an application
  is recording from that mic, muted or not (Linux only).
- **Device Brightness** - dial steps brightness and press toggles between two
  presets; a strip tap toggles too, or steps down/up on the left/right half if
  chosen; holding the strip toggles. A key can increase, decrease, set or toggle.
  An optional plugin-wide schedule (two local times, each with a brightness)
  dims the deck at night and restores it in the morning, whatever profile is
  shown; a change made in between is kept until the next scheduled time.
- **Switch Profile** - a key jumps to a profile; a dial scrolls a list of
  profiles on the touch strip and switches on press. A strip tap switches to the
  highlighted profile, or goes to the previous/next one on the left/right half if
  chosen; holding the strip switches to the first profile in the dial's list.
- **Do Not Disturb** (keys, Linux only) - silences desktop notifications through
  the notification server's inhibit API (`org.freedesktop.Notifications`, e.g.
  Plasma) and shows "On" whenever notifications are silenced, whoever did it. A
  press turns the plugin's own silencing on or off; when the desktop's own Do Not
  Disturb is on ("by desktop"), a press can't lift it and alerts instead. The
  silencing ends when the plugin exits. Notification servers without the inhibit
  API (GNOME's) show "Unavailable"; on macOS the key shows "Linux only".

## Requirements

**Linux** (x86_64):

- PipeWire (or PulseAudio) and `pactl` 16 or newer on `PATH`
  (the plugin reads `pactl -f json` output, added in version 16).
- OpenDeck installed natively (config in `~/.config/opendeck`), tested on
  OpenDeck 2.14.0 (native, Fedora), or as the Flatpak (config in
  `~/.var/app/me.amankhanna.opendeck/config/opendeck`). Inside the Flatpak the
  plugin runs the host's `pactl` through `flatpak-spawn --host`, so `pactl` must be
  installed on the host and the Flatpak must be allowed to talk to
  `org.freedesktop.Flatpak`; brightness and profile requests run OpenDeck's own
  binary inside the sandbox. The Flatpak path is unit-tested only; its checklist
  item below has not been run yet.

**macOS** (Apple Silicon):

- Nothing to install: audio goes through CoreAudio, and OpenDeck's config is read
  from `~/Library/Application Support/opendeck`.
- Not available on macOS: the **Application** audio target (macOS has no public
  API for another app's volume or output device), volumes above 100 %, and the
  "● <app>" recording hint on mic controls.
- OpenDeck's config changes are picked up as they are written (kqueue, as Linux
  uses inotify), with a 30 s re-check as a safety net.

## How brightness and profile switching work

Stock OpenDeck drops brightness and profile-switch requests sent over a
plugin's websocket unless they come from its own Starter Pack plugin
(`src-tauri/src/events/inbound/mod.rs`). This plugin sends them through
OpenDeck's command line instead, `opendeck --process-message <json>`, which
hands the request to the running OpenDeck without that check (the flag
exists since OpenDeck 2.6.0; tested on 2.14.0). It runs the OpenDeck binary
that started the plugin (on macOS `OpenDeck.app/Contents/MacOS/opendeck`), or
`opendeck` on `PATH`, and logs which one once.

Each action re-reads OpenDeck's config files (never written) to confirm the
change. If it doesn't land within 1.5 s, the control flashes an alert and
the plugin log says why. If those files can't be read at all (config folder
not found, or a format change in a newer OpenDeck), requests are still sent
but can't be confirmed: the plugin logs that once instead of alerting on
every press.

## Installing

Download the latest `.streamDeckPlugin` from
[Releases](https://github.com/jfms7s/opendeck-utilities/releases) and check it
before anything else. Each release carries a build-provenance attestation signed
by GitHub; with the [GitHub CLI](https://cli.github.com/), in the download folder:

```bash
gh attestation verify opendeck-utilities.streamDeckPlugin --repo jfms7s/opendeck-utilities
```

Without `gh`, check it against the release's `SHA256SUMS` file:
`sha256sum -c SHA256SUMS` (macOS: `shasum -a 256 -c SHA256SUMS`) in the folder
holding both files. That only catches a corrupted download: the checksum sits in
the same release as the bundle, so whoever could replace one could replace both.

Then either
double-click it (if your file manager associates the extension with OpenDeck) or unzip it
into OpenDeck's plugins folder and restart OpenDeck (OpenDeck only loads plugins at
startup):

- Linux: `~/.config/opendeck/plugins/` (Flatpak:
  `~/.var/app/me.amankhanna.opendeck/config/opendeck/plugins/`)
- macOS: `~/Library/Application Support/opendeck/plugins/`

On macOS, a bundle unzipped by hand (e.g. in Finder) is marked as downloaded and
Gatekeeper refuses to start the binary. Once the download checked out, clear the
mark on that one executable:

```bash
xattr -d com.apple.quarantine ~/Library/Application\ Support/opendeck/plugins/com.jfms7s.utilities.sdPlugin/opendeck-utilities-aarch64-apple-darwin
```

## Manual smoke-test checklist

Run this in a live OpenDeck + Stream Deck+ session (with PipeWire/PulseAudio running and
at least two output devices), and the Mac list on a Mac, before publishing a release.
It covers the dials the maintainer's deck uses; key modes, the App and specific-device
targets and the display options are covered by the unit tests only.
Record the result per item: pass, fail or **not run** (with why). Each release's draft
notes start with this list, unticked, under an `UNVERIFIED` line: tick or annotate each
item there, fill in the run details and delete the `UNVERIFIED` line before publishing.
The full run (date, OpenDeck, `pactl` and PipeWire or macOS versions, Stream Deck model,
per-item notes) goes in that release's smoke log, `releases/vX.Y.Z-smoke.md` in the
maintainer's project notes (kept outside this repository), which the release notes link.

<!-- smoke-checklist:start (scripts/release-body.mjs copies everything up to the end marker into each draft release) -->

**Audio** (dials; one on the default output, one on the default input)

- [ ] Running `pactl set-sink-volume @DEFAULT_SINK@ 30%` in a terminal updates the
      output dial's strip to 30% without touching it.
- [ ] Rotating the output dial changes the volume by the step (5% by default).
      Pressing the dial and tapping the touch strip both toggle mute ("muted" in red,
      icon struck through).
- [ ] Holding the press for ½ s or longer and releasing switches the default output to
      the next device (ordered by description, wrapping around). Holding the dial
      while turning it changes the volume and does not switch device on release.
- [ ] With Maximum volume at 100%, rotating up stops at 100%; at 150%, it goes past
      100%.
- [ ] Editing only the Label of a new dial keeps its default gestures (press toggles
      mute, long press switches device).
- [ ] The input dial shows a mic icon and follows the system's default source; a press
      toggles its mute. With Press set to "Push to talk (hold)", holding the dial
      unmutes the mic and releasing mutes it again; switching profile while holding it
      also mutes it again.
- [ ] Holding a finger on the output dial's touch strip toggles mute (the
      "Touch-strip hold" default); set it to "Volume up" and a hold raises the
      volume instead, while a short tap still toggles mute. With "Touch-strip tap"
      set to "Left half: volume down · right half: up", a tap on the left half of
      the dial's strip lowers the volume and one on the right half raises it.
- [ ] Opening a mic test page in a browser makes the input dial's title read
      "● <browser>" within a second, also while muted; closing the tab clears it.
      Opening Plasma's volume applet (or pavucontrol) on its input tab does not.
- [ ] With two mics, a control set to "All inputs" mutes both on one press
      (`pactl list short sources` shows every non-monitor source muted, the strip
      shows "muted"); unmuting one in a terminal makes it show "1/2 live"; the next
      press mutes both again.

**Device Brightness** (dial)

- [ ] Rotating steps the brightness up and down and the Stream Deck follows; a quick
      spin settles on the final value. Pressing the dial or tapping the strip toggles
      between the presets.
- [ ] Changing the brightness in OpenDeck's own settings updates every brightness
      control within a second.
- [ ] With "Touch strip tap" set to the half step, tapping the left half of the
      dial's strip lowers the brightness by the step and the right half raises it;
      holding the strip toggles between the presets.
- [ ] A schedule time two minutes ahead (Schedule card) sets the brightness at that
      minute, also with another profile shown; turning the dial afterwards is not
      undone; restarting OpenDeck between the two times re-applies the earlier
      time's value at start-up.

**Switch Profile** (dial)

- [ ] The touch strip shows "active: <profile>". Rotating highlights the next or
      previous profile (wrapping around) with "press to switch"; pressing the dial or
      tapping the strip switches to it.
- [ ] Switching profile from OpenDeck's own UI updates every Switch Profile control.
- [ ] Holding a finger on the strip switches to the first profile in the dial's
      list. With "Touch strip tap" set to the half step, tapping the right half
      switches to the next profile at once and the left half to the previous one.

**Do Not Disturb** (key, Linux)

- [ ] Pressing the key shows "On"; `notify-send test` then shows no popup. Pressing
      again shows "Off" and `notify-send test` pops up.
- [ ] Turning on Plasma's own Do Not Disturb makes the key show "On · by desktop"
      within a second; a press alerts and changes nothing.
- [ ] With the key "On", quitting OpenDeck ends the silencing: `notify-send test`
      pops up again.

**Flatpak OpenDeck** (optional; run when a Flatpak OpenDeck is available, e.g. in a VM)

- [ ] With the bundle unzipped into the Flatpak's plugins folder, the plugin log says
      it runs inside a Flatpak; an Audio dial shows the level and mutes on press,
      and follows a volume change made outside OpenDeck; a brightness dial and a
      Switch Profile dial work without an alert. After quitting OpenDeck, no
      `pactl subscribe` is left running on the host (`pgrep -a pactl`).

On a Mac (Apple Silicon), with the release bundle installed through OpenDeck:

- [ ] `xattr -l` on the installed `opendeck-utilities-aarch64-apple-darwin` shows no
      `com.apple.quarantine`.
- [ ] Unzipped by hand instead: after `gh attestation verify` and the single-file
      `xattr -d` from "Installing", the plugin starts.
- [ ] A dial on the default output changes volume and mutes on press; the level
      follows changes made in Control Center within about a second.
- [ ] Switching to the next output device moves the system output (Sound settings
      shows it); a specific-device control picks its device from the list.
- [ ] The default input mutes and unmutes; push-to-talk unmutes only while held.
- [ ] An output without a volume control (e.g. HDMI) shows 100 % and a press alerts
      instead of crashing.
- [ ] The Audio settings page shows "Application (not supported on macOS)" disabled.
- [ ] With Maximum volume at 100 %, a device at full volume shows a full bar; the
      Audio settings page shows 125 % and 150 % disabled ("max 100% here").
- [ ] Device Brightness and Switch Profile change OpenDeck and confirm without an alert.
- [ ] From a checkout of the release tag, `cargo test --locked -- --ignored --nocapture coreaudio_round_trip`
      passes (it writes the default output's and input's volume to 30 or 70 % and
      back, toggles mute twice and restores both); paste its output into the release notes.

<!-- smoke-checklist:end -->

## Development

```bash
cargo test --locked                          # unit tests (no live audio or OpenDeck needed)
node --test tests/*.test.mjs                 # property-inspector helpers (pi.js)
cargo build --release --locked --target x86_64-unknown-linux-gnu   # macOS: aarch64-apple-darwin
node build.mjs                               # packages every built target into dist/<uuid>.sdPlugin
cargo clippy --target aarch64-apple-darwin --all-targets   # type-checks the macOS code from Linux (rustup target add aarch64-apple-darwin)
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
`~/.var/app/me.amankhanna.opendeck/config/opendeck/plugins/`; on macOS it is
`~/Library/Application Support/opendeck/plugins/`.

The plugin log (`~/.local/share/opendeck/logs/plugins/`; macOS
`~/Library/Logs/opendeck/plugins/`) starts with the plugin's
version, so you can confirm which build the checklist ran against.

Icon sources (SVG) live in `assets/icon-src/`; only the PNGs in `assets/icons/` are
shipped in the plugin bundle.

## Releasing

1. Bump `version` in `Cargo.toml` and `Version` in `assets/manifest.json` together
   (a test and `build.mjs` both fail if they differ), and merge that.
2. Push a `vX.Y.Z` tag matching that version. The release workflow re-runs the
   checks, builds the Linux x86_64 binary and the macOS one (on a macOS runner) and
   creates a **draft** release with the bundle, `SHA256SUMS` and a build-provenance
   attestation (check it with `gh attestation verify` on the draft's bundle). The draft's notes
   start with the smoke-test checklist above, unticked, under an `UNVERIFIED` line
   (`scripts/release-body.mjs`), followed by the generated change list.
3. Install the draft's bundle on Linux and on a Mac, run the smoke-test checklist, write
   the release's smoke log, tick or annotate every item in the draft's notes, link the
   log and delete the `UNVERIFIED` line. The release notes are the record; a draft whose
   notes still say `UNVERIFIED` is not published. Then publish with
   `scripts/publish-release.sh vX.Y.Z`, which refuses while the notes say `UNVERIFIED`
   or while GitHub's immutable releases setting is off. Ansible installs the latest
   *published* release, so publishing is what deploys it. Published releases are
   immutable: the assets and the tag are frozen, so a broken release is fixed with a new
   version, never by replacing an asset (`gh api repos/jfms7s/opendeck-utilities/immutable-releases`
   must report `"enabled":true`; a release published while it is off stays mutable).

## License

MIT — see [LICENSE](LICENSE).
