//! "Beautify": place a capture on a padded background (CleanShot X / Shottr
//! style), optionally framed by a macOS-style title bar, with rounded corners,
//! a drop shadow and a fixed output aspect ratio.
//!
//! This module is the pure model: style types, built-in presets and the
//! layout maths. Rendering lives in the platform crates and only has to draw
//! what [`layout`] returns.
//!
//! Coordinates are y-down (origin top-left of the output canvas) and in the
//! the unit of the `content` size: points for [`layout`], device pixels for
//! [`layout_at_scale`] (which scales the point-based style for you).

use std::fmt;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::color::Rgb8;
use crate::geometry::{Point, Rect, Size};
use crate::i18n::Msg;

/// An 8-bit sRGB colour with straight (non-premultiplied) alpha.
///
/// Serialised as `"#RRGGBBAA"` so settings files stay readable; parsing also
/// accepts `#RRGGBB` and `#RGB` (opaque).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Rgba {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Rgba {
    pub const WHITE: Rgba = Rgba::opaque(255, 255, 255);
    pub const BLACK: Rgba = Rgba::opaque(0, 0, 0);

    pub const fn new(r: u8, g: u8, b: u8, a: u8) -> Self {
        Rgba { r, g, b, a }
    }

    pub const fn opaque(r: u8, g: u8, b: u8) -> Self {
        Rgba { r, g, b, a: 255 }
    }

    /// `#RRGGBBAA`, uppercase.
    pub fn hex(self) -> String {
        format!("#{:02X}{:02X}{:02X}{:02X}", self.r, self.g, self.b, self.a)
    }

    /// Parse `#RRGGBBAA`, `#RRGGBB` or `#RGB` (leading `#` optional).
    pub fn from_hex(s: &str) -> Option<Rgba> {
        let t = s.trim().trim_start_matches('#');
        if t.len() == 8 {
            let rgb = Rgb8::from_hex(&t[..6])?;
            let a = u8::from_str_radix(&t[6..], 16).ok()?;
            return Some(Rgba::new(rgb.r, rgb.g, rgb.b, a));
        }
        Rgb8::from_hex(t).map(Rgba::from)
    }

    /// Components as `0.0..=1.0` floats, for `CGColor` construction.
    pub fn components(self) -> [f64; 4] {
        [self.r, self.g, self.b, self.a].map(|c| f64::from(c) / 255.0)
    }

    pub fn rgb(self) -> Rgb8 {
        Rgb8::new(self.r, self.g, self.b)
    }
}

impl From<Rgb8> for Rgba {
    fn from(c: Rgb8) -> Self {
        Rgba::opaque(c.r, c.g, c.b)
    }
}

impl From<Rgba> for String {
    fn from(c: Rgba) -> String {
        c.hex()
    }
}

impl TryFrom<String> for Rgba {
    type Error = String;
    fn try_from(s: String) -> Result<Self, Self::Error> {
        Rgba::from_hex(&s).ok_or_else(|| format!("invalid colour {s:?}; expected #RRGGBB or #RRGGBBAA"))
    }
}

impl fmt::Display for Rgba {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.hex())
    }
}

/// What fills the canvas behind the capture.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum Background {
    /// Transparent canvas (padding, shadow and corners still apply).
    None,
    Solid(Rgba),
    /// `angle_deg` follows CSS: 0° runs bottom→top, 90° left→right, 180°
    /// top→bottom. `from` is the start colour. See [`gradient_endpoints`].
    LinearGradient {
        from: Rgba,
        to: Rgba,
        angle_deg: f32,
    },
    /// A custom wallpaper, drawn aspect-fill (cover) and centred.
    Image(PathBuf),
}

impl Default for Background {
    fn default() -> Self {
        Preset::DEFAULT.background()
    }
}

impl Background {
    /// The built-in preset this background is, if any (for highlighting the
    /// selected swatch).
    pub fn preset(&self) -> Option<Preset> {
        Preset::ALL.iter().copied().find(|p| p.background() == *self)
    }
}

/// Start and end points of a CSS-style linear gradient of `angle_deg` across
/// `rect` (y-down), such that the gradient line passes through the centre
/// and the corners get exactly the end colours — the same maths as CSS
/// `linear-gradient(<angle>, …)`. Feed these to `CGContextDrawLinearGradient`
/// (after flipping into the context's space if it is y-up).
pub fn gradient_endpoints(rect: Rect, angle_deg: f32) -> (Point, Point) {
    let a = f64::from(angle_deg).to_radians();
    // Direction in y-down space: 0° points up (−y), 90° points right (+x).
    let (dx, dy) = (a.sin(), -a.cos());
    let half = (rect.width() * dx.abs() + rect.height() * dy.abs()) / 2.0;
    let c = rect.mid();
    (Point::new(c.x - dx * half, c.y - dy * half), Point::new(c.x + dx * half, c.y + dy * half))
}

/// Built-in background presets. The ids are stable (they may be stored), the
/// display names go through i18n.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Preset {
    Sunset,
    Ocean,
    Aurora,
    Peach,
    Mint,
    Graphite,
    Lavender,
    Midnight,
}

