# Architecture

Lumora is a Cargo workspace of five crates with a strict dependency
direction. Logic that doesn't need the OS lives in the bottom crate and is
tested on every host; every crate above it is a thin, typed layer over Apple
frameworks.

```
apps/lumora        binary: NSApplication, status item, hidden main menu, hotkeys,
   │               App state, flows (capture, record, OCR, editor, redaction,
   │               tutorial, transcription, AI, history, trim / GIF, burn-in,
   │               scrolling capture)
   │
   ├── crates/lumora-ui        AppKit + Core Animation: overlay, Quick Access, pins,
   │      │                    shutter, countdown, recording HUD, click-ripple overlay,
   │      │                    settings (incl. AI + redaction rows), onboarding,
   │      │                    annotation editor + beautify panel, AI result sheet,
   │      │                    Capture Text panel, tutorial + transcript windows, toasts,
   │      │                    history + trim windows, keystroke HUD, scroll control
   │      │
   ├──────┼── crates/lumora-ai        blocking OpenAI-compatible clients (ureq + rustls):
   │      │                           chat streaming + cancellation, /audio/transcriptions
   │      │                           (multipart), Keychain key store
   │      │
   ├──────┴── crates/lumora-capture   ScreenCaptureKit, ImageIO, AVFoundation (writer,
   │                 │                AVCaptureSession mic, keyframes, audio export),
   │                 │                Vision OCR + faces, Apple Speech, pasteboard,
   │                 │                multi-display stitching, beautify + tutorial renderers,
   │                 │                trim / GIF export, subtitle burn-in, keystroke event tap
   │                 │
   └─────────────────┴── crates/lumora-core   pure Rust: geometry, springs, selection
                                              state machine, snapping, settings, file
                                              names, hotkey parsing, layout, recording FSM,
                                              annotation document + editor FSM, i18n,
                                              OCR text assembly, stitch planning,
                                              beautify model, .lumora document format,
                                              ai (settings, prompts, SSE parsing, errors),
                                              redact, extract, click_log, tutorial,
                                              transcript (+ burn-in layout), cleanup,
                                              history, clip, keystroke, scroll_capture
```
| Crate | macOS-only? | Tests | What it owns |
|---|---|---|---|
| `lumora-core` | no | 426 unit tests | `history` (capture-history index: scan + template name matcher, versioned `history.json` with migrations, merge/refresh, retention, search/filter, thumbnail keys + pruning, grid layout), `ai` (provider presets, `AiSettings`, prompt building per writing task incl. extract table / code / checklist, SSE/stream parsing, `AiError` with localised messages, settings-form helpers, `KeyStore` trait, local OCR-action results), `redact` (detectors for emails, phones, Chinese ID / bank cards, keys and tokens, IPv4, URL secrets; CJK-aware glyph-width mapping of hits onto OCR boxes; AI-check prompt + parser), `extract` (table / code / checklist recovery from OCR boxes; Markdown, CSV, TSV), `click_log` (`.events.json` sidecar: timing, display → video pixel mapping), `tutorial` (steps from a click log, merge / collapse rules, crop and OCR rects, local titles, AI prompt + tolerant parser, `.steps.json`, Markdown / HTML export, safe relative paths), `transcript` (segments, `.transcript.json`, CJK-aware SRT / WebVTT, chapters and summary with local fallback; `transcript::burn` burn-in layout: ≤2-line cues, sub-cues, flipped y, opacity keyframes), `clip` (`TrimRange`, sidecar time shift, `GifPlan` size estimate), `keystroke` (key code → label, ⌃⌥⇧⌘ order, show-only-shortcuts rule, repeat merging), `cleanup` (stale `$TMPDIR/Lumora` entries), `beautify` (style model, presets, layout, auto-balance), `lumora_doc` (`.lumora` container, DTO layer, schema migrations), `annotate` (paths, shapes, display list, command-pattern document, editor FSM), `i18n` (en + zh-Hans table, language resolution), `ocr` (reading-order assembly), `stitch` (multi-display plans), `scroll_capture` (scrolling-capture stitcher: row-hash offset search, sticky header/footer bands, height cap), `geometry` (Rect/Point/Size, handles, pixel alignment, Quartz⇄AppKit flip), `spring` (closed-form damped oscillator + retargetable animator), `selection` (overlay interaction FSM), `snap` (window hit-testing, edge snapping), `settings` (serde/TOML model with validation), `filename` (templates, unique paths), `hotkey` (parse/format/Carbon codes/conflicts), `layout` (card stack, pill, loupe, fly transform), `recording` (lifecycle FSM), `color`, `format`, `action` |
| `lumora-capture` | yes (error type and `words` are portable) | 6 (+1 mac) | `content` (SCShareableContent → displays + z-ordered windows), `screenshot` (SCScreenshotManager), `image` (crop, stitch, pixel reads, BGRA buffers for scrolling capture, PNG+DPI via ImageIO), `ocr` (VNRecognizeTextRequest, boxes per run), `faces` (VNDetectFaceRectanglesRequest), `beautify_render` (CoreGraphics background/shadow/frame compositing), `clipboard`, `recorder` (SCStream → AVAssetWriter), `mic` (device list, permission, `AVCaptureSession` path on macOS 14), `tutorial` (`AVAssetImageGenerator` keyframes, ring, crop, OCR hint), `transcribe` (audio export, `SFSpeechRecognizer`), `words` (word → subtitle segment grouping), `clip` (passthrough trim export, `AVAssetImageGenerator` + ImageIO GIF), `burn` (`AVMutableComposition` + `AVVideoCompositionCoreAnimationTool` subtitle burn-in), `keys` (listen-only `CGEventTap`, Input Monitoring permission), `permission` |
| `lumora-ai` | client portable, Keychain macOS-only | 19 | `client` (blocking chat completions over `ureq`, SSE streaming with deltas, cancel flag, connection test, HTTP → `AiError`), `transcribe` (`POST /audio/transcriptions`, 25 MB limit, verbose-JSON segments), `multipart` (form-data builder), `keychain` (`KeyStore` on the macOS Keychain, service `app.lumora.ai`); re-exports `lumora_core::ai` |
| `lumora-ui` | yes (`tokens` is portable) | 3 (tokens) | All windows, views, layers and motion (incl. `editor`, `editor::beautify_panel`, `ai_sheet`, `text_result`, `click_overlay`, `tutorial_window`, `transcript_window`, `history_window`, `trim_window`, `keystroke_hud`, `scroll_hud`, `settings_window::{ai, redact}`, `toast`); emits `UiEvent`s |
| `lumora` (app) | yes | 1 | `App` state, `Command` dispatch, flows (capture, record, OCR, editor, `doc_flow` for `.lumora` save/open, `ai_flow` for Keychain key + Test Connection, `ai_write_flow` for editor AI writing, `redact_flow`, `text_flow` for the Capture Text panel, `tutorial_flow`, `transcribe_flow`, `history_flow`, `clip_flow` for trim / GIF, `subtitle_flow` for burn-in, `scroll_flow` for scrolling capture), Carbon hotkeys, status item, hidden main menu (Edit / Window key equivalents), OS glue |

