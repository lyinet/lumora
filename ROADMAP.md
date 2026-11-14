# Roadmap

## v0.1.x — make it real (hardware bring-up)

- Run on Apple Silicon + Intel, macOS 14 and 15; fix whatever the type
  checker couldn't catch (event routing across displays, layer geometry,
  window levels over full-screen apps, TCC flows).
- Microphone on macOS 14 via `AVCaptureSession` (SC microphone is 15+).
- Replace the 120 Hz `NSTimer` with `NSScreen.displayLink` (macOS 14) for
  ProMotion-synchronised window springs.
- Crash/diagnostics: os_log subsystem, opt-in crash reports.
- Recordings spanning several displays (screenshots stitch since v0.2;
  a recording is still one display per `SCStream`).

## v0.2 — annotate (built 2026-11-15, awaiting hardware QA)

- ✅ Annotation editor: arrows, shapes, text, highlighter, blur/pixelate,
  numbered steps, spotlight, crop; spring-animated tool palette.
- ✅ OCR (Vision) — copy text from any region (en, zh-Hans, zh-Hant, ja).
- ✅ Toast HUD; shortcut conflicts surfaced in Settings.
- ✅ zh-Hans localisation; app icon; click-to-pick window mode; stitched
  multi-display area captures.

## v0.2.x — polish the editor

- Hardware QA of the editor (text field metrics vs export, Retina
  redaction alignment, colour panel focus in an accessory app).
- Re-open past captures in the editor with editable annotations (persist the
  document next to the PNG).
- Arrowhead styles, opacity, multiply-blend highlighter, magnifier tool,
  shape fill, emoji stamps.
- Background/padding presets ("beautify") with rounded corners and shadow.
- Scrolling capture; self-timer; capture previous area.

## v0.3 — record like a pro

- Pause/resume, trim on the card, GIF export, click & keystroke overlays,
  webcam bubble, per-app audio selection, 4K/HDR options.

## v0.4 — share

- Cloud links (bring-your-own S3/R2 first), history browser, Shortcuts.app
  and URL-scheme automation, Sparkle updates for the direct build.

## Later

- Mac App Store variant (sandboxed: security-scoped bookmarks for the save
  folder), more localisations (ja, zh-Hant, de, fr, es — zh-Hans shipped
  in v0.2).
