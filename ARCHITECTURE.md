# Architecture

Lumora is a Cargo workspace of four crates with a strict dependency
direction. Logic that doesn't need the OS lives in the bottom crate and is
tested on every host; every crate above it is a thin, typed layer over Apple
frameworks.

```
apps/lumora        binary: NSApplication, status item, hotkeys, App state, flows
   │
   ├── crates/lumora-ui        AppKit + Core Animation: overlay, Quick Access, pins,
   │      │                    shutter, countdown, recording HUD, settings, onboarding,
   │      │                    annotation editor + beautify panel, toast HUD
   │      │
   ├──────┴── crates/lumora-capture   ScreenCaptureKit, ImageIO, AVFoundation, pasteboard,
   │                 │                Vision OCR, multi-display stitching, beautify renderer
   │                 │
   └─────────────────┴── crates/lumora-core   pure Rust: geometry, springs, selection
                                              state machine, snapping, settings, file
                                              names, hotkey parsing, layout, recording FSM,
                                              annotation document + editor FSM, i18n,
                                              OCR text assembly, stitch planning,
                                              beautify model, .lumora document format
```

| Crate | macOS-only? | Tests | What it owns |
|---|---|---|---|
| `lumora-core` | no | 221 unit tests | `beautify` (style model, presets, layout, auto-balance), `lumora_doc` (`.lumora` container, DTO layer, schema migrations), `annotate` (paths, shapes, display list, command-pattern document, editor FSM), `i18n` (en + zh-Hans table, language resolution), `ocr` (reading-order assembly), `stitch` (multi-display plans), `geometry` (Rect/Point/Size, handles, pixel alignment, Quartz⇄AppKit flip), `spring` (closed-form damped oscillator + retargetable animator), `selection` (overlay interaction FSM), `snap` (window hit-testing, edge snapping), `settings` (serde/TOML model with validation), `filename` (templates, unique paths), `hotkey` (parse/format/Carbon codes/conflicts), `layout` (card stack, pill, loupe, fly transform), `recording` (lifecycle FSM), `color`, `format`, `action` |
| `lumora-capture` | yes (error type is portable) | 1 (mac) | `content` (SCShareableContent → displays + z-ordered windows), `screenshot` (SCScreenshotManager), `image` (crop, stitch, pixel reads, PNG+DPI via ImageIO), `ocr` (VNRecognizeTextRequest), `beautify_render` (CoreGraphics background/shadow/frame compositing), `clipboard`, `recorder` (SCStream → AVAssetWriter), `permission` |
| `lumora-ui` | yes (`tokens` is portable) | 3 (tokens) | All windows, views, layers and motion (incl. `editor`, `editor::beautify_panel`, `toast`); emits `UiEvent`s |
| `lumora` (app) | yes | 1 | `App` state, `Command` dispatch, flows (capture, record, OCR, editor, `doc_flow` for `.lumora` save/open), Carbon hotkeys, status item, OS glue |

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
* **Background encode** — PNG encoding runs on a user-initiated global queue.

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