impl Preset {
    pub const ALL: [Preset; 8] = [
        Preset::Sunset,
        Preset::Ocean,
        Preset::Aurora,
        Preset::Peach,
        Preset::Mint,
        Preset::Graphite,
        Preset::Lavender,
        Preset::Midnight,
    ];

    pub const DEFAULT: Preset = Preset::Ocean;

    /// Stable identifier.
    pub fn id(self) -> &'static str {
        match self {
            Preset::Sunset => "sunset",
            Preset::Ocean => "ocean",
            Preset::Aurora => "aurora",
            Preset::Peach => "peach",
            Preset::Mint => "mint",
            Preset::Graphite => "graphite",
            Preset::Lavender => "lavender",
            Preset::Midnight => "midnight",
        }
    }

    pub fn from_id(id: &str) -> Option<Preset> {
        Preset::ALL.iter().copied().find(|p| p.id() == id)
    }

    /// i18n key for the display name.
    pub fn name_key(self) -> Msg {
        match self {
            Preset::Sunset => Msg::PresetSunset,
            Preset::Ocean => Msg::PresetOcean,
            Preset::Aurora => Msg::PresetAurora,
            Preset::Peach => Msg::PresetPeach,
            Preset::Mint => Msg::PresetMint,
            Preset::Graphite => Msg::PresetGraphite,
            Preset::Lavender => Msg::PresetLavender,
            Preset::Midnight => Msg::PresetMidnight,
        }
    }

    /// `(from, to, angle_deg)`.
    pub fn stops(self) -> (Rgba, Rgba, f32) {
        let c = Rgba::opaque;
        match self {
            Preset::Sunset => (c(0xFF, 0x7E, 0x5F), c(0xFE, 0xB4, 0x7B), 135.0),
            Preset::Ocean => (c(0x2E, 0x8B, 0xFF), c(0x5E, 0xE7, 0xDF), 135.0),
            Preset::Aurora => (c(0x43, 0xE9, 0x7B), c(0x38, 0xA4, 0xF8), 135.0),
            Preset::Peach => (c(0xFF, 0xD3, 0xA5), c(0xFD, 0x6F, 0x9C), 135.0),
            Preset::Mint => (c(0xD4, 0xFC, 0x79), c(0x96, 0xE6, 0xA1), 135.0),
            Preset::Graphite => (c(0x43, 0x4A, 0x54), c(0x1C, 0x1F, 0x24), 160.0),
            Preset::Lavender => (c(0xA1, 0x8C, 0xD1), c(0xFB, 0xC2, 0xEB), 135.0),
            Preset::Midnight => (c(0x0F, 0x20, 0x27), c(0x2C, 0x53, 0x64), 160.0),
        }
    }

    pub fn background(self) -> Background {
        let (from, to, angle_deg) = self.stops();
        Background::LinearGradient { from, to, angle_deg }
    }
}

/// Output aspect ratio. Non-`Auto` ratios grow the canvas (never crop).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum AspectRatio {
    #[default]
    #[serde(rename = "auto")]
    Auto,
    #[serde(rename = "16:9")]
    R16x9,
    #[serde(rename = "9:16")]
    R9x16,
    #[serde(rename = "4:3")]
    R4x3,
    #[serde(rename = "3:2")]
    R3x2,
    #[serde(rename = "1:1")]
    R1x1,
}

impl AspectRatio {
    pub const ALL: [AspectRatio; 6] = [
        AspectRatio::Auto,
        AspectRatio::R16x9,
        AspectRatio::R9x16,
        AspectRatio::R4x3,
        AspectRatio::R3x2,
        AspectRatio::R1x1,
    ];

    /// Width ÷ height, or `None` for `Auto`.
    pub fn ratio(self) -> Option<f64> {
        match self {
            AspectRatio::Auto => None,
            AspectRatio::R16x9 => Some(16.0 / 9.0),
            AspectRatio::R9x16 => Some(9.0 / 16.0),
            AspectRatio::R4x3 => Some(4.0 / 3.0),
            AspectRatio::R3x2 => Some(3.0 / 2.0),
            AspectRatio::R1x1 => Some(1.0),
        }
    }

    /// Menu label. Ratios are language-neutral; only `Auto` is translated.
    pub fn label(self) -> &'static str {
        match self {
            AspectRatio::Auto => crate::i18n::tr(Msg::BeautifyAspectAuto),
            AspectRatio::R16x9 => "16:9",
            AspectRatio::R9x16 => "9:16",
            AspectRatio::R4x3 => "4:3",
            AspectRatio::R3x2 => "3:2",
            AspectRatio::R1x1 => "1:1",
        }
    }
}

/// Optional macOS-style window frame drawn above the capture.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WindowChrome {
    #[default]
    None,
    MacLight,
    MacDark,
}

impl WindowChrome {
    pub const ALL: [WindowChrome; 3] = [WindowChrome::None, WindowChrome::MacLight, WindowChrome::MacDark];

