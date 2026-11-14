# Design

Lumora's chrome is quiet, dark glass over arbitrary content, with motion
that feels physical. All values live in `crates/lumora-ui/src/tokens.rs`;
this document mirrors them. Change both together.

## Principles

1. **The screenshot is the hero.** Chrome is small, dark, translucent and
   gets out of the way (hint pill disappears on first click).
2. **Never lag the pointer.** Direct manipulation is 1:1; springs are only
   for discrete changes (snap, nudge, commit, appear/disappear).
3. **Springs, not curves.** Every motion is a damped harmonic oscillator, so
   interrupted animations retarget with their velocity intact.
4. **Native first.** SF Pro, SF Symbols, `NSVisualEffectView` materials,
   continuous corner curves, system shutter sound.

## Colour

| Token | Value | Use |
|---|---|---|
| `ACCENT` | `#0A84FF` | Handles, window highlight, focus |
| `RECORD` | `#FF453A` | Recording border, stop button, countdown ring, menu-bar dot |
| `DIM` | black @ 42 % | Overlay outside the selection |
| `SELECTION_STROKE` | white @ 95 % | Selection outline (1 pt + 2 pt soft shadow) |
| `HIGHLIGHT_FILL` / `_STROKE` | accent @ 16 % / 90 % | Hovered window |
| `CROSSHAIR` | white @ 55 %, dashed 4/3 | Guides |
| `PILL_BG` | `rgb(18,18,20)` @ 82 % | Size pill, loupe label, hint, badges |
| `PILL_TEXT` / `_SECONDARY` | white @ 96 % / 60 % | Text on pills |
| `HAIRLINE` | white @ 14 % | 0.5 pt borders on glass |
| `LOUPE_GRID` | black @ 18 % | Pixel grid |
| `FLASH` | white | Shutter flash |

Record mode swaps `ACCENT` for `RECORD` in the overlay.

## Shape

| Token | Radius (pt) |
|---|---|
| Card | 14 (continuous) |
| Card thumbnail | 9 |
| Pill | 7 (hint pill: fully rounded) |
| Loupe | 14 |
| HUD / stop control | height / 2 |
| Countdown tile | 40 |
| Pin | 8 |
| Button | 8 (card glyph buttons: circular, 30 pt) |

Sizes: card width 232, height 96–196 by aspect, margin 18, spacing 12, inset
6; handles 9 pt circles (8 pt hit radius); pill height 24, padding 10, gap 10;
loupe 15 × 15 px at 8 pt per pixel (120 pt) + 22 pt label; countdown 168 pt
tile with a 112 pt, 5 pt-wide ring; stop control 132 × 40.

## Typography

SF Pro (system font). Live numbers use **monospaced digits** so they don't
jitter.

| Token | Size | Weight | Use |
|---|---|---|---|
| `PILL` | 12 | Medium, mono digits | `1280 × 720`, coordinates |
| `LOUPE_HEX` | 11 | Medium, mono digits | `#0A84FF   1204, 388` |
| `HUD_TIMER` | 13 | Semibold, mono digits | Stop control timer |
| `COUNTDOWN_DIGIT` | 64 | Bold | 3-2-1 |
| `BODY` | 13 | Regular | Onboarding, settings |
| `TITLE` | 22 | Bold | Onboarding title |
| `CAPTION` | 11 | Medium | Hint pill, badges, notes |

## Motion

Springs are specified physically (mass 1). Settling time is to within 0.1 %
of the target; overshoot is the peak excursion past it. (Numbers computed by
the solver in `lumora-core`; unit tests enforce the bounds.)

| Token | Stiffness | Damping | ζ | Settles | Overshoot | Use |
|---|---|---|---|---|---|---|
| `SELECTION` (`SNAPPY`) | 420 | 30 | 0.73 | 0.48 s | 3.4 % | Selection nudges, window highlight |
| `CARD` | 300 | 26 | 0.75 | 0.51 s | 2.8 % | Quick Access slide-in |
| `REFLOW` (`SMOOTH`) | 220 | 30 | 1.01 | 0.64 s | 0 % | Stack reflow, card slide-out |
| `FLY` | 260 | 24 | 0.74 | 0.54 s | 3.0 % | Shutter capture → card |
| `POP` (`BOUNCY`) | 260 | 14 | 0.43 | 1.0 s | 22 % | Countdown digits, handles appearing, pin drop |

Durations (ease-out): overlay dim fade-in **120 ms**, overlay fade-out 100 ms,
shutter flash 180 ms (0 → 0.9 → 0 at 25 %), hover reveal 150 ms, marching
ants period 600 ms (dash 6 / gap 4). Frame-driven animations tick at 120 Hz.

### Choreography

* **Overlay appear** — frozen frame shows instantly (it *is* the screen);
  dim fades in over 120 ms; hint pill at top centre until first click.
* **Hover** — highlight springs from window to window; first appearance pops
  from 96 % of the window's size.
* **Commit** — handles pop in (scale 0.3 → 1, `POP`).
* **Capture** — overlay removed instantly → flash over the region → capture
  shrinks onto the card slot (`FLY`, aspect-fit, corners rounding toward the
  thumbnail's) while the card slides in from the left (`CARD`); the flying
  image fades in its last 140 ms, landing on the card.
* **Stack** — new card at the bottom; older cards reflow up (`REFLOW`);
  a fourth card retires the oldest (slide-out + fade).
* **Countdown** — tile pops in (0.85 → 1); each second the digit pops
  (0.55 → 1) and the red ring drains clockwise from 12 o'clock.

## Iconography

The mark — four viewfinder brackets around a lens dot — is drawn in code
(`lumora_ui::icon`) at any size: a template image in the menu bar, a red dot
while recording, and a colour version on onboarding. Card actions use SF
Symbols: `doc.on.doc`, `square.and.arrow.down`, `pin`, `trash`; recording uses
`stop.fill`.
