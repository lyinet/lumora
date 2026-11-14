# Lumora v0.1 — status (2026-11-15; nothing has been run on a Mac yet)
1. Compiles: all 4 crates (~10.1k lines Rust) pass `cargo check` + `cargo clippy --all-targets -D warnings` for aarch64-apple-darwin and x86_64-apple-darwin with 0 errors, 0 warnings; `cargo fmt --check` clean. Linking/running was not possible in the Linux sandbox.
2. Tests: 80 pass on Linux (lumora-core 76: geometry, springs vs numeric integration, selection FSM, snapping, settings/TOML, filenames, hotkeys, layout, recording FSM; lumora-ui tokens 3; app command map 1). +1 recorder test compiles for macOS only (not run).
3. Untested on hardware: everything touching AppKit/ScreenCaptureKit/AVFoundation/Carbon — overlay event routing across displays, layer geometry, key focus in non-activating panels, SC captures, MP4 output, audio sync, Quick Access drag-out, SMAppService, TCC onboarding, bundle.sh/CI macOS job.
4. Risks: (a) objc2 bindings type-check but selector/ownership mistakes only show at runtime; (b) key events in non-activating panels for an LSUIElement app; (c) AVAssetWriter timing (session starts at first complete frame; audio before it is dropped);
5. (d) SendBox soundness relies on serial-queue discipline in recorder.rs; (e) ad-hoc signing resets Screen Recording permission each build; (f) deprecated-but-needed APIs (resize cursors, copyCGImageAtTime) on macOS 15+.
6. Known gaps (marked TODO in code): mic on macOS 14 (SC mic is 15+), multi-display selections clipped to one display, window mode = forced hover picking, no .icns app icon, errors surface as beep+log, hotkey registration failures only logged, NSTimer instead of display link.
7. Next 1: Bring-up on Apple Silicon + Intel (macOS 14 & 15): run every flow, fix runtime crashes, verify Retina crop alignment and PNG DPI.
8. Next 2: Recording QA — A/V sync, HEVC, 60 fps on 5K, system audio + mic, long recordings, display unplug mid-recording.
9. Next 3: Ship plumbing — Developer ID signing + notarization in CI, app icon, Sparkle, Homebrew cask.
10. Next 4: Error HUD + hotkey-conflict feedback; Next 5: annotation editor (v0.2) on top of the existing layer/spring toolkit.
