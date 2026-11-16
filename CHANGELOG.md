# Changelog

All notable changes to Lumora are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project uses
[Semantic Versioning](https://semver.org/).

## [Unreleased]

## [0.5.0] — 2026-11-17 (prerelease, not yet hardware-tested)

### Added
- **Subtitles, summary and chapters** — "Subtitles" on a recording's Quick
  Access card and "Transcribe Recording…" in the menu bar (it becomes
  "Cancel Transcription" while running; a recording with a
  `.transcript.json` reopens it with its edits). Speech is recognised on
  your Mac with Apple Speech (`SFSpeechRecognizer`, strict on-device mode
  optional) or by the AI provider's OpenAI-compatible
  `/audio/transcriptions` endpoint (audio up to 25 MB). A summary and
  chapters follow — by AI when it is on (text only), otherwise locally from
  pauses and clicks. The transcript window (Esc / ⌘W) has editable lines,
  summary and chapters, Export SRT / WebVTT (CJK-aware line wrapping), Copy
  Chapters and Copy Summary; edits are saved back on close. Settings →
  Recording gains engine (on-device / cloud), spoken language, audio source
  (microphone / system audio / both), cloud model and strict on-device rows
  (`[transcribe]` settings section); progress toasts and localised speech
  errors. `NSSpeechRecognitionUsageDescription` added to `Info.plist`.
- **Recording → step-by-step tutorial** — "Make Tutorial" on a recording's
  Quick Access card (shown when it has a click log) and "Make Tutorial from
  Recording…" in the menu bar (open panel for `.mp4` / `.mov`; a recording
  that already has a `.steps.json` reopens with its edits). Steps come from
  `events.json` (`lumora_core::tutorial`): clicks outside the recorded area
  are dropped, double / rapid clicks on the same spot (≤ 0.4 s, ≤ 24 px)
  merge, clicks < 0.15 s apart collapse, at most 40 steps. For each step a
  frame 0.12 s after the click is read with `AVAssetImageGenerator` (zero
  tolerance), a ring is drawn at the click with Core Graphics, the frame is
  cropped around it (aspect-preserving, ≥ 640 pt or half the frame) and
  saved as `<stem>.tutorial/step-NN.png`, and Vision reads a strip around the
  click for an OCR hint. Titles: local templates ("Click “Save”", "Click in
  <window>" / "点击“保存”", "在 <窗口> 中点击") or, with AI on and Settings →
  AI → "Write tutorial steps with AI" (default on), the provider writes
  `{title, detail}` JSON from text only (click kind, app, window title, OCR
  hint — no images) in the output language; a tolerant parser falls back to
  local titles. The tutorial window lists thumbnail + editable title / detail
  + delete per step with Export Markdown…, Export HTML… (one self-contained
  file, base64 images, light / dark) and Copy Markdown. Edits are saved to
  `<stem>.steps.json` (`schema_version` 1).
- **Click ripples** — while recording, a global + local mouse-down monitor
  draws a spring-and-fade ring at each click (left blue, right / ⌃-click
  orange, other buttons purple) on a transparent, click-through, all-Spaces
  overlay per display. The overlay windows are deliberately left out of the
  `SCContentFilter`'s excluded windows so the rings are in the video, while
  the border, stop control and toasts stay excluded. Settings → Recording →
  Show: Click ripples (on by default).
- **Click-event log** — every click is written to `<movie stem>.events.json`
  next to the movie (`schema_version` 1): time since the first video frame
  (host clock, same as the writer's session start), button, click count,
  modifiers, display id, point in display points / pixels, pixel in the video
  (`null` + `inside_recording: false` when outside the recorded area), and
  the app (bundle id, name) and window title under the click from the window
  server list (no Accessibility permission). Pure types and mapping in
  `lumora_core::click_log`.
- **Microphone on macOS 14** — ScreenCaptureKit's microphone capture is
  macOS 15+; on 14 the recorder now runs an `AVCaptureSession` audio input
  (default or selected device, 48 kHz stereo LPCM) on the recorder's sample
  queue, retimes buffers from the session's synchronisation clock to the
  host clock with `CMSyncConvertTime`, and drops audio from before the first
  video frame. macOS 15 keeps the SC path (and honours the chosen device).
  Settings → Recording gains a microphone device popup; mic permission is
  requested before the first mic recording, and a denied permission shows a
  toast and records without the mic.
- **Smart Redaction (智能打码)** — the editor's new Smart Redact button
  (<kbd>⌘</kbd><kbd>⇧</kbd><kbd>R</kbd>) finds personal and secret data and
  covers every hit with a pixelate (or blur) redaction, all as one undo step,
  with a "Redacted N items" toast. Local-first: detectors in `lumora-core`
  run over the on-device Vision OCR boxes — emails, phone numbers (mainland
  China mobiles with separators, international `+…`), Chinese resident ID
  numbers (MOD 11-2 checksum), bank cards (Luhn + issuer prefix), API keys
  and tokens (`sk-`, `ghp_`/`github_pat_`, `AKIA`, Slack `xox?-`, JWTs,
  high-entropy hex/base64), IPv4 addresses and secrets in URL queries — and
  matches are mapped back to sub-rectangles of the OCR boxes with CJK-aware
  glyph widths. Faces come from Vision's face detector.
- Settings → Capture → Smart redaction: per-category toggles, faces,
  pixelate/blur, an optional **AI check** (off by default; sends the
  recognised text lines only, never the image, and needs AI Writing on) and
  **redact new screenshots before copy / save** (off by default; local
  detectors only, redactions stay editable in the `.lumora` document).
- **OCR actions** — Capture Text (<kbd>⌥</kbd><kbd>⇧</kbd><kbd>2</kbd>) still
  copies the text and shows its toast, and now also opens a small floating
  "Recognised Text" panel with Copy Text, Summarize, Translate (English /
  简体中文), As Table, As Code and Checklist. Results stream into the AI
  result sheet (factored out of the editor so it also works as a standalone
  panel) with Copy, Copy as CSV (tables), Stop and Retry. Summarize and
  Translate need AI Writing and are disabled with a hint when it is off.
- **Screenshot → table / code, local-first** — `lumora_core::extract`
  rebuilds tables from OCR boxes (rows by line grouping, columns by x-range
  clustering; Markdown, CSV and TSV), code with indentation restored from
  each line's x offset (character width estimated from the runs, snapped to
  a 2–8 space unit), and checklists from plain lines. These show at once;
  with AI on, new `WritingTask::ExtractTable` / `ExtractCode` / `Checklist`
  prompts refine them (tables get the runs with their x positions; code and
  tables keep their original language). A failed or stopped AI request keeps
  the on-device result.
- Editor AI menu: **Extract table** and **Extract code** (with Copy as CSV
  for tables).
- Hidden main menu: Edit (Undo ⌘Z, Redo ⇧⌘Z, Cut, Copy, Paste, Select All)
  and Window (Close ⌘W, Minimize ⌘M) key equivalents now work in Settings,
  the editor and the tutorial / transcript windows. ⌘Q is deliberately not
  bound, so a focused window can't quit the app and lose an unsaved edit.
- Lumora's scratch folder (`$TMPDIR/Lumora`) is cleaned at launch: entries
  untouched for more than 7 days are removed (never when the folder
  overlaps your save folder; symlinks are not followed).

### Changed
- Capture Text (⌥⇧2) now opens the Recognised Text actions panel after
  copying; turn it off with Settings → Capture → "Show actions panel after
  Capture Text" (`capture.text_actions_panel`, on by default).
- Settings → Capture gains the Smart Redaction rows; Settings → AI gains
  "Write tutorial steps with AI"; Settings → Recording gains click ripples,
  microphone device and the transcription rows.
- New settings keys with defaults — `recording.show_clicks = true`,
  `recording.microphone_device` (system default), `capture.text_actions_panel
  = true`, `[redact]`, `[transcribe]`; older settings files load unchanged.
- Shorter English Settings strings (capture, text recognition and redaction
  notes, "Write steps with AI (text only)", "Redact new screenshots
  automatically"); Chinese strings unchanged.
- `scripts/bundle.sh` strips any pre-release / build suffix from the version
  for `CFBundleShortVersionString` (DMG names keep the full version).
- Copying with beautify off now fills the clipboard immediately; with
  beautify on, right after rendering and before the PNG is encoded and
  written. Quick Access Copy with beautify on renders in the background.

### Fixed
- Saving a temporary recording from its Quick Access card now moves its
  `.events.json` with it, and Delete trashes it, so the click log never
  strands in the temp folder.
- The editor toolbar no longer overflows at the 760 pt minimum window width:
  a pure `layout::toolbar_overflow` decides on every resize which trailing
  items of the tool strip (options, stroke widths, colours, then tools from
  the end) move into a "…" menu, which mirrors their state and actions.
- The Capture Text panel no longer steals keyboard focus when it appears;
  it becomes key only when clicked. Esc / ⌘. first stops a streaming AI
  request, then closes the panel.
- Settings notes, the shortcut warning, the AI base URL, key note, vision
  hint, status and privacy note now wrap within the window instead of
  overflowing; the redaction rows' second column is aligned.
- The tutorial window's summary takes the width the button bar leaves and
  the window can no longer be made too narrow for its buttons.
- The AI result sheet's trailing buttons leave no gap when some are hidden;
  "Copy as CSV" shortens to "CSV" (full title as tooltip) when space is
  tight.
- Hex colours with non-ASCII characters (e.g. in a hand-edited settings
  file) no longer crash; a stray `+` is rejected too.
- Smart Redaction no longer redacts order, tracking, invoice or transaction
  numbers as bank cards: ungrouped digit runs need a matching card network
  prefix and length plus a nearby card keyword, and words like 订单 / 单号 /
  快递 / order / tracking / invoice veto a match.
- Deleting a recording now trashes all its sidecars (`.events.json`,
  `.steps.json`, the `.tutorial/` folder, `.transcript.json`); saving moves
  them with the movie (copy + delete across volumes), rewrites
  `.steps.json` when the name changes, and an open tutorial window follows
  the move.
- Tutorial files with absolute or `..` image paths are sanitised on load
  and on HTML export.
- A plain capture that also writes a `.lumora` document is PNG-encoded only
  once; if writing the file fails, the image is still on the clipboard.
- Tutorial HTML export and Keychain reads for Capture Text, AI Writing and
  tutorials moved off the main thread.

## [0.4.0] — 2026-11-16 (not yet hardware-tested)

### Added
- **AI writing** — optional, off by default. Turn it on in Settings → AI
  and pick a provider: OpenAI, DeepSeek, Qwen (Alibaba Cloud DashScope),
  Moonshot (Kimi), a local Ollama server, or any OpenAI-compatible endpoint
  (base URL, model, temperature, output language — match the screenshot,
  English or 简体中文 — and timeout; "Test Connection" checks the key and
  model). In the annotation editor, the ✨ **AI Writing** button (⌘J) offers
  Write caption, Step guide (follows your numbered step badges), Alt text,
  Polish, Translate (English / 简体中文) and Summarize. Polish and Translate
  work on the selected text annotation, otherwise on the screenshot's
  recognised text. The answer streams into a result sheet with Stop, Retry,
  Copy and Insert as Text (or Replace Text for the selected annotation —
  one undoable edit); problems with the key or settings come with an
  "Open AI Settings" button.
  **Privacy:** text is recognised on-device with Vision; by default only
  that text and your annotation text are sent to the provider you chose.
  The screenshot image is sent only if you turn on "Send screenshot image"
  (for vision models). API keys live in the macOS Keychain
  (`app.lumora.ai`), never in `settings.toml`, and are never logged.

## [0.3.0] — 2026-11-16 (not yet hardware-tested)

### Added
- **Beautify** — frame captures on a background before sharing: 8 gradient
  presets, a custom solid colour or your own image (PNG / JPEG / HEIC),
  padding, corner radius, drop shadow (blur, opacity, on/off), an optional
  macOS-style window frame (light / dark), fixed aspect ratios (16:9, 9:16,
  4:3, 3:2, 1:1) and auto-balance. Edit it from the ✨ panel in the
  annotation editor with a live Core Animation preview; the last style is
  remembered (Settings) and applied to saved, copied and dragged-out images
  when enabled.
- **Editable .lumora documents** — every saved screenshot gets a
  `<name>.lumora` next to its PNG holding the original capture, its
  annotations and (when enabled) the beautify style; editor saves keep it in
  sync. Reopen it from the menu bar (Open…, ⌘O in the menu), by
  double-clicking it in Finder, or with Annotate on its Quick Access card —
  every arrow, label and crop stays editable. Written atomically; skipped
  when "Save to disk" is off; toggle in Settings → General ("Also save an
  editable .lumora document", on by default). Lumora registers the
  `app.lumora.document` type (`.lumora`, `application/x-lumora`) as its
  owner. Damaged or newer-format files open with an explanatory alert.

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

[Unreleased]: https://github.com/lyinet/lumora/compare/v0.5.0...HEAD
[0.5.0]: https://github.com/lyinet/lumora/compare/v0.4.0...v0.5.0
[0.4.0]: https://github.com/lyinet/lumora/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/lyinet/lumora/releases/tag/v0.3.0
