//! Design tokens — the single source of truth for Lumora's look and motion.
//! DESIGN.md documents the same values for designers; keep them in sync.

use lumora_core::spring::Spring;

/// Linear sRGB colour with alpha, components 0–1.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rgba {
    pub r: f64,
    pub g: f64,
    pub b: f64,
    pub a: f64,
}

impl Rgba {
    pub const fn new(r: f64, g: f64, b: f64, a: f64) -> Self {
        Rgba { r, g, b, a }
    }

    /// From a `0xRRGGBB` literal.
    pub const fn hex(rgb: u32, a: f64) -> Self {
        Rgba::new(
            ((rgb >> 16) & 0xFF) as f64 / 255.0,
            ((rgb >> 8) & 0xFF) as f64 / 255.0,
            (rgb & 0xFF) as f64 / 255.0,
            a,
        )
    }

    pub const fn with_alpha(self, a: f64) -> Self {
        Rgba { a, ..self }
    }
}

/// Colour palette. Lumora's overlay chrome is always "dark glass" regardless
/// of system appearance, because it sits on top of arbitrary content.
pub mod color {
    use super::Rgba;

    /// Primary accent (macOS system blue, dark variant).
    pub const ACCENT: Rgba = Rgba::hex(0x0A84FF, 1.0);
    /// Recording red (system red, dark variant).
    pub const RECORD: Rgba = Rgba::hex(0xFF453A, 1.0);
    /// Overlay dim outside the selection.
    pub const DIM: Rgba = Rgba::new(0.0, 0.0, 0.0, 0.42);
    /// Selection outline.
    pub const SELECTION_STROKE: Rgba = Rgba::new(1.0, 1.0, 1.0, 0.95);
    /// Hovered-window fill and stroke.
    pub const HIGHLIGHT_FILL: Rgba = Rgba::hex(0x0A84FF, 0.16);
    pub const HIGHLIGHT_STROKE: Rgba = Rgba::hex(0x0A84FF, 0.9);
    /// Crosshair guide lines.
    pub const CROSSHAIR: Rgba = Rgba::new(1.0, 1.0, 1.0, 0.55);
    /// Dark glass pills (size readout, hex label).
    pub const PILL_BG: Rgba = Rgba::new(0.07, 0.07, 0.08, 0.82);
    pub const PILL_TEXT: Rgba = Rgba::new(1.0, 1.0, 1.0, 0.96);
    pub const PILL_TEXT_SECONDARY: Rgba = Rgba::new(1.0, 1.0, 1.0, 0.6);
    /// Hairline border used on glass surfaces for definition on dark content.
    pub const HAIRLINE: Rgba = Rgba::new(1.0, 1.0, 1.0, 0.14);
    /// Loupe pixel grid and centre marker.
    pub const LOUPE_GRID: Rgba = Rgba::new(0.0, 0.0, 0.0, 0.18);
    pub const LOUPE_CENTER: Rgba = Rgba::new(1.0, 1.0, 1.0, 1.0);
    /// Soft shadow colour.
    pub const SHADOW: Rgba = Rgba::new(0.0, 0.0, 0.0, 1.0);
    /// Shutter flash.
    pub const FLASH: Rgba = Rgba::new(1.0, 1.0, 1.0, 1.0);
    /// Handle fill/stroke.
    pub const HANDLE_FILL: Rgba = Rgba::new(1.0, 1.0, 1.0, 1.0);
    pub const HANDLE_STROKE: Rgba = Rgba::hex(0x0A84FF, 1.0);
    /// Card button glyph tint and hover background.
    pub const BUTTON_BG: Rgba = Rgba::new(0.0, 0.0, 0.0, 0.55);
    /// Editor canvas backdrop (behind the image).
    pub const EDITOR_BACKDROP: Rgba = Rgba::new(0.11, 0.11, 0.12, 1.0);
    /// Active-tool pill behind toolbar glyphs.
    pub const TOOL_ACTIVE: Rgba = Rgba::hex(0x0A84FF, 0.9);
    /// Crop tool: dim outside the pending crop.
    pub const CROP_DIM: Rgba = Rgba::new(0.0, 0.0, 0.0, 0.5);
    /// Toast glyph tints.
    pub const SUCCESS: Rgba = Rgba::hex(0x30D158, 1.0);
    pub const WARNING: Rgba = Rgba::hex(0xFFD60A, 1.0);
    pub const ERROR: Rgba = Rgba::hex(0xFF453A, 1.0);
}