## Coordinate spaces

Everything in `lumora-core` is **global Quartz space**: points, origin at
the top-left of the primary display, y down — the space ScreenCaptureKit uses
for window and display frames. AppKit's bottom-left space appears only at
the UI edge and is converted with `geometry::flip_y` (an involution, using the
primary display's height). Overlay views are flipped (`isFlipped = YES`,
`geometryFlipped` root layers) so view-local coordinates are simply
`global - screen.origin`.

## Threading

* **Main thread** — all of `App`, all AppKit/Core Animation work. Types are
  `Rc`/`RefCell`; AppKit classes are `MainThreadOnly` and constructed with a
  `MainThreadMarker`.
* **ScreenCaptureKit queues** — `ShareableContent::fetch` and
  `screenshot::capture` call back with `Send` data (`CFRetained<CGImage>` is
  thread-safe); the app hops to main with `app::on_main`, which finds the
  app through a main-thread-local `Weak<App>`.
* **Recorder queue** — `com.lumora.recorder` (serial). SCStream delivers
  screen, audio and microphone buffers there; all `AVAssetWriter` access is
  serialised on it (plus the writer's own completion), which is what makes
  the `SendBox` wrappers sound. `stop()` hops onto the same queue so
  finalisation is ordered after in-flight samples.
* **Background work** — PNG encoding, `.lumora` writes, Vision OCR / face
  detection, AI requests, Keychain reads, tutorial frame rendering, audio
  export + transcription, HTML export and the launch-time scratch cleanup
  run on user-initiated global queues. Results hop back with `on_main`,
  tagged with a request id so stale or cancelled work is dropped.

## Control flow

```
hotkey (Carbon) ─┐
menu item ───────┼──▶ post_command(Command) ──▶ App::dispatch ──▶ flow
UI event ────────┘          (main queue)

UI component ──▶ EventSink ──▶ App event queue ──(next main turn)──▶ App::handle_event
```

UI components never call into the app synchronously: events are queued and
drained on the next main-queue turn, so a component can't be dropped from
inside its own callback.

`App.mode` is a single enum — `Idle`, `Preparing`, `Selecting`, `Countdown`,
`Recording`, `Finishing` — so only one capture/record flow can run, and
every resource of a flow (overlay, countdown, border, stop control,
recorder, timer) is owned by its variant and torn down by dropping it.
Recording additionally drives `lumora_core::recording::RecordingSession`,
which rejects illegal transitions and provides the elapsed time.

### Area capture

1. `ShareableContent::fetch` → displays + windows (z-ordered via
   `CGWindowListCreate`, own process excluded).
2. Freeze: one `SCScreenshotManager` capture per display, in parallel.
3. `OverlayController::show` — one key-capable non-activating panel per
   display at screen-saver level; a single `SelectionMachine` handles input.
4. On confirm: area → crop the frozen frame (pixel-aligned); window →
   desktop-independent window capture; click on desktop → that display.
5. `finish_capture`: name, shutter sound, shutter flight + Quick Access card
   immediately; PNG encode in the background; clipboard (image + file URL)
   when the file exists.

### Window mode and multi-display areas

Capture Window uses `selection::Config::window_only`: a pure click-to-pick
(no rubber band, desktop clicks ignored). Area selections may cross
display edges; `stitch::plan` splits them per frozen display at the
highest backing scale and `image::stitch` composites the pieces in a
`CGBitmapContext` (single-display selections are still a plain crop).
Recording remains one display per `SCStream`.

### Capture Text (OCR)

Same frozen-frame overlay in `OverlayMode::Text` → pixels for the target
(`frozen_image`, stitching if needed) → `ocr::recognize_text` on a
background queue (Vision, accurate, en / zh-Hans / zh-Hant / ja, language
correction) → `lumora_core::ocr::assemble` puts observations in reading
order (CJK runs joined without spaces) → pasteboard + toast with a preview.

Then, unless Settings → Capture → "Show actions panel after Capture Text"
is off, the floating `text_result` panel opens (shown with
`orderFrontRegardless`, never made key until clicked; Esc stops a stream,
then closes):

```
Vision runs (text + boxes) ─▶ text_result panel (main)
   ├─ Copy Text ─────────────▶ pasteboard
   ├─ As Table / As Code / Checklist ─▶ lumora_core::extract (instant, on-device)
   │        └─ AI on ─▶ apps/lumora::text_flow ─▶ background: Keychain read,
   │                    build_request(ExtractTable/ExtractCode/Checklist) ─▶ lumora_ai::complete
   └─ Summarize / Translate (AI only) ─▶ same path
deltas ─▶ on_main (request id) ─▶ ai_sheet hosted by the panel (Copy, Copy as CSV, Stop, Retry)
```

A failed or stopped AI refinement keeps the on-device result. Only the task
kind is logged, never the text.

### Smart redaction

Editor ⌘⇧R → `EditorAction::SmartRedact` → `apps/lumora::redact_flow` on a
background queue: Vision OCR (boxes per run) + face detection →
`lumora_core::redact::detect` (regex-free scanners with checksums: MOD 11-2
for Chinese IDs, Luhn + issuer prefix + keyword / veto context for bank
cards) → `redact::layout` maps each match to a sub-rectangle of its OCR box
using CJK-aware glyph widths → optional AI check (recognised text lines
only, needs AI Writing) → rectangles back on main → one undoable group of
pixelate / blur annotations. With "Redact new screenshots automatically"
on, the local detectors run before copy / save and the redactions stay
editable in the `.lumora` document.
### Annotation editor

```
Quick Access “Annotate” / double-click / “open editor after capture”
   └▶ App::open_editor ─▶ lumora_ui::editor::EditorWindow (one per capture)
          NSEvent ─▶ lumora_core::annotate::Editor (FSM) ─▶ Effect
          Effect ─▶ sync(): document ─▶ display list ─▶ layers / chrome / toolbar
          ⌘C/⌘S/close ─▶ render::flatten (CGBitmapContext) ─▶ UiEvent::Editor
   App writes the PNG over the capture file, card returns with the result
```

* **Model** (`lumora-core::annotate`): shapes in document points (y down,
  uncropped image origin); `Document` keeps z-order, crop and an undo
  stack of `Command`s (insert / delete / replace / reorder / crop / atomic
  batch). Live gestures mutate untracked and `record` one command on
  mouse-up, so ⌘Z undoes a whole drag. Step badges number themselves by
  creation order (ids are never reused).
* **One display list, two renderers** (`annotate::render` →
  `editor::render`): ops become `CAShapeLayer`/`CATextLayer`/Core Image
  layers live, and are replayed into a bitmap context for export — the
  saved PNG matches the screen by construction.
* Redaction is Core Image (`CIPixellate` anchored at the region corner,
  `CIGaussianBlur` on a clamped image) over the *base* image, cached per
  rect.

### Beautify

`lumora_core::beautify::BeautifyStyle` (background preset / solid / image,
padding, radius, shadow, window frame, aspect, auto-balance) →
`beautify::layout` computes the canvas and image rect in points →
`lumora_capture::beautify_render` composites it with CoreGraphics for
save / copy / drag-out; `lumora_ui::editor::beautify_panel` (the ✨ panel)
edits the style with a live Core Animation preview. The last style is
persisted in Settings and applied only when enabled.

### Editable documents (`.lumora`)

`lumora_core::lumora_doc` defines the file: magic + length-prefixed JSON
header (schema version, annotations as DTOs, beautify style) + the
original PNG. The live `annotate` model has no serde; DTOs convert both
ways, and `migrate` upgrades old schemas while newer ones are rejected.
`apps/lumora::doc_flow` writes the document atomically on a background
queue next to every saved PNG and reopens it (Open…, Finder via
`application:openURLs:`, Quick Access Annotate) into the editor with every
annotation still editable.

### AI writing

```
Settings → AI tab (settings_window::ai) ─▶ UiEvent::AiKeyRequested / AiKeySave / AiTestConnection
   └▶ apps/lumora::ai_flow ─▶ lumora_ai::Keychain · lumora_ai::test_connection (background queue)
Editor ✨ menu / ⌘J ─▶ EditorAction::AiWrite(AiWriteRequest)
   └▶ apps/lumora::ai_write_flow: key + settings check ─▶ background queue:
          Vision OCR (if the task needs it) ─▶ lumora_core::ai::build_request ─▶ lumora_ai::complete
          deltas ─▶ on_main, tagged with the request id ─▶ editor::ai_sheet
```

`lumora_core::ai` is pure (no network, no Keychain): provider presets,
settings, prompts and stream parsing are unit-tested on every host. The API
key lives only in the Keychain (`app.lumora.ai`, account = provider id),
never in `AiSettings`/`settings.toml`, and is never logged. By default only
recognised text and annotation text are sent; the PNG is attached as a data
URL only when "Send screenshot image" is on. Stop sets the request's cancel
flag; a stopped or retried request's late deltas are dropped by id. Insert /
Replace go through the editor's command stack as one undoable edit.

## Localisation

`lumora_core::i18n` holds every user-facing string in one
`messages!` table (`Msg` → en / zh-Hans), so a missing translation does not
compile; tests check placeholders, ellipses and shortcut glyphs. The
process-wide language comes from `general.language` (`system` follows
`NSLocale.preferredLanguages`, any Chinese → zh-Hans). Changing it
re-titles the menu and rebuilds the Settings window in place.

### Recording

overlay (Record mode, live backdrop) → `RecordingBorder` (outside the area)
→ `Countdown` → `StopControl` → *refetch content so our chrome is listed* →
`Recorder::start` with `SCContentFilter(display:excludingWindows: ourWindows)`
→ first complete frame starts the writer session → stop → `finishWriting` →
poster frame via `AVAssetImageGenerator` → Quick Access card.

* **Microphone** — on macOS 15 SC captures the mic; on macOS 14
  `lumora_capture::mic` runs an `AVCaptureSession` on the recorder's sample
  queue, retimed to the host clock with `CMSyncConvertTime`; audio before
  the first video frame is dropped.
* **Click ripples + click log** — `click_overlay::ClickMonitor` (global +
  local mouse-down monitors) feeds one click-through overlay panel per
  display; those panels are deliberately *not* excluded from the
  `SCContentFilter`, so the rings are in the video. The raw clicks become
  `<stem>.events.json` (`lumora_core::click_log`) when the movie finalises.
* **Sidecars** — `.events.json`, `.steps.json`, `<stem>.tutorial/` and
  `.transcript.json` move with the movie on Save (copy + delete across
  volumes) and go to the Trash with it on Delete. At launch,
  `lumora_core::cleanup` removes `$TMPDIR/Lumora` entries untouched for
  7 days (never when that folder overlaps the save folder).

### Capture history

```
⌥⇧H / status menu ─▶ apps/lumora::history_flow::show_history ─▶ lumora_ui::history_window
background (utility): history::scan_folder(save folder, NameMatcher(template))
   ─▶ HistoryIndex::refresh (keeps OCR text / dimensions of unchanged files,
      re-stats entries outside the folder) ─▶ PNG IHDR / .lumora header /
      AVAssetTrack dimensions ─▶ history.json (atomic) ─▶ on_main: set_items
window asks for thumbnails ─▶ background: Thumbnails/<key>.png or ImageIO
   thumbnail / .lumora PNG / first movie frame ─▶ disk + memory cache ─▶ set_thumbnail
window open ─▶ OCR backfill in batches of 12 (Vision, newest first) ─▶ history.json
captures, editor saves, .lumora writes, recordings, Quick Access Save ─▶ history_note
```

The index sits behind a mutex and is only touched off the main thread.
Deleting moves files (and a recording's sidecars) to the Trash after a
confirmation; it never falls back to deleting.

### Trim, GIF and burned-in subtitles

```
Quick Access "Trim / GIF…" / history ─▶ apps/lumora::clip_flow ─▶ lumora_ui::trim_window
   Save Trimmed ─▶ global queue: lumora_capture::clip (AVAssetExportSession passthrough)
      ─▶ lumora_core::clip shifts .events.json / transcript ─▶ Quick Access
   Export GIF… ─▶ AVAssetImageGenerator frames ─▶ ImageIO GIF (≤ 1200 frames)
transcript window "Burn In Subtitles…" ─▶ apps/lumora::subtitle_flow
   ─▶ lumora_core::transcript::burn (layout) ─▶ lumora_capture::burn
      (composition + Core Animation text layers, HighestQuality export) ─▶ Quick Access
```

Both exports report progress and can be cancelled from their window.

### Keystroke HUD and scrolling capture

* **Keystroke HUD** — while a recording runs (opt-in), `lumora_capture::keys`
  installs a listen-only session event tap (Input Monitoring);
  `lumora_core::keystroke` decides what may be shown and merges repeats;
  `lumora_ui::keystroke_hud` draws the pill, which the recorder's content
  filter explicitly lets into the video (like the click ripples).
* **Scrolling capture** — ⌥⇧S → live overlay in `Scroll` mode → area →
  `apps/lumora::scroll_flow` screenshots it every 150 ms (SCScreenshotManager
  → BGRA) and feeds `lumora_core::scroll_capture` (row-hash offset, sticky
  bands, 20,000 px cap); `lumora_ui::scroll_hud` shows the frame count with
  Done / Cancel; the result goes through `finish_capture`.

### Recording → tutorial

`apps/lumora::tutorial_flow`: `.events.json` → `lumora_core::tutorial`
steps (outside clicks dropped, double / rapid clicks merged, ≤ 40 steps) →
background: `lumora_capture::tutorial` grabs a keyframe 0.12 s after each
click, draws a ring, crops around it, saves `<stem>.tutorial/step-NN.png`
and reads an OCR hint → titles from AI (text only: click kind, app, window
title, OCR hint) or local templates → `<stem>.steps.json` →
`tutorial_window` (edit, Export Markdown / HTML, Copy Markdown). Loaded
steps are sanitised to safe relative image paths.

### Subtitles, summary and chapters

`apps/lumora::transcribe_flow` on a background queue, cancellable at every
stage: Speech permission (asked on main first) → export the chosen audio
track(s) to a temp `.m4a` → recognise with `SFSpeechRecognizer` (on-device,
strict mode optional; `words::group_words` makes segments) or the AI
provider's `/audio/transcriptions` (≤ 25 MB) → normalise → summary and
chapters (AI text-only when on, else local from pauses and clicks) →
`<stem>.transcript.json` → `transcript_window` (edit lines, Export SRT /
VTT, Copy Chapters / Summary; edits saved on close). The temp audio is
always deleted.

## Motion

Two mechanisms, one model (`lumora_core::spring::Spring`):

* **Layers** — `util::spring_animation` builds a `CASpringAnimation` with the
  same mass/stiffness/damping and a duration equal to the spring's computed
  settling time, so CA never truncates the curve.
* **Windows and multi-layer geometry** — `anim::WindowSpring` and
  `anim::RectSpring` sample the analytic solution on a 120 Hz timer.
  Retargeting carries velocity over (`SpringAnimator::retarget_with`), so the
  Quick Access stack and the overlay highlight flow instead of jumping.

Live tracking (dragging a selection) is deliberately *not* animated.

## Error handling

`CaptureError` covers permission, vanished targets, framework errors and
write failures. Flows reset to `Idle` and surface errors: onboarding for
permission, otherwise a localised toast (`App::notify` /
`App::fail_with` → `lumora_ui::toast`). Shortcuts another app holds are
toasted once and outlined in the Shortcuts tab. Settings never fail to load: corrupt
files are backed up and defaults used.

## Unsafe code

Unsafe is confined to FFI calls the bindings mark unsafe (mostly
ScreenCaptureKit/AVFoundation methods, CF bridging, Carbon). Each block has a
`SAFETY:` comment. `SendBox` is the only manual `Send` impl and every
construction documents its synchronisation argument.