    /// Title-bar height in points (macOS untitled window).
    pub const BAR_HEIGHT: f64 = 28.0;
    /// Traffic-light diameter, gap between lights and left inset, in points.
    pub const LIGHT_DIAMETER: f64 = 12.0;
    pub const LIGHT_GAP: f64 = 8.0;
    pub const LIGHT_INSET: f64 = 12.0;
    /// Close / minimise / zoom fills.
    pub const LIGHT_COLORS: [Rgba; 3] =
        [Rgba::opaque(0xFF, 0x5F, 0x57), Rgba::opaque(0xFE, 0xBC, 0x2E), Rgba::opaque(0x28, 0xC8, 0x40)];

    /// Title-bar height in points; 0 without chrome.
    pub fn bar_height(self) -> f64 {
        match self {
            WindowChrome::None => 0.0,
            _ => Self::BAR_HEIGHT,
        }
    }

    /// Bar fill colour, if any.
    pub fn bar_color(self) -> Option<Rgba> {
        match self {
            WindowChrome::None => None,
            WindowChrome::MacLight => Some(Rgba::opaque(0xEC, 0xEC, 0xEC)),
            WindowChrome::MacDark => Some(Rgba::opaque(0x2B, 0x2B, 0x2E)),
        }
    }

    pub fn name_key(self) -> Msg {
        match self {
            WindowChrome::None => Msg::BeautifyNone,
            WindowChrome::MacLight => Msg::BeautifyChromeLight,
            WindowChrome::MacDark => Msg::BeautifyChromeDark,
        }
    }
}

/// Drop shadow under the framed capture.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Shadow {
    pub enabled: bool,
    /// Blur radius in points, `0..=128`.
    pub radius: f64,
    /// `0.0..=1.0`.
    pub opacity: f64,
    /// Vertical offset in points (positive = down), `-64..=64`.
    pub offset_y: f64,
}

impl Default for Shadow {
    fn default() -> Self {
        Shadow { enabled: true, radius: 24.0, opacity: 0.35, offset_y: 12.0 }
    }
}

/// The complete beautify style; persisted as the "remember last" style in
/// [`crate::settings::Settings::beautify`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BeautifyStyle {
    /// Apply beautify to new captures.
    pub enabled: bool,
    pub background: Background,
    /// Space around the framed capture, in points, `0..=256`.
    pub padding: f64,
    /// Corner radius of the framed capture, in points, `0..=64`.
    pub corner_radius: f64,
    pub shadow: Shadow,
    pub chrome: WindowChrome,
    pub aspect: AspectRatio,
    /// Nudge the capture up so the shadow below it doesn't make it look
    /// bottom-heavy; see [`layout`].
    pub auto_balance: bool,
}

impl Default for BeautifyStyle {
    fn default() -> Self {
        BeautifyStyle {
            enabled: false,
            background: Background::default(),
            padding: 64.0,
            corner_radius: 12.0,
            shadow: Shadow::default(),
            chrome: WindowChrome::None,
            aspect: AspectRatio::Auto,
            auto_balance: true,
        }
    }
}

pub const MAX_PADDING: f64 = 256.0;
pub const MAX_CORNER_RADIUS: f64 = 64.0;
pub const MAX_SHADOW_RADIUS: f64 = 128.0;
pub const MAX_SHADOW_OFFSET: f64 = 64.0;
/// Largest content edge [`layout`] accepts; larger (or non-finite) input is
/// clamped so the maths can never overflow a `u32` pixel size.
pub const MAX_CONTENT_EDGE: f64 = 100_000.0;

fn clamp_finite(v: f64, lo: f64, hi: f64, fallback: f64) -> f64 {
    if v.is_finite() {
        v.clamp(lo, hi)
    } else {
        fallback
    }
}

impl BeautifyStyle {
    /// Clamp every numeric field into its supported range (non-finite values
    /// fall back to the default).
    pub fn validate(&mut self) {
        let d = BeautifyStyle::default();
        self.padding = clamp_finite(self.padding, 0.0, MAX_PADDING, d.padding);
        self.corner_radius = clamp_finite(self.corner_radius, 0.0, MAX_CORNER_RADIUS, d.corner_radius);
        let s = &mut self.shadow;
        s.radius = clamp_finite(s.radius, 0.0, MAX_SHADOW_RADIUS, d.shadow.radius);
        s.opacity = clamp_finite(s.opacity, 0.0, 1.0, d.shadow.opacity);
        s.offset_y = clamp_finite(s.offset_y, -MAX_SHADOW_OFFSET, MAX_SHADOW_OFFSET, d.shadow.offset_y);
        if let Background::LinearGradient { angle_deg, .. } = &mut self.background {
            *angle_deg = if angle_deg.is_finite() { angle_deg.rem_euclid(360.0) } else { 135.0 };
        }
    }

    /// A validated copy.
    pub fn validated(&self) -> BeautifyStyle {
        let mut s = self.clone();
        s.validate();
        s
    }

