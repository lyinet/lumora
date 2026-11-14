# Roadmap

## v0.1.x — make it real (hardware bring-up)

- Run on Apple Silicon + Intel, macOS 14 and 15; fix whatever the type
  checker couldn't catch (event routing across displays, layer geometry,
  window levels over full-screen apps, TCC flows).
- App icon (`.icns`) generated from the code-drawn mark at bundle time.
- Microphone on macOS 14 via `AVCaptureSession` (SC microphone is 15+).
- Replace the 120 Hz `NSTimer` with `NSScreen.displayLink` (macOS 14) for
  ProMotion-synchronised window springs.
- Crash/diagnostics: os_log subsystem, opt-in crash reports.
- Window-only mode that disables rubber-band selection (v0.1 window mode =
  forced hover picking).
- Selections spanning several displays (v0.1 clips to the display under the
  selection's centre).

## v0.2 — annotate

- Annotation editor: arrows, shapes, text, highlighter, blur/pixelate,
  numbered steps, crop; spring-animated tool palette.
- Background/padding presets ("beautify") with rounded corners and shadow.
- Scrolling capture; self-timer; capture previous area.
- OCR (Vision) — copy text from any region.

## v0.3 — record like a pro

- Pause/resume, trim on the card, GIF export, click & keystroke overlays,
  webcam bubble, per-app audio selection, 4K/HDR options.

## v0.4 — share

- Cloud links (bring-your-own S3/R2 first), history browser, Shortcuts.app
  and URL-scheme automation, Sparkle updates for the direct build.

## Later

- Mac App Store variant (sandboxed: security-scoped bookmarks for the save
  folder), localisation (zh-Hans first, then ja, de, fr, es).
