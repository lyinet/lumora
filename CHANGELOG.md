# Changelog

All notable changes to Lumora are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project uses
[Semantic Versioning](https://semver.org/).

## [0.1.0] — 2026-11-15 (unreleased, not yet hardware-tested)

### Added
- Menu-bar app (Accessory policy, code-drawn template icon) with Capture
  Area / Fullscreen / Window, Record Screen, Open Folder, Settings, Quit.
- Global shortcuts via Carbon hotkeys (⌥⇧4 / ⌥⇧3 / ⌥⇧5 / ⌥⇧R / ⌥⇧O),
  rebindable with conflict detection against each other and macOS.
- Area overlay on frozen display frames: dim + cutout, crosshair, size pill,
  pixel loupe with hex readout and C-to-copy, hover window highlight with
  click-to-capture, edge snapping, Space-move, ⇧ aspect lock, arrow nudging,
  spring-animated selection and highlight.
- Fullscreen and window capture via `SCScreenshotManager`.
- Retina PNG output with DPI metadata, template-based file names, clipboard.
- Shutter flash and fly-to-card animation.
- Quick Access cards (glass, spring slide-in, hover actions, drag-out,
  auto-dismiss, stack of three with reflow).
- Pin-to-screen windows (zoom, opacity, drag, Esc/double-click to close).
- Screen recording (`SCStream` → `AVAssetWriter`, H.264/HEVC, 30/60 fps,
  cursor, system audio, microphone on macOS 15+), countdown ring, menu-bar
  timer, floating stop control, excluded marching-ants border.
- Settings window (General, Shortcuts, Capture, Recording) persisted to TOML;
  launch at login via `SMAppService`.
- Screen Recording onboarding with live permission polling and relaunch.
- Universal bundle script (lipo, codesign, notarytool, create-dmg) and CI.