    /// Copy with every point-based length multiplied by `scale` (e.g. the
    /// capture's backing scale), so [`layout`] can work in pixels. Apply
    /// after validation; the result is not re-clamped.
    pub fn scaled(&self, scale: f64) -> BeautifyStyle {
        let k = if scale.is_finite() && scale > 0.0 { scale } else { 1.0 };
        let mut s = self.clone();
        s.padding *= k;
        s.corner_radius *= k;
        s.shadow.radius *= k;
        s.shadow.offset_y *= k;
        s
    }
}

/// Where everything goes on the output canvas (y-down, whole units).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BeautifyLayout {
    /// Output image size.
    pub canvas: Size,
    /// The framed "window": chrome bar + content. Corner radius, shadow and
    /// clipping apply to this rect.
    pub frame: Rect,
    /// The title bar, at the top of `frame`, if chrome is on.
    pub chrome_bar: Option<Rect>,
    /// Where the captured image is drawn (unscaled, 1:1).
    pub content: Rect,
    /// Corner radius to use for `frame`, clamped to half its shorter edge.
    pub corner_radius: f64,
    /// Units `frame` was moved up by auto-balance (0 when off).
    pub balance_offset: f64,
}

impl BeautifyLayout {
    /// Canvas size in whole pixels (the layout is already integral).
    pub fn pixel_size(&self) -> (u32, u32) {
        (self.canvas.width.max(0.0) as u32, self.canvas.height.max(0.0) as u32)
    }

    /// Centres of the three traffic lights in the chrome bar (close,
    /// minimise, zoom), with their diameter, at `scale` units per point.
    pub fn traffic_lights(&self, scale: f64) -> Option<([Point; 3], f64)> {
        let bar = self.chrome_bar?;
        let k = if scale.is_finite() && scale > 0.0 { scale } else { 1.0 };
        let d = WindowChrome::LIGHT_DIAMETER * k;
        let y = bar.mid().y;
        let x0 = bar.min_x() + WindowChrome::LIGHT_INSET * k + d / 2.0;
        let step = d + WindowChrome::LIGHT_GAP * k;
        Some(([Point::new(x0, y), Point::new(x0 + step, y), Point::new(x0 + 2.0 * step, y)], d))
    }
}

/// Lay out `content` on a beautify canvas, everything in points (scale 1).
/// Shorthand for [`layout_at_scale`]`(content, style, 1.0)`.
pub fn layout(content: Size, style: &BeautifyStyle) -> BeautifyLayout {
    layout_at_scale(content, style, 1.0)
}

/// Lay out `content` — the capture's size in **pixels** — on a beautify
/// canvas. `style` stays in points; its lengths and the chrome bar are
/// multiplied by `scale` (the capture's backing scale, 2.0 on Retina), so
/// the result is in pixels and can be drawn 1:1 into a bitmap context.
///
/// Rules:
/// 1. frame = content + chrome bar on top;
/// 2. canvas = frame + `padding` on every side;
/// 3. for a fixed aspect ratio the canvas grows on one axis (never shrinks
///    or crops) until it matches; the frame stays centred;
/// 4. `auto_balance` (only with an enabled, visible shadow pointing down)
///    moves the frame up by `shadow.offset_y / 2`, but never above the
///    canvas top, so frame + shadow read as visually centred;
/// 5. every edge is rounded to a whole unit. Zero, negative or non-finite
///    content is treated as 0; huge content is clamped to
///    [`MAX_CONTENT_EDGE`]; a non-finite or non-positive scale means 1.
///    Never panics.
pub fn layout_at_scale(content: Size, style: &BeautifyStyle, scale: f64) -> BeautifyLayout {
    let k = if scale.is_finite() && scale > 0.0 { scale } else { 1.0 };
    let style = &style.scaled(k);
    let edge = |v: f64| clamp_finite(v, 0.0, MAX_CONTENT_EDGE, 0.0).round();
    let cw = edge(content.width);
    let ch = edge(content.height);
    let nonneg = |v: f64| if v.is_finite() { v.max(0.0) } else { 0.0 };
    let bar_h = (style.chrome.bar_height() * k).min(MAX_CONTENT_EDGE).round();
    let pad = nonneg(style.padding).min(MAX_PADDING * 16.0).round();

    let frame_w = cw;
    let frame_h = ch + bar_h;
    let mut canvas_w = frame_w + 2.0 * pad;
    let mut canvas_h = frame_h + 2.0 * pad;

    if let Some(r) = style.aspect.ratio() {
        if canvas_w > 0.0 && canvas_h > 0.0 {
            if canvas_w / canvas_h < r {
                canvas_w = (canvas_h * r).round().max(canvas_w);
            } else {
                canvas_h = (canvas_w / r).round().max(canvas_h);
            }
        }
    }

    let x = ((canvas_w - frame_w) / 2.0).round();
    let centred_y = ((canvas_h - frame_h) / 2.0).round();
    let sh = &style.shadow;
    let balance = if style.auto_balance && sh.enabled && sh.opacity > 0.0 {
        (nonneg(sh.offset_y) / 2.0).round().min(centred_y)
    } else {
        0.0
    };
    let y = centred_y - balance;

    let frame = Rect::new(x, y, frame_w, frame_h);
    let chrome_bar = (bar_h > 0.0).then(|| Rect::new(x, y, frame_w, bar_h));
    let content_rect = Rect::new(x, y + bar_h, cw, ch);
    let corner_radius = nonneg(style.corner_radius).min(frame_w.min(frame_h) / 2.0);

    BeautifyLayout {
        canvas: Size::new(canvas_w, canvas_h),
        frame,
        chrome_bar,
        content: content_rect,
        corner_radius,
        balance_offset: balance,
    }
}

