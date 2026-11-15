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

## v0.3 — beautify + editable documents (built 2026-11-16, awaiting hardware QA)

- ✅ Beautify: gradient presets, solid colour or custom image background,
  padding, corner radius, drop shadow, macOS-style window frame, fixed
  aspect ratios, auto-balance; ✨ editor panel with live preview; applied to
  saved / copied / dragged-out images when enabled.
- ✅ Editable `.lumora` documents next to every saved PNG (original capture
  + annotations + beautify style), versioned schema with migrations,
  reopen via Open… / Finder double-click / Annotate; registered
  `app.lumora.document` UTI.

## v0.4 — AI Writing (built 2026-11-16, awaiting hardware QA)

- ✅ Optional AI writing, off by default: OpenAI, DeepSeek, Qwen
  (DashScope), Moonshot (Kimi), local Ollama or any OpenAI-compatible
  endpoint; Settings → AI tab with model, temperature, output language,
  timeout and "Test Connection"; API keys in the macOS Keychain.
- ✅ Editor ✨ **AI Writing** menu (⌘J): caption, step guide, alt text,
  polish, translate (English / 简体中文), summarize; streaming result sheet
  with Stop / Retry / Copy / Insert as Text / Replace Text.
- ✅ Privacy default: only on-device OCR text and annotation text are sent;
  the screenshot image only with "Send screenshot image" turned on.

## v0.5 — real-Mac bring-up, history, recording extras

- Real-Mac bring-up on Apple Silicon + Intel (macOS 14 & 15) and bug fixes
  for everything v0.1–v0.4 that has only been type-checked (editor,
  beautify orientation / shadow sign, colour panel, `.lumora` ImageIO /
  `openURLs` / LaunchServices registration, recording, AI sheet and
  Keychain access).
- Capture history window (⌥⇧H): browse, reopen, copy and delete past
  captures and their `.lumora` documents.
- Recording extras: click ripples, keystroke HUD, microphone on macOS 14
  (`AVCaptureSession`), GIF export and trim on the card.
- Scrolling capture.
- ✅ OCR actions: a Capture Text result panel with Copy, Summarize,
  Translate, As Table, As Code and Checklist; on-device table / code /
  checklist reconstruction that AI refines when enabled (chunk 2).
- ✅ Smart Redaction (chunk 1).

## Later

- Developer ID signing + notarization in CI, Sparkle updates, Homebrew cask.
- Editor polish: arrowhead styles, opacity, multiply-blend highlighter,
  magnifier tool, shape fill, emoji stamps; self-timer; capture previous
  area.
- Recording: pause/resume, webcam bubble, per-app audio selection, 4K/HDR,
  recordings spanning several displays.
- Share: cloud links (bring-your-own S3/R2 first), Shortcuts.app and
  URL-scheme automation.
- ProMotion-synchronised springs (`NSScreen.displayLink`), os_log
  diagnostics and opt-in crash reports.
- Mac App Store variant (sandboxed: security-scoped bookmarks for the save
  folder), more localisations (ja, zh-Hant, de, fr, es — zh-Hans shipped
  in v0.2).
