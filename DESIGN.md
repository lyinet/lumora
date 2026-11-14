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

Editor and HUD additions:

| Token | Value | Use |
|---|---|---|
| `EDITOR_BACKDROP` | `rgb(28,28,31)` | Canvas behind the image |
| `TOOL_ACTIVE` | accent @ 90 % | Active-tool pill |
| `CROP_DIM` | black @ 50 % | Outside a pending crop |
| `SUCCESS` / `WARNING` / `ERROR` | `#30D158` / `#FFD60A` / `#FF453A` | Toast glyphs, invalid shortcut outline |

Annotation palette (`annotate::style::PALETTE`): red `#FF3B30`, yellow
`#FFCC00`, green `#34C759`, blue `#0A84FF`, ink `#1C1C1E`, white — plus a
custom colour from `NSColorPanel`. Stroke widths thin / medium / thick =
2 / 4 / 8 pt (highlighter 12 / 20 / 32, text 15 / 22 / 34, step badges
24 / 32 / 44). Ink gets a soft shadow (black 30 %, 3 pt blur, 1 pt down);
badge and text-pill labels are white unless the fill's luminance > 0.5.
Arrows are one filled, tapered outline (shaft 40 % → 100 % of the width,
swallowtail head ≈ 3.6 w + 9 long).

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
| Editor tool pill | 7 |
| Editor image | 6 |
| Toast | height / 2 (max 22) |

Sizes: card width 232, height 96–196 by aspect, margin 18, spacing 12, inset
6; handles 9 pt circles (8 pt hit radius); pill height 24, padding 10, gap 10;
loupe 15 × 15 px at 8 pt per pixel (120 pt) + 22 pt label; countdown 168 pt
tile with a 112 pt, 5 pt-wide ring; stop control 132 × 40; editor toolbar
52 pt tall (30 pt glyph buttons, 18 pt swatches), canvas margin 28, editor
minimum 760 × 420; toast 44 pt (58 with detail), 180–420 wide, 64 pt below
the menu bar.

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
| `INSERT` | 380 | 24 | 0.62 | ≈0.6 s | 8.6 % | New annotation settling in (scale 0.85 → 1) |
| `TOOL` (`SNAPPY`) | 420 | 30 | 0.73 | 0.48 s | 3.4 % | Active-tool pill sliding |
| `TOAST` | 320 | 24 | 0.67 | ≈0.55 s | 5.8 % | Toast drop-in (12 pt) |

(Settling times for INSERT/TOAST are approximate; unit tests bound them
below 1.2 s and overshoot below 10 %.) Toasts stay 2.2 s (success) or 4 s (warning / error).

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

**App icon** (`scripts/make_icon.py` → `packaging/icon/AppIcon.svg`,
`packaging/AppIcon.icns`): a superellipse (n = 5) body filled with an
indigo `#5E5CE6` → `#7B5CF0` → violet `#BF5AF2` diagonal gradient, a soft
central glow and top sheen, the mark's four white viewfinder brackets, and
a pale lens with six indigo→violet aperture blades and a specular dot.
Edit the script, not the SVG.

The mark — four viewfinder brackets around a lens dot — is drawn in code
(`lumora_ui::icon`) at any size: a template image in the menu bar, a red dot
while recording, and a colour version on onboarding. Card actions use SF
Symbols: `doc.on.doc`, `square.and.arrow.down`, `pencil.tip.crop.circle`,
`pin`, `trash`; recording uses `stop.fill`. Editor tools: `cursorarrow`,
`arrow.up.right`, `rectangle`, `circle`, `line.diagonal`, `scribble`,
`highlighter`, `textformat`, `1.circle`, `square.grid.3x3.middle.filled`,
`light.max`, `crop`.