/// Corner radii (points).
pub mod radius {
    pub const CARD: f64 = 14.0;
    pub const CARD_THUMB: f64 = 9.0;
    pub const PILL: f64 = 7.0;
    pub const LOUPE: f64 = 14.0;
    pub const HUD: f64 = 18.0;
    pub const COUNTDOWN: f64 = 40.0;
    pub const PIN: f64 = 8.0;
    pub const BUTTON: f64 = 8.0;
    /// Editor toolbar glyph buttons and active-tool pill.
    pub const TOOL: f64 = 7.0;
    /// Image corners on the editor canvas.
    pub const EDITOR_IMAGE: f64 = 6.0;
    /// Toast HUD (fully rounded at its height).
    pub const TOAST: f64 = 22.0;
}

/// Fixed sizes (points).
pub mod size {
    pub const CARD_WIDTH: f64 = 232.0;
    pub const CARD_MIN_HEIGHT: f64 = 96.0;
    pub const CARD_MAX_HEIGHT: f64 = 196.0;
    pub const CARD_MARGIN: f64 = 18.0;
    pub const CARD_SPACING: f64 = 12.0;
    pub const CARD_INSET: f64 = 6.0;
    pub const CARD_BUTTON: f64 = 30.0;
    pub const HANDLE: f64 = 9.0;
    pub const HANDLE_HIT: f64 = 8.0;
    pub const PILL_HEIGHT: f64 = 24.0;
    pub const PILL_PADDING: f64 = 10.0;
    pub const PILL_GAP: f64 = 10.0;
    /// Loupe: `LOUPE_GRID`×`LOUPE_GRID` pixels, each `LOUPE_CELL` points.
    pub const LOUPE_GRID: u32 = 15;
    pub const LOUPE_CELL: f64 = 8.0;
    pub const LOUPE_OFFSET: f64 = 22.0;
    pub const LOUPE_LABEL_HEIGHT: f64 = 22.0;
    pub const COUNTDOWN: f64 = 168.0;
    pub const COUNTDOWN_RING: f64 = 112.0;
    pub const COUNTDOWN_RING_WIDTH: f64 = 5.0;
    pub const BORDER_INSET: f64 = 3.0;
    pub const STOP_HUD_WIDTH: f64 = 132.0;
    pub const STOP_HUD_HEIGHT: f64 = 40.0;
    pub const STATUS_ICON: f64 = 18.0;
    /// Annotation editor.
    pub const EDITOR_TOOLBAR_HEIGHT: f64 = 52.0;
    pub const EDITOR_TOOL_BUTTON: f64 = 30.0;
    pub const EDITOR_TOOL_GAP: f64 = 2.0;
    pub const EDITOR_SWATCH: f64 = 18.0;
    pub const EDITOR_CANVAS_MARGIN: f64 = 28.0;
    pub const EDITOR_MIN_WIDTH: f64 = 760.0;
    pub const EDITOR_MIN_HEIGHT: f64 = 420.0;
    /// Leading inset that clears the traffic-light buttons.
    pub const EDITOR_TRAFFIC_LIGHTS: f64 = 78.0;
    /// Selection handles on annotations (diameter, screen points).
    pub const EDITOR_HANDLE: f64 = 9.0;
    /// Toast HUD.
    pub const TOAST_HEIGHT: f64 = 44.0;
    pub const TOAST_MIN_WIDTH: f64 = 180.0;
    pub const TOAST_MAX_WIDTH: f64 = 420.0;
    pub const TOAST_TOP: f64 = 64.0;
}

