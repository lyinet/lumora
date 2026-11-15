# Changelog

All notable changes to Lumora are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project uses
[Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added
- **Beautify** — frame captures on a background before sharing: 8 gradient
  presets, a custom solid colour or your own image (PNG / JPEG / HEIC),
  padding, corner radius, drop shadow (blur, opacity, on/off), an optional
  macOS-style window frame (light / dark), fixed aspect ratios (16:9, 9:16,
  4:3, 3:2, 1:1) and auto-balance. Edit it from the ✨ panel in the
  annotation editor with a live Core Animation preview; the last style is
  remembered (Settings) and applied to saved, copied and dragged-out images
  when enabled.

## [0.2.0] — 2026-11-15 (unreleased, not yet hardware-tested)

### Added
- **Annotation editor** — a native, resizable window opened from the Quick
  Access card (Annotate button or double-click the thumbnail) or
  automatically after every capture (Settings → General). Tools: arrow
  (straight or curved, tapered swallowtail head), rectangle / rounded
  rectangle, ellipse, line, smoothed freehand pen, highlighter, text (with
  optional background pill), auto-numbered step badges, pixelate / blur
  redaction (Core Image), spotlight, crop. Six-colour palette plus custom
  colour, three stroke widths, select / move / resize / delete, ⇧ to
  constrain, arrow-key nudging, ⌘Z / ⇧⌘Z, ⌘C copies the flattened image, ⌘S
  saves over the capture, drag-out tile, ⌘[ / ⌘] z-order. Spring-animated
  tool pill and insertion. The card returns with the annotated image.
- **Capture Text (OCR)** — ⌥⇧2 or the menu: select an area, window or
  display; Vision recognises English, Simplified & Traditional Chinese and
  Japanese with language correction; the text (in reading order, CJK
  without stray spaces) goes to the clipboard with a toast preview.
- **Toast HUD** for outcomes and errors (copied, saved, text copied, no text
  found, capture/recording/save failures with a localised reason).
- **Simplified Chinese** localisation of every string, following the system
  language with a Settings override (System Default / English / 简体中文),
  switchable live.
- **App icon** (squircle, aperture motif, indigo→violet), generated from
  an SVG by `scripts/make_icon.py` with a built-in ICNS writer; wired into
  `Info.plist` and `bundle.sh` (optional `ICONUTIL=1` path on macOS).
- Area captures spanning several displays are stitched at the highest
  backing scale.

### Changed
- Capture Window (⌥⇧5) is a true click-to-pick mode: no rubber band,
  desktop clicks ignored.
- Shortcuts another app already holds are reported (toast once, red outline
  and explanation in the Shortcuts tab) instead of only logged.
- Errors no longer beep; settings files gain `general.language`,
  `general.open_editor_after_capture` and `shortcuts.capture_text` with
  defaults (v0.1 files load unchanged).
- New dependencies: `objc2-core-image`, `objc2-vision`.

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
