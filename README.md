# Lumora

**Screenshots and screen recordings that feel like they shipped with macOS.**

Lumora is a menu-bar app for macOS 14+ that captures an area, a window or the
whole screen, records video with system audio, and hands you the result in a
floating Quick Access card — with fluid, spring-based motion everywhere. It's
written in Rust on top of native AppKit, Core Animation and ScreenCaptureKit:
no web views, no Electron, a few MB on disk, instant to launch.

> Status: **v0.1, pre-release.** Everything type-checks for Apple Silicon and
> Intel, and the platform-independent core is unit-tested, but v0.1 has not
> been run on real hardware yet. See [STATUS.md](STATUS.md).

## Highlights

- **Area capture on a frozen frame** — the screen holds still while you
  select, so video and animations can't change what you get. A dimmed
  backdrop with a crisp cutout, crosshair guides, a live `W × H` pill, and a
  pixel loupe with the hex colour under the cursor (press **C** to copy it).
- **Window snapping** — hover to highlight the frontmost window, click to
  capture it cleanly (with its shadow, even if something covers it). While
  dragging, edges stick to nearby window and display edges (hold ⌥ to
  disable).
- **Keyboard precision** — Space to move the selection while dragging, ⇧ to
  lock the aspect ratio, arrows to nudge 1 pt (⇧ = 10 pt, ⌥ = resize), Enter
  or double-click to confirm, Esc to cancel.
- **Retina-correct output** — PNGs at native pixel density with DPI metadata
  so they display at the right size everywhere, named
  `Lumora 2026-11-15 at 04.30.12.png`, copied to the clipboard.
- **Quick Access** — a glass card slides in bottom-left on a spring. Hover for
  Copy / Save / Pin / Delete, drag it straight into Slack, Mail or Finder.
  Stacks up to three, auto-hides after 6 s unless you're hovering.
- **Pin to screen** — float any capture above your windows; scroll to zoom,
  ⌥-scroll to fade, double-click or Esc to close.
- **Screen recording** — area or full screen to H.264 or HEVC MP4 at 30/60
  fps, cursor on/off, system audio, microphone (macOS 15+), a 3-2-1 countdown
  ring, a red timer in the menu bar, a floating stop button, and a
  marching-ants border that never appears in the video.

## Default shortcuts

| Action | Shortcut |
|---|---|
| Capture Area | ⌥⇧4 |
| Capture Fullscreen | ⌥⇧3 |
| Capture Window | ⌥⇧5 |
| Record Screen (start/stop) | ⌥⇧R |
| Open Captures Folder | ⌥⇧O |

Option-Shift avoids clashing with macOS's own ⇧⌘3/4/5. Rebind them in
Settings → Shortcuts (click a field and press keys).

## Building

Requirements: macOS 14+, Xcode command-line tools, Rust stable (1.80+).

```sh
# Run a debug build from the terminal (permission prompts will name your terminal)
cargo run -p lumora

# Universal (arm64 + x86_64) app bundle + DMG, ad-hoc signed
brew install create-dmg   # optional; falls back to hdiutil
scripts/bundle.sh
open dist/Lumora.app

# Signed + notarized release
SIGN_IDENTITY="Developer ID Application: Your Name (TEAMID)" \
NOTARY_PROFILE=lumora-notary scripts/bundle.sh
```

Screen Recording permission is tied to the app's code signature. Ad-hoc
builds get a new identity each time, so macOS will ask again after every
rebuild — use a stable signing identity for day-to-day development.

### Developing on Linux / CI

The pure-logic crates build and test anywhere:

```sh
cargo test -p lumora-core -p lumora-ui     # geometry, springs, selection, settings, hotkeys, tokens
```

With the Apple targets installed you can type-check the whole app from any
host:

```sh
rustup target add aarch64-apple-darwin x86_64-apple-darwin
cargo check --workspace --target aarch64-apple-darwin
cargo clippy --workspace --all-targets --target x86_64-apple-darwin -- -D warnings
```

## Settings

Stored as TOML at `~/Library/Application Support/Lumora/settings.toml`.
Missing keys fall back to defaults, unknown keys are ignored, and a corrupt
file is moved aside to `settings.toml.bak`. Logs go to stderr; set
`LUMORA_LOG=debug` for more.

## Project docs

- [ARCHITECTURE.md](ARCHITECTURE.md) — crates, threading, data flow
- [DESIGN.md](DESIGN.md) — design tokens and motion specs
- [ROADMAP.md](ROADMAP.md) — what's next
- [STATUS.md](STATUS.md) — what compiles, what's tested, known risks
- [CHANGELOG.md](CHANGELOG.md)

## License

MIT — see [LICENSE](LICENSE).