/// Type scale. All text uses SF Pro (the system font); numerals that change
/// live use monospaced digits so they don't jitter.
pub mod typography {
    pub const PILL: f64 = 12.0;
    pub const LOUPE_HEX: f64 = 11.0;
    pub const HUD_TIMER: f64 = 13.0;
    pub const COUNTDOWN_DIGIT: f64 = 64.0;
    pub const BODY: f64 = 13.0;
    pub const TITLE: f64 = 22.0;
    pub const CAPTION: f64 = 11.0;
}

/// Motion: durations (seconds) and springs.
pub mod motion {
    use super::Spring;

    /// Overlay dim fade-in.
    pub const DIM_FADE: f64 = 0.12;
    /// Overlay fade-out on cancel/confirm.
    pub const OVERLAY_OUT: f64 = 0.10;
    /// Shutter flash total duration.
    pub const FLASH: f64 = 0.18;
    /// Hover reveal for card buttons.
    pub const HOVER_FADE: f64 = 0.15;
    /// Dashed recording border: one full dash cycle.
    pub const MARCH_PERIOD: f64 = 0.6;
    /// Window-frame animation tick (120 Hz for ProMotion displays).
    pub const FRAME_TICK: f64 = 1.0 / 120.0;

    /// Selection corners / window highlight retargeting.
    pub const SELECTION: Spring = Spring::SNAPPY;
    /// Quick Access card slide-in (stiffness 300, damping 26).
    pub const CARD: Spring = Spring::CARD;
    /// Card stack reflow and slide-out.
    pub const REFLOW: Spring = Spring::SMOOTH;
    /// Shutter thumbnail flight into the card.
    pub const FLY: Spring = Spring::new(1.0, 260.0, 24.0);
    /// Countdown digit pop and handle appearance.
    pub const POP: Spring = Spring::BOUNCY;
    /// A new annotation settling in (k = 380, c = 24: a hint of overshoot).
    pub const INSERT: Spring = Spring::new(1.0, 380.0, 24.0);
    /// Active-tool pill sliding between toolbar buttons.
    pub const TOOL: Spring = Spring::SNAPPY;
    /// Toast HUD drop-in.
    pub const TOAST: Spring = Spring::new(1.0, 320.0, 24.0);
    /// How long toasts stay up (success / warning+error).
    pub const TOAST_SHORT: f64 = 2.2;
    pub const TOAST_LONG: f64 = 4.0;
    /// Toast fade-out.
    pub const TOAST_OUT: f64 = 0.22;
    /// Settling threshold used to derive CA durations from springs.
    pub const SETTLE_EPSILON: f64 = 1e-3;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_decodes() {
        let c = Rgba::hex(0xFF8000, 0.5);
        assert_eq!((c.r, c.g, c.a), (1.0, 128.0 / 255.0, 0.5));
        assert_eq!(c.b, 0.0);
        assert_eq!(color::ACCENT.with_alpha(0.2).a, 0.2);
    }

    #[test]
    fn springs_settle_quickly_enough_for_ui() {
        for s in [
            motion::SELECTION,
            motion::CARD,
            motion::REFLOW,
            motion::FLY,
            motion::POP,
            motion::INSERT,
            motion::TOOL,
            motion::TOAST,
        ] {
            let t = s.settling_time(motion::SETTLE_EPSILON);
            assert!(t < 1.2, "{s:?} settles in {t}s");
        }
        // Card and reflow must not visibly overshoot; pop must.
        assert!(motion::CARD.overshoot() < 0.05);
        assert!(motion::REFLOW.overshoot() < 0.01);
        assert!(motion::POP.overshoot() > 0.1);
        // Insertion is subtle: some life, far less than the countdown pop.
        assert!(motion::INSERT.overshoot() > 0.01 && motion::INSERT.overshoot() < 0.1);
        assert!(motion::TOAST.overshoot() < 0.1);
    }

    #[test]
    fn loupe_grid_is_odd() {
        assert_eq!(size::LOUPE_GRID % 2, 1, "loupe needs a centre pixel");
    }
}
