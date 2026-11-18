# Roadmap

## v0.1.x — make it real (hardware bring-up)

- Run on Apple Silicon + Intel, macOS 14 and 15; fix whatever the type
  checker couldn't catch (event routing across displays, layer geometry,
  window levels over full-screen apps, TCC flows).
- ✅ Microphone on macOS 14 via `AVCaptureSession` (SC microphone is 15+) —
  done in v0.5.
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

## v0.5 — AI combo (built 2026-11-17, awaiting hardware QA)

- ✅ Smart Redaction (chunk 1): on-device detectors for emails, phone
  numbers, Chinese ID and bank cards (checksums + context), API keys and
  tokens, IPv4 addresses, URL secrets and faces; one-click pixelate / blur
  as one undo step (⌘⇧R); optional text-only AI check; optional automatic
  redaction of new screenshots.
- ✅ OCR actions (chunk 2): a Capture Text result panel with Copy,
  Summarize, Translate, As Table, As Code and Checklist; on-device table /
  code / checklist reconstruction that AI refines when enabled; editor AI
  menu gains Extract table / Extract code.
- ✅ Click ripples, click-event log (`.events.json`) and microphone on
  macOS 14 via `AVCaptureSession` (chunk 3).
- ✅ Recording → step-by-step tutorial with Markdown / HTML export and
  optional text-only AI write-up (chunk 4).
- ✅ Subtitles, summary and chapters for recordings: Apple Speech on-device
  or OpenAI-compatible cloud transcription, SRT / WebVTT export (chunk 5).
- ✅ Audit fixes: text panel focus / Esc, settings layout, hidden main menu
  (⌘Z / ⌘C / ⌘W …), bank-card false positives, sidecars moving with
  recordings, scratch-folder cleanup, background Keychain reads.

## v0.5.1 — polish (2026-11-17, awaiting hardware QA)

- ✅ PNG pasteboard type (`public.png` + file URL) instead of TIFF.
- ✅ OCR result cache per capture, shared by auto-redaction, Smart Redact and
  AI Writing; AI Writing only reads text still visible after crop/redaction.
- ✅ Keychain reads off the main thread everywhere (redaction AI check,
  Settings key load).
- ✅ Transcript window: Esc commits an edit instead of closing, batched rows
  for long recordings; clear message + "switch to Cloud" hint when on-device
  speech is unavailable.
- ✅ Async AVFoundation loaders instead of deprecated synchronous calls;
  Quick Access recording buttons clear the duration badge; scrollable,
  top-anchored Settings tabs; Esc on tutorial / welcome windows; capture
  menu items disabled while recording; localised error details.

## v0.6 — history, recording extras, scrolling capture (built 2026-11-18, awaiting hardware QA)

- ✅ Capture history window (⌥⇧H): browse, filter, search (file names +
  on-device OCR text), reopen, copy, reveal and trash past captures and
  their `.lumora` documents; optional 30 / 90-day retention that only
  offers the Trash.
- ✅ Keystroke HUD for recordings (opt-in, Input Monitoring, listen-only
  tap; plain typing and secure input never shown, nothing stored).
- ✅ Microphone list via `AVCaptureDeviceDiscoverySession`.
- ✅ Trim (passthrough) and GIF export from the recording card and history.
- ✅ Burn-in subtitles with size / position / box style kept in Settings.
- ✅ Scrolling capture (⌥⇧S): area screenshots while you scroll, stitched
  with sticky header / footer detection.

## v0.6.2 — real-Mac fixes (2026-11-18)

- ✅ Click ripples drawn at the mirrored y (top-down layer-hosting views for
  every overlay: ripples, keystroke HUD, recording border, shutter,
  countdown).
- ✅ Scrolling capture no longer finishes before you scroll (in-place
  changes skipped, restart before the first scroll, lost-frame run and
  retried frame errors after it, every end logged).

## v0.6.3 — scrolling-capture fixes (2026-11-18)

- ✅ Lumora excluded from every capture as an application (no own panels,
  HUDs, toasts or border in images or recordings).
- ✅ Scrolling control placed outside the captured area; ignores the mouse
  when it can't be.
- ✅ Stitcher matches real pages: tolerant cell matching, fixed top / bottom
  bands and fixed columns, 1–2 px steps, torn frames skipped, stitching up
  and down; per-frame debug log and an end-of-session summary.

## v0.7 — make it real on hardware, signing

- Real-Mac bring-up on Apple Silicon + Intel (macOS 14 & 15) and bug fixes
  for everything v0.1–v0.6 that has only been type-checked (editor,
  beautify orientation / shadow sign, colour panel, `.lumora` ImageIO /
  `openURLs` / LaunchServices registration, recording incl. the macOS 14
  mic path, click overlay and keystroke HUD, AI sheet and Keychain access,
  Capture Text panel focus, tutorial keyframes, Speech permission and
  transcription, history window, trim / GIF and burn-in exports, scrolling
  capture stitching).
- Developer ID signing + notarization in CI (stable code identity, so
  Screen Recording, Input Monitoring and Keychain trust survive updates).
- Keyboard-layout-aware keystroke labels (today: US layout only).
- Scroll-wheel driven scrolling capture (capture on scroll events instead
  of a fixed 150 ms timer).
- Live preview of burned-in subtitles in the subtitles window.

## Later

- Sparkle updates, Homebrew cask.
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