// ---------------------------------------------------------------------------
// Rendering helpers (pure maths shared with the CoreGraphics renderer)
// ---------------------------------------------------------------------------

/// The rect that covers `bounds` with an image of `image` size while keeping
/// its aspect ratio (CSS `background-size: cover`), centred. The result is
/// at least as large as `bounds` on both axes; the overflow is meant to be
/// clipped. Degenerate input (empty or non-finite) returns `bounds`.
pub fn aspect_fill_rect(image: Size, bounds: Rect) -> Rect {
    let ok = |v: f64| v.is_finite() && v > 0.0;
    if !ok(image.width) || !ok(image.height) || !ok(bounds.width()) || !ok(bounds.height()) {
        return bounds;
    }
    let k = (bounds.width() / image.width).max(bounds.height() / image.height);
    let (w, h) = (image.width * k, image.height * k);
    let c = bounds.mid();
    Rect::new(c.x - w / 2.0, c.y - h / 2.0, w, h)
}

/// A y-down rect on a canvas `canvas_height` tall, in y-up (Core Graphics
/// bitmap) coordinates. Its own inverse.
pub fn rect_y_up(r: Rect, canvas_height: f64) -> Rect {
    crate::geometry::flip_y(r, canvas_height)
}

/// Point version of [`rect_y_up`].
pub fn point_y_up(p: Point, canvas_height: f64) -> Point {
    crate::geometry::flip_point_y(p, canvas_height)
}

/// Arguments for `CGContextSetShadowWithColor` in a y-up bitmap context.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CgShadow {
    /// Horizontal offset (always 0).
    pub dx: f64,
    /// Vertical offset, y-up: negative moves the shadow down. Shadow offsets
    /// are in the context's base space and ignore the CTM.
    pub dy: f64,
    /// Blur radius in pixels.
    pub blur: f64,
    /// Opacity of the black shadow colour, `0..=1`.
    pub alpha: f64,
}

