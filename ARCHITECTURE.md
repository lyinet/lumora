# Architecture

Lumora is a Cargo workspace of four crates with a strict dependency
direction. Logic that doesn't need the OS lives in the bottom crate and is
tested on every host; every crate above it is a thin, typed layer over Apple
frameworks.

```
apps/lumora        binary: NSApplication, status item, hotkeys, App state, flows
   │
   ├── crates/lumora-ui        AppKit + Core Animation: overlay, Quick Access, pins,
   │      │                    shutter, countdown, recording HUD, settings, onboarding
   │      │
   ├──────┴── crates/lumora-capture   ScreenCaptureKit, ImageIO, AVFoundation, pasteboard
   │                 │
   └─────────────────┴── crates/lumora-core   pure Rust: geometry, springs, selection
                                              state machine, snapping, settings, file
                                              names, hotkey parsing, layout, recording FSM
```

| Crate | macOS-only? | Tests | What it owns |
|---|---|---|---|
| `lumora-core` | no | 76 unit tests | `geometry` (Rect/Point/Size, handles, pixel alignment, Quartz⇄AppKit flip), `spring` (closed-form damped oscillator + retargetable animator), `selection` (overlay interaction FSM), `snap` (window hit-testing, edge snapping), `settings` (serde/TOML model with validation), `filename` (templates, unique paths), `hotkey` (parse/format/Carbon codes/conflicts), `layout` (card stack, pill, loupe, fly transform), `recording` (lifecycle FSM), `color`, `format`, `action` |
| `lumora-capture` | yes (error type is portable) | 1 (mac) | `content` (SCShareableContent → displays + z-ordered windows), `screenshot` (SCScreenshotManager), `image` (crop, pixel reads, PNG+DPI via ImageIO), `clipboard`, `recorder` (SCStream → AVAssetWriter), `permission` |
| `lumora-ui` | yes (`tokens` is portable) | 3 (tokens) | All windows, views, layers and motion; emits `UiEvent`s |
| `lumora` (app) | yes | 1 | `App` state, `Command` dispatch, flows, Carbon hotkeys, status item, OS glue |

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
write failures. Flows reset to `Idle` and surface errors (onboarding for
permission, a beep + log otherwise). Settings never fail to load: corrupt
files are backed up and defaults used.

## Unsafe code

Unsafe is confined to FFI calls the bindings mark unsafe (mostly
ScreenCaptureKit/AVFoundation methods, CF bridging, Carbon). Each block has a
`SAFETY:` comment. `SendBox` is the only manual `Send` impl and every
construction documents its synchronisation argument.