impl Shadow {
    /// The shadow to set on a y-up context drawing at `scale` pixels per
    /// point, or `None` when it would be invisible. Validates its inputs.
    pub fn cg_params(&self, scale: f64) -> Option<CgShadow> {
        let k = if scale.is_finite() && scale > 0.0 { scale } else { 1.0 };
        let d = Shadow::default();
        let alpha = clamp_finite(self.opacity, 0.0, 1.0, d.opacity);
        if !self.enabled || alpha <= 0.0 {
            return None;
        }
        let blur = clamp_finite(self.radius, 0.0, MAX_SHADOW_RADIUS, d.radius) * k;
        let off = clamp_finite(self.offset_y, -MAX_SHADOW_OFFSET, MAX_SHADOW_OFFSET, d.offset_y) * k;
        Some(CgShadow { dx: 0.0, dy: -off, blur, alpha })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::Language;
    use std::collections::HashSet;

    fn flat(padding: f64) -> BeautifyStyle {
        BeautifyStyle { padding, auto_balance: false, ..BeautifyStyle::default() }
    }

    fn ratio_of(l: &BeautifyLayout) -> f64 {
        l.canvas.width / l.canvas.height
    }

    #[test]
    fn presets_have_unique_ids_and_names() {
        let ids: HashSet<_> = Preset::ALL.iter().map(|p| p.id()).collect();
        assert_eq!(ids.len(), 8);
        let names: HashSet<_> = Preset::ALL.iter().map(|p| p.name_key().text(Language::En)).collect();
        assert_eq!(names.len(), 8);
        let grads: HashSet<_> = Preset::ALL.iter().map(|p| format!("{:?}", p.stops())).collect();
        assert_eq!(grads.len(), 8);
        for p in Preset::ALL {
            assert_eq!(Preset::from_id(p.id()), Some(p));
            assert_eq!(p.background().preset(), Some(p));
        }
        assert_eq!(Preset::from_id("nope"), None);
        assert_eq!(Background::Solid(Rgba::WHITE).preset(), None);
    }

    #[test]
    fn rgba_hex() {
        let c = Rgba::new(0x12, 0xAB, 0xFF, 0x80);
        assert_eq!(c.hex(), "#12ABFF80");
        assert_eq!(Rgba::from_hex("#12abff80"), Some(c));
        assert_eq!(Rgba::from_hex("fff"), Some(Rgba::WHITE));
        assert_eq!(Rgba::from_hex("#000000"), Some(Rgba::BLACK));
        assert_eq!(Rgba::from_hex("#1234567"), None);
        assert_eq!(Rgba::BLACK.components(), [0.0, 0.0, 0.0, 1.0]);
    }

    #[test]
    fn auto_aspect_is_content_plus_padding() {
        let l = layout(Size::new(800.0, 600.0), &flat(40.0));
        assert_eq!(l.canvas, Size::new(880.0, 680.0));
        assert_eq!(l.content, Rect::new(40.0, 40.0, 800.0, 600.0));
        assert_eq!(l.frame, l.content);
        assert_eq!(l.chrome_bar, None);
        assert_eq!(l.pixel_size(), (880, 680));
    }

    #[test]
    fn landscape_ratio_widens_portrait_content() {
        let s = BeautifyStyle { aspect: AspectRatio::R16x9, ..flat(0.0) };
        let l = layout(Size::new(900.0, 900.0), &s);
        assert_eq!(l.canvas, Size::new(1600.0, 900.0));
        assert_eq!(l.content.origin, Point::new(350.0, 0.0));
    }

    #[test]
    fn portrait_9x16_from_landscape_content() {
        let s = BeautifyStyle { aspect: AspectRatio::R9x16, ..flat(20.0) };
        let l = layout(Size::new(1600.0, 900.0), &s);
        // Width 1640 is kept; height grows to 1640 × 16/9.
        assert_eq!(l.canvas.width, 1640.0);
        assert_eq!(l.canvas.height, (1640.0_f64 * 16.0 / 9.0).round());
        assert!(l.canvas.contains_content(&l.content));
        assert_eq!(l.content.mid().y.round(), (l.canvas.height / 2.0).round());
    }

    #[test]
    fn every_ratio_matches_and_never_crops() {
        for a in AspectRatio::ALL {
            for content in [Size::new(1920.0, 1080.0), Size::new(400.0, 1200.0), Size::new(333.0, 333.0)] {
                let s = BeautifyStyle { aspect: a, ..flat(32.0) };
                let l = layout_at_scale(content, &s, 2.0);
                assert!(l.canvas.width >= content.width + 128.0, "{a:?}");
                assert!(l.canvas.height >= content.height + 128.0, "{a:?}");
                assert!(l.canvas.contains_content(&l.content));
                if let Some(r) = a.ratio() {
                    assert!((ratio_of(&l) - r).abs() < 0.01, "{a:?} {content:?} → {:?}", l.canvas);
                }
                assert_eq!(l.canvas.width.fract(), 0.0);
                assert_eq!(l.content.origin.x.fract(), 0.0);
            }
        }
    }

    #[test]
    fn four_three_and_square() {
        let s = BeautifyStyle { aspect: AspectRatio::R4x3, ..flat(0.0) };
        assert_eq!(layout(Size::new(1600.0, 900.0), &s).canvas, Size::new(1600.0, 1200.0));
        let s = BeautifyStyle { aspect: AspectRatio::R1x1, ..flat(0.0) };
        assert_eq!(layout(Size::new(300.0, 500.0), &s).canvas, Size::new(500.0, 500.0));
    }

    #[test]
    fn chrome_adds_bar_height() {
        let s = BeautifyStyle { chrome: WindowChrome::MacDark, ..flat(10.0) };
        let l = layout_at_scale(Size::new(200.0, 100.0), &s, 2.0);
        assert_eq!(l.canvas, Size::new(240.0, 100.0 + 56.0 + 40.0));
        assert_eq!(l.chrome_bar, Some(Rect::new(20.0, 20.0, 200.0, 56.0)));
        assert_eq!(l.content, Rect::new(20.0, 76.0, 200.0, 100.0));
        assert_eq!(l.frame, Rect::new(20.0, 20.0, 200.0, 156.0));
        let (lights, d) = l.traffic_lights(2.0).unwrap();
        assert_eq!(d, 24.0);
        assert_eq!(lights[0], Point::new(20.0 + 24.0 + 12.0, 48.0));
        assert_eq!(lights[1].x - lights[0].x, 40.0);
        assert!(WindowChrome::MacLight.bar_color().is_some());
        assert_eq!(WindowChrome::None.bar_height(), 0.0);
        assert!(layout(Size::new(1.0, 1.0), &flat(0.0)).traffic_lights(1.0).is_none());
    }

    #[test]
    fn auto_balance_lifts_by_half_shadow_offset() {
        let mut s = BeautifyStyle {
            padding: 50.0,
            shadow: Shadow { offset_y: 12.0, ..Shadow::default() },
            ..BeautifyStyle::default()
        };
        let off = layout(Size::new(100.0, 100.0), &BeautifyStyle { auto_balance: false, ..s.clone() });
        let on = layout(Size::new(100.0, 100.0), &s);
        assert_eq!(on.canvas, off.canvas);
        assert_eq!(on.balance_offset, 6.0);
        assert_eq!(on.content.min_y(), off.content.min_y() - 6.0);
        // No shadow or no padding room → no shift.
        s.shadow.enabled = false;
        assert_eq!(layout(Size::new(100.0, 100.0), &s).balance_offset, 0.0);
        s.shadow.enabled = true;
        s.padding = 0.0;
        assert_eq!(layout(Size::new(100.0, 100.0), &s).balance_offset, 0.0);
        // Upward shadows are not balanced.
        s.padding = 50.0;
        s.shadow.offset_y = -20.0;
        assert_eq!(layout(Size::new(100.0, 100.0), &s).balance_offset, 0.0);
    }

    #[test]
    fn validate_clamps_ranges() {
        let mut s = BeautifyStyle {
            padding: 9999.0,
            corner_radius: -4.0,
            shadow: Shadow { enabled: true, radius: 1e9, opacity: 3.0, offset_y: f64::NAN },
            background: Background::LinearGradient { from: Rgba::BLACK, to: Rgba::WHITE, angle_deg: -90.0 },
            ..BeautifyStyle::default()
        };
        s.validate();
        assert_eq!(s.padding, MAX_PADDING);
        assert_eq!(s.corner_radius, 0.0);
        assert_eq!(s.shadow.radius, MAX_SHADOW_RADIUS);
        assert_eq!(s.shadow.opacity, 1.0);
        assert_eq!(s.shadow.offset_y, Shadow::default().offset_y);
        assert!(matches!(s.background, Background::LinearGradient { angle_deg, .. } if angle_deg == 270.0));
        let mut n = BeautifyStyle { padding: f64::INFINITY, ..BeautifyStyle::default() };
        n.validate();
        assert_eq!(n.padding, BeautifyStyle::default().padding);
    }

    #[test]
    fn degenerate_and_huge_sizes_dont_panic() {
        let s =
            BeautifyStyle { aspect: AspectRatio::R16x9, chrome: WindowChrome::MacLight, ..BeautifyStyle::default() };
        let z = layout(Size::ZERO, &s);
        assert!(z.canvas.width.is_finite() && z.canvas.height > 0.0);
        let n = layout_at_scale(Size::new(f64::NAN, -5.0), &flat(0.0), f64::NAN);
        assert_eq!(n.canvas, Size::ZERO);
        assert_eq!(n.pixel_size(), (0, 0));
        assert_eq!(n.corner_radius, 0.0);
        let h = layout_at_scale(Size::new(1e12, f64::INFINITY), &s, 1e9);
        assert!(h.canvas.width.is_finite() && h.canvas.height.is_finite());
        let (w, hh) = h.pixel_size();
        assert!(w > 0 && hh > 0);
    }

    #[test]
    fn corner_radius_clamped_to_frame() {
        let s = BeautifyStyle { corner_radius: 64.0, ..flat(0.0) };
        assert_eq!(layout(Size::new(40.0, 20.0), &s).corner_radius, 10.0);
        assert_eq!(layout(Size::new(400.0, 200.0), &s).corner_radius, 64.0);
    }

    #[test]
    fn scaled_multiplies_lengths() {
        let s = BeautifyStyle::default().scaled(2.0);
        let d = BeautifyStyle::default();
        assert_eq!(s.padding, d.padding * 2.0);
        assert_eq!(s.shadow.offset_y, d.shadow.offset_y * 2.0);
        assert_eq!(s.shadow.opacity, d.shadow.opacity);
        assert_eq!(BeautifyStyle::default().scaled(f64::NAN), d);
    }

    #[test]
    fn gradient_endpoints_follow_css() {
        let r = Rect::new(0.0, 0.0, 100.0, 50.0);
        let (a, b) = gradient_endpoints(r, 90.0);
        assert!((a.x - 0.0).abs() < 1e-9 && (b.x - 100.0).abs() < 1e-9 && (a.y - 25.0).abs() < 1e-9);
        let (a, b) = gradient_endpoints(r, 180.0);
        assert!((a.y - 0.0).abs() < 1e-9 && (b.y - 50.0).abs() < 1e-9);
        let (a, b) = gradient_endpoints(r, 0.0);
        assert!(a.y > b.y);
    }

    #[test]
    fn style_serde_round_trip() {
        #[derive(Serialize, Deserialize, PartialEq, Debug)]
        struct W {
            s: BeautifyStyle,
        }
        for bg in [
            Background::None,
            Background::Solid(Rgba::new(1, 2, 3, 4)),
            Preset::Aurora.background(),
            Background::Image(PathBuf::from("/Users/a/wall.jpg")),
        ] {
            let w = W {
                s: BeautifyStyle {
                    enabled: true,
                    background: bg,
                    chrome: WindowChrome::MacLight,
                    aspect: AspectRatio::R9x16,
                    ..BeautifyStyle::default()
                },
            };
            let t = toml::to_string(&w).unwrap();
            assert_eq!(toml::from_str::<W>(&t).unwrap(), w, "{t}");
        }
        let t = toml::to_string(&W { s: BeautifyStyle::default() }).unwrap();
        assert!(t.contains("aspect = \"auto\""), "{t}");
        assert!(toml::from_str::<W>("[s.background]\nkind = \"solid\"\nvalue = \"#zz\"\n").is_err());
    }

    #[test]
    fn labels_are_localised() {
        assert_eq!(AspectRatio::R9x16.label(), "9:16");
        assert_ne!(Preset::Ocean.name_key().text(Language::En), Preset::Ocean.name_key().text(Language::ZhHans));
        assert_eq!(WindowChrome::MacDark.name_key(), Msg::BeautifyChromeDark);
    }

    trait ContainsContent {
        fn contains_content(&self, r: &Rect) -> bool;
    }
    impl ContainsContent for Size {
        fn contains_content(&self, r: &Rect) -> bool {
            Rect::new(0.0, 0.0, self.width, self.height).contains_rect(r)
        }
    }

    #[test]
    fn aspect_fill_covers_and_centres() {
        let b = Rect::new(10.0, 20.0, 200.0, 100.0);
        // Square image into a 2:1 box: scaled to 200×200, centred vertically.
        let r = aspect_fill_rect(Size::new(50.0, 50.0), b);
        assert_eq!(r, Rect::new(10.0, -30.0, 200.0, 200.0));
        // Wide image into the box: height matches, width overflows.
        let r = aspect_fill_rect(Size::new(400.0, 100.0), b);
        assert_eq!(r, Rect::new(-90.0, 20.0, 400.0, 100.0));
        assert!(r.width() >= b.width() && r.height() >= b.height());
        assert_eq!(r.mid(), b.mid());
        // Exact ratio: identical.
        assert_eq!(aspect_fill_rect(Size::new(20.0, 10.0), b), b);
    }

    #[test]
    fn aspect_fill_degenerate_returns_bounds() {
        let b = Rect::new(0.0, 0.0, 100.0, 50.0);
        assert_eq!(aspect_fill_rect(Size::new(0.0, 10.0), b), b);
        assert_eq!(aspect_fill_rect(Size::new(f64::NAN, 10.0), b), b);
        assert_eq!(
            aspect_fill_rect(Size::new(10.0, 10.0), Rect::new(0.0, 0.0, 0.0, 5.0)),
            Rect::new(0.0, 0.0, 0.0, 5.0)
        );
    }

    #[test]
    fn y_up_flip_round_trips() {
        let r = Rect::new(5.0, 10.0, 30.0, 20.0);
        let up = rect_y_up(r, 100.0);
        assert_eq!(up, Rect::new(5.0, 70.0, 30.0, 20.0));
        assert_eq!(rect_y_up(up, 100.0), r);
        assert_eq!(point_y_up(Point::new(3.0, 0.0), 100.0), Point::new(3.0, 100.0));
        // The chrome bar (top of frame, y-down) ends up at the frame's top in y-up.
        let l = layout(Size::new(100.0, 50.0), &BeautifyStyle { chrome: WindowChrome::MacLight, ..flat(10.0) });
        let h = l.canvas.height;
        let bar = rect_y_up(l.chrome_bar.unwrap(), h);
        let frame = rect_y_up(l.frame, h);
        assert_eq!(bar.max_y(), frame.max_y());
        assert_eq!(rect_y_up(l.content, h).min_y(), frame.min_y());
    }

    #[test]
    fn gradient_endpoints_flip_with_canvas() {
        // 180° runs top→bottom in y-down; in y-up the start is the larger y.
        let rect = Rect::new(0.0, 0.0, 100.0, 100.0);
        let (a, b) = gradient_endpoints(rect, 180.0);
        let (a, b) = (point_y_up(a, 100.0), point_y_up(b, 100.0));
        assert!(a.y > b.y);
        assert!((a.y - 100.0).abs() < 1e-9 && b.y.abs() < 1e-9);
    }

    #[test]
    fn cg_shadow_points_down_in_y_up_space() {
        let s = Shadow { enabled: true, radius: 20.0, opacity: 0.5, offset_y: 10.0 };
        let p = s.cg_params(2.0).unwrap();
        assert_eq!(p, CgShadow { dx: 0.0, dy: -20.0, blur: 40.0, alpha: 0.5 });
        let up = Shadow { offset_y: -4.0, ..s }.cg_params(1.0).unwrap();
        assert_eq!(up.dy, 4.0);
    }

    #[test]
    fn cg_shadow_invisible_or_invalid() {
        let s = Shadow::default();
        assert!(Shadow { enabled: false, ..s }.cg_params(1.0).is_none());
        assert!(Shadow { opacity: 0.0, ..s }.cg_params(1.0).is_none());
        let p = Shadow { radius: 1e9, offset_y: f64::NAN, opacity: 2.0, enabled: true }.cg_params(f64::NAN).unwrap();
        assert_eq!(p.blur, MAX_SHADOW_RADIUS);
        assert_eq!(p.dy, -Shadow::default().offset_y);
        assert_eq!(p.alpha, 1.0);
    }
}
