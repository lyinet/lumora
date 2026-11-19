//! Editable `.lumora` documents.
//!
//! A `.lumora` file keeps a capture re-editable: the **original** capture
//! (un-annotated, un-beautified) plus everything drawn on it, so reopening
//! it restores the editor exactly as it was saved.
//!
//! ## Container
//!
//! ```text
//! offset  size  content
//! 0       8     magic  b"LUMORA\0\x01"  (last byte = container version)
//! 8       4     header length N, u32 little-endian
//! 12      N     header, UTF-8 JSON (see [`Header`])
//! 12+N    ..    PNG bytes of the original capture, to end of file
//! ```
//!
//! ## Schema
//!
//! The JSON header carries `schema_version`. Older versions are upgraded by
//! [`migrate`] before deserialising; newer ones are rejected with
//! [`DocError::FutureSchema`] rather than half-read.
//!
//! The live annotation model ([`crate::annotate`]) deliberately has no serde
//! derives: the file format is a separate DTO layer ([`AnnotationDto`],
//! [`ShapeDto`], …) with lossless conversions both ways, so the in-memory
//! model can change without silently changing the file format.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::annotate::{
    Annotation, AnnotationId, ArrowHeads, Dash, Document, Fill, RedactMode, Shape, StrokeWidth, Style,
};
use crate::beautify::BeautifyStyle;
use crate::color::Rgb8;
use crate::geometry::{Point, Rect, Size};

/// File magic; the last byte is the container version.
pub const MAGIC: [u8; 8] = *b"LUMORA\0\x01";
/// The header schema this build writes.
pub const SCHEMA_VERSION: u32 = 2;
/// File extension (without the dot).
pub const EXTENSION: &str = "lumora";
/// Upper bound on the JSON header, so a corrupt length can't make us
/// allocate gigabytes.
pub const MAX_HEADER_LEN: usize = 64 * 1024 * 1024;
const PNG_SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1A, b'\n'];
const PREFIX_LEN: usize = MAGIC.len() + 4;

/// Why a `.lumora` file couldn't be read.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum DocError {
    #[error("not a Lumora document (bad magic)")]
    BadMagic,
    #[error("Lumora container version {0} is not supported")]
    UnsupportedContainer(u8),
    #[error("Lumora document is truncated")]
    Truncated,
    #[error("Lumora document header is too large ({0} bytes)")]
    HeaderTooLarge(usize),
    #[error("Lumora document header is not valid JSON: {0}")]
    Json(String),
    #[error(
        "Lumora document schema version {found} is newer than this app supports ({supported}); please update Lumora"
    )]
    FutureSchema { found: u32, supported: u32 },
    #[error("Lumora document is missing schema_version")]
    MissingSchemaVersion,
    #[error("no migration from schema version {0}")]
    NoMigration(u32),
    #[error("unknown annotation kind {0:?}")]
    UnknownAnnotationKind(String),
    #[error("invalid Lumora document: {0}")]
    Invalid(String),
    #[error("embedded image is not a PNG")]
    NotPng,
    #[error("{0}")]
    Io(String),
}

// -- DTOs ----------------------------------------------------------------------

/// `[x, y]` in document points.
pub type PointDto = [f64; 2];

fn pt(p: Point) -> PointDto {
    [p.x, p.y]
}
fn unpt(p: PointDto) -> Point {
    Point::new(p[0], p[1])
}

/// A rectangle in document points.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RectDto {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl From<Rect> for RectDto {
    fn from(r: Rect) -> Self {
        RectDto { x: r.origin.x, y: r.origin.y, w: r.size.width, h: r.size.height }
    }
}
impl From<RectDto> for Rect {
    fn from(r: RectDto) -> Self {
        Rect::new(r.x, r.y, r.w, r.h)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WidthDto {
    Thin,
    Medium,
    Thick,
}

impl From<StrokeWidth> for WidthDto {
    fn from(w: StrokeWidth) -> Self {
        match w {
            StrokeWidth::Thin => WidthDto::Thin,
            StrokeWidth::Medium => WidthDto::Medium,
            StrokeWidth::Thick => WidthDto::Thick,
        }
    }
}
impl From<WidthDto> for StrokeWidth {
    fn from(w: WidthDto) -> Self {
        match w {
            WidthDto::Thin => StrokeWidth::Thin,
            WidthDto::Medium => StrokeWidth::Medium,
            WidthDto::Thick => StrokeWidth::Thick,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RedactModeDto {
    Pixelate,
    Blur,
}

impl From<RedactMode> for RedactModeDto {
    fn from(m: RedactMode) -> Self {
        match m {
            RedactMode::Pixelate => RedactModeDto::Pixelate,
            RedactMode::Blur => RedactModeDto::Blur,
        }
    }
}
impl From<RedactModeDto> for RedactMode {
    fn from(m: RedactModeDto) -> Self {
        match m {
            RedactModeDto::Pixelate => RedactMode::Pixelate,
            RedactModeDto::Blur => RedactMode::Blur,
        }
    }
}

/// Fill of a rectangle / ellipse (schema 2+).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FillDto {
    #[default]
    None,
    Solid,
    Translucent,
}

impl From<Fill> for FillDto {
    fn from(f: Fill) -> Self {
        match f {
            Fill::None => FillDto::None,
            Fill::Solid => FillDto::Solid,
            Fill::Translucent => FillDto::Translucent,
        }
    }
}
impl From<FillDto> for Fill {
    fn from(f: FillDto) -> Self {
        match f {
            FillDto::None => Fill::None,
            FillDto::Solid => Fill::Solid,
            FillDto::Translucent => Fill::Translucent,
        }
    }
}

/// Arrowheads (schema 2+).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HeadsDto {
    None,
    #[default]
    Filled,
    Open,
    Both,
}

impl From<ArrowHeads> for HeadsDto {
    fn from(h: ArrowHeads) -> Self {
        match h {
            ArrowHeads::None => HeadsDto::None,
            ArrowHeads::Filled => HeadsDto::Filled,
            ArrowHeads::Open => HeadsDto::Open,
            ArrowHeads::Both => HeadsDto::Both,
        }
    }
}
impl From<HeadsDto> for ArrowHeads {
    fn from(h: HeadsDto) -> Self {
        match h {
            HeadsDto::None => ArrowHeads::None,
            HeadsDto::Filled => ArrowHeads::Filled,
            HeadsDto::Open => ArrowHeads::Open,
            HeadsDto::Both => ArrowHeads::Both,
        }
    }
}

/// Stroke pattern (schema 2+).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DashDto {
    #[default]
    Solid,
    Dashed,
    Dotted,
}

impl From<Dash> for DashDto {
    fn from(d: Dash) -> Self {
        match d {
            Dash::Solid => DashDto::Solid,
            Dash::Dashed => DashDto::Dashed,
            Dash::Dotted => DashDto::Dotted,
        }
    }
}
impl From<DashDto> for Dash {
    fn from(d: DashDto) -> Self {
        match d {
            DashDto::Solid => Dash::Solid,
            DashDto::Dashed => Dash::Dashed,
            DashDto::Dotted => Dash::Dotted,
        }
    }
}

/// The geometry of one annotation, tagged by `"kind"`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ShapeDto {
    Arrow {
        start: PointDto,
        end: PointDto,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        control: Option<PointDto>,
    },
    Line {
        start: PointDto,
        end: PointDto,
    },
    Rect {
        rect: RectDto,
        #[serde(default)]
        rounded: bool,
    },
    Ellipse {
        rect: RectDto,
    },
    Pen {
        points: Vec<PointDto>,
    },
    Highlighter {
        points: Vec<PointDto>,
    },
    Text {
        origin: PointDto,
        text: String,
        /// Measured text size `[w, h]`; zero means "estimate".
        #[serde(default)]
        size: [f64; 2],
        #[serde(default)]
        background: bool,
    },
    Step {
        center: PointDto,
    },
    Redact {
        rect: RectDto,
        mode: RedactModeDto,
    },
    Spotlight {
        rect: RectDto,
        #[serde(default)]
        ellipse: bool,
    },
}

/// Every `kind` this build understands.
pub const KNOWN_KINDS: [&str; 10] =
    ["arrow", "line", "rect", "ellipse", "pen", "highlighter", "text", "step", "redact", "spotlight"];

impl From<&Shape> for ShapeDto {
    fn from(s: &Shape) -> Self {
        let pts = |v: &[Point]| v.iter().copied().map(pt).collect();
        match s {
            Shape::Arrow { start, end, control } => {
                ShapeDto::Arrow { start: pt(*start), end: pt(*end), control: control.map(pt) }
            }
            Shape::Line { start, end } => ShapeDto::Line { start: pt(*start), end: pt(*end) },
            Shape::Rect { rect, rounded } => ShapeDto::Rect { rect: (*rect).into(), rounded: *rounded },
            Shape::Ellipse { rect } => ShapeDto::Ellipse { rect: (*rect).into() },
            Shape::Pen { points } => ShapeDto::Pen { points: pts(points) },
            Shape::Highlighter { points } => ShapeDto::Highlighter { points: pts(points) },
            Shape::Text { origin, text, size, background } => ShapeDto::Text {
                origin: pt(*origin),
                text: text.clone(),
                size: [size.width, size.height],
                background: *background,
            },
            Shape::Step { center } => ShapeDto::Step { center: pt(*center) },
            Shape::Redact { rect, mode } => ShapeDto::Redact { rect: (*rect).into(), mode: (*mode).into() },
            Shape::Spotlight { rect, ellipse } => ShapeDto::Spotlight { rect: (*rect).into(), ellipse: *ellipse },
        }
    }
}

impl From<ShapeDto> for Shape {
    fn from(s: ShapeDto) -> Self {
        let pts = |v: Vec<PointDto>| v.into_iter().map(unpt).collect();
        match s {
            ShapeDto::Arrow { start, end, control } => {
                Shape::Arrow { start: unpt(start), end: unpt(end), control: control.map(unpt) }
            }
            ShapeDto::Line { start, end } => Shape::Line { start: unpt(start), end: unpt(end) },
            ShapeDto::Rect { rect, rounded } => Shape::Rect { rect: rect.into(), rounded },
            ShapeDto::Ellipse { rect } => Shape::Ellipse { rect: rect.into() },
            ShapeDto::Pen { points } => Shape::Pen { points: pts(points) },
            ShapeDto::Highlighter { points } => Shape::Highlighter { points: pts(points) },
            ShapeDto::Text { origin, text, size, background } => {
                Shape::Text { origin: unpt(origin), text, size: Size::new(size[0], size[1]), background }
            }
            ShapeDto::Step { center } => Shape::Step { center: unpt(center) },
            ShapeDto::Redact { rect, mode } => Shape::Redact { rect: rect.into(), mode: mode.into() },
            ShapeDto::Spotlight { rect, ellipse } => Shape::Spotlight { rect: rect.into(), ellipse },
        }
    }
}

/// One annotation: identity, style and geometry. The id is kept because
/// step badges are numbered by creation (id) order, not z-order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AnnotationDto {
    pub id: AnnotationId,
    /// `#RRGGBB`.
    pub color: String,
    pub width: WidthDto,
    /// Percent, 10–100 (schema 2+; the v1 → v2 migration writes 100).
    #[serde(default = "full_opacity")]
    pub opacity: u8,
    /// Rectangles and ellipses (schema 2+; migration writes `"none"`).
    #[serde(default)]
    pub fill: FillDto,
    /// Arrows (schema 2+; migration writes `"filled"`).
    #[serde(default)]
    pub heads: HeadsDto,
    /// Lines, arrows, rectangles, ellipses (schema 2+; migration writes `"solid"`).
    #[serde(default)]
    pub dash: DashDto,
    #[serde(flatten)]
    pub shape: ShapeDto,
}

fn full_opacity() -> u8 {
    crate::annotate::OPACITY_MAX
}

impl From<&Annotation> for AnnotationDto {
    fn from(a: &Annotation) -> Self {
        AnnotationDto {
            id: a.id,
            color: a.style.color.hex(),
            width: a.style.width.into(),
            opacity: a.style.opacity,
            fill: a.style.fill.into(),
            heads: a.style.heads.into(),
            dash: a.style.dash.into(),
            shape: (&a.shape).into(),
        }
    }
}

impl TryFrom<AnnotationDto> for Annotation {
    type Error = DocError;
    fn try_from(d: AnnotationDto) -> Result<Self, DocError> {
        let color = Rgb8::from_hex(&d.color)
            .ok_or_else(|| DocError::Invalid(format!("annotation {}: bad colour {:?}", d.id, d.color)))?;
        if !(crate::annotate::OPACITY_MIN..=crate::annotate::OPACITY_MAX).contains(&d.opacity) {
            return Err(DocError::Invalid(format!("annotation {}: opacity {} out of 10–100", d.id, d.opacity)));
        }
        let style = Style {
            color,
            width: d.width.into(),
            opacity: d.opacity,
            fill: d.fill.into(),
            heads: d.heads.into(),
            dash: d.dash.into(),
        };
        Ok(Annotation { id: d.id, shape: d.shape.into(), style })
    }
}

/// Pixel size and backing scale of the original capture.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ImageInfo {
    pub width_px: u32,
    pub height_px: u32,
    /// Pixels per point (2.0 on Retina).
    pub scale: f64,
}

impl ImageInfo {
    /// The image size in document points.
    pub fn size_points(&self) -> Size {
        Size::new(f64::from(self.width_px) / self.scale, f64::from(self.height_px) / self.scale)
    }

    fn validate(&self) -> Result<(), DocError> {
        if self.width_px == 0 || self.height_px == 0 {
            return Err(DocError::Invalid("image has zero size".into()));
        }
        if !(self.scale.is_finite() && self.scale > 0.0) {
            return Err(DocError::Invalid(format!("bad image scale {}", self.scale)));
        }
        Ok(())
    }
}

/// The JSON header.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Header {
    pub schema_version: u32,
    /// The Lumora version that wrote the file.
    pub app_version: String,
    /// Unix seconds (UTC).
    pub created_at: u64,
    pub image: ImageInfo,
    /// Bottom → top.
    #[serde(default)]
    pub annotations: Vec<AnnotationDto>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub beautify: Option<BeautifyStyle>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub crop: Option<RectDto>,
}

/// A decoded `.lumora` document.
#[derive(Debug, Clone, PartialEq)]
pub struct LumoraDoc {
    pub header: Header,
    /// The original capture, PNG-encoded.
    pub png: Vec<u8>,
}

impl LumoraDoc {
    /// Snapshot an editor document. `png` is the original capture, `image`
    /// its pixel size and scale, `created_at` unix seconds.
    pub fn from_editor_state(
        doc: &Document,
        png: Vec<u8>,
        image: ImageInfo,
        beautify: Option<BeautifyStyle>,
        created_at: u64,
    ) -> Self {
        LumoraDoc {
            header: Header {
                schema_version: SCHEMA_VERSION,
                app_version: env!("CARGO_PKG_VERSION").to_string(),
                created_at,
                image,
                annotations: doc.items().iter().map(AnnotationDto::from).collect(),
                beautify,
                crop: doc.crop().map(RectDto::from),
            },
            png,
        }
    }

    /// The annotations, bottom → top.
    pub fn to_annotations(&self) -> Result<Vec<Annotation>, DocError> {
        self.header.annotations.iter().cloned().map(Annotation::try_from).collect()
    }

    /// A fresh editor document (empty undo history) holding the saved
    /// annotations and crop; hand it to [`crate::annotate::Editor::with_document`].
    pub fn to_document(&self) -> Result<Document, DocError> {
        self.header.image.validate()?;
        Document::from_parts(self.header.image.size_points(), self.to_annotations()?, self.header.crop.map(Rect::from))
            .ok_or_else(|| DocError::Invalid("duplicate annotation ids".into()))
    }

    /// Serialise to the `.lumora` container.
    ///
    /// Non-finite coordinates are written as JSON `null` and will fail to
    /// decode; the editor never produces them.
    pub fn encode(&self) -> Vec<u8> {
        // Serialising plain data structs to a Vec cannot fail.
        let json = serde_json::to_vec(&self.header).unwrap_or_default();
        let mut out = Vec::with_capacity(PREFIX_LEN + json.len() + self.png.len());
        out.extend_from_slice(&MAGIC);
        out.extend_from_slice(&(json.len() as u32).to_le_bytes());
        out.extend_from_slice(&json);
        out.extend_from_slice(&self.png);
        out
    }

    /// Parse a `.lumora` container, migrating older schemas.
    pub fn decode(bytes: &[u8]) -> Result<LumoraDoc, DocError> {
        if bytes.len() < MAGIC.len() {
            return if MAGIC.starts_with(bytes) { Err(DocError::Truncated) } else { Err(DocError::BadMagic) };
        }
        if bytes[..7] != MAGIC[..7] {
            return Err(DocError::BadMagic);
        }
        if bytes[7] != MAGIC[7] {
            return Err(DocError::UnsupportedContainer(bytes[7]));
        }
        let len_bytes: [u8; 4] =
            bytes.get(8..PREFIX_LEN).ok_or(DocError::Truncated)?.try_into().map_err(|_| DocError::Truncated)?;
        let len = u32::from_le_bytes(len_bytes) as usize;
        if len > MAX_HEADER_LEN {
            return Err(DocError::HeaderTooLarge(len));
        }
        let json = bytes.get(PREFIX_LEN..PREFIX_LEN + len).ok_or(DocError::Truncated)?;
        let png = &bytes[PREFIX_LEN + len..];
        if png.is_empty() {
            return Err(DocError::Truncated);
        }
        if !png.starts_with(&PNG_SIGNATURE) {
            return Err(DocError::NotPng);
        }
        let header = parse_header(json)?;
        Ok(LumoraDoc { header, png: png.to_vec() })
    }
}

/// Parse and migrate a JSON header.
pub fn parse_header(json: &[u8]) -> Result<Header, DocError> {
    let value: Value = serde_json::from_slice(json).map_err(|e| DocError::Json(e.to_string()))?;
    let from = value.get("schema_version").and_then(Value::as_u64).ok_or(DocError::MissingSchemaVersion)?;
    let from = u32::try_from(from).unwrap_or(u32::MAX);
    let value = migrate(value, from)?;
    check_kinds(&value)?;
    let header: Header = serde_json::from_value(value).map_err(|e| DocError::Json(e.to_string()))?;
    header.image.validate()?;
    Ok(header)
}

/// Reject annotation kinds this build doesn't know, by name, before serde
/// turns them into a generic error.
fn check_kinds(value: &Value) -> Result<(), DocError> {
    let Some(list) = value.get("annotations").and_then(Value::as_array) else { return Ok(()) };
    for a in list {
        match a.get("kind").and_then(Value::as_str) {
            Some(k) if KNOWN_KINDS.contains(&k) => {}
            Some(k) => return Err(DocError::UnknownAnnotationKind(k.to_string())),
            None => return Err(DocError::Invalid("annotation without a kind".into())),
        }
    }
    Ok(())
}

/// One upgrade step: takes a header at version `n`, returns it at `n + 1`.
type MigrationStep = fn(Value) -> Result<Value, DocError>;

/// `MIGRATIONS[n]` upgrades schema `n` to `n + 1`.
const MIGRATIONS: [MigrationStep; SCHEMA_VERSION as usize] = [migrate_v0_to_v1, migrate_v1_to_v2];

/// Upgrade a header from schema `from` to [`SCHEMA_VERSION`], one step at a
/// time. Current-version input is returned unchanged; newer input is an error.
pub fn migrate(mut json: Value, from: u32) -> Result<Value, DocError> {
    if from > SCHEMA_VERSION {
        return Err(DocError::FutureSchema { found: from, supported: SCHEMA_VERSION });
    }
    for v in from..SCHEMA_VERSION {
        let step = MIGRATIONS.get(v as usize).ok_or(DocError::NoMigration(v))?;
        json = step(json)?;
    }
    if let Some(obj) = json.as_object_mut() {
        obj.insert("schema_version".into(), Value::from(SCHEMA_VERSION));
    }
    Ok(json)
}

/// v0 was the unreleased prototype layout: annotations under `"items"`,
/// tagged by `"type"`, with the image size as top-level `"width"` /
/// `"height"` pixels and an implicit 2× scale. No shipped build wrote it;
/// it exists to keep the chain exercised.
fn migrate_v0_to_v1(mut json: Value) -> Result<Value, DocError> {
    let obj = json.as_object_mut().ok_or_else(|| DocError::Invalid("header is not an object".into()))?;
    if let Some(mut items) = obj.remove("items") {
        if let Some(list) = items.as_array_mut() {
            for item in list.iter_mut().filter_map(Value::as_object_mut) {
                if let Some(t) = item.remove("type") {
                    item.insert("kind".into(), t);
                }
            }
        }
        obj.insert("annotations".into(), items);
    }
    if !obj.contains_key("image") {
        let w = obj.remove("width").unwrap_or(Value::from(0));
        let h = obj.remove("height").unwrap_or(Value::from(0));
        obj.insert("image".into(), serde_json::json!({ "width_px": w, "height_px": h, "scale": 2.0 }));
    }
    obj.entry("app_version").or_insert_with(|| Value::from("0.0.0"));
    obj.entry("created_at").or_insert_with(|| Value::from(0));
    Ok(json)
}

/// v2 (Lumora 0.8) added per-annotation style fields. v1 files had none of
/// them, so every annotation gets the value that reproduces its v1 look:
/// `opacity` 100, `fill` none, `heads` filled, `dash` solid.
fn migrate_v1_to_v2(mut json: Value) -> Result<Value, DocError> {
    let obj = json.as_object_mut().ok_or_else(|| DocError::Invalid("header is not an object".into()))?;
    if let Some(list) = obj.get_mut("annotations").and_then(Value::as_array_mut) {
        for item in list.iter_mut().filter_map(Value::as_object_mut) {
            item.entry("opacity").or_insert_with(|| Value::from(100));
            item.entry("fill").or_insert_with(|| Value::from("none"));
            item.entry("heads").or_insert_with(|| Value::from("filled"));
            item.entry("dash").or_insert_with(|| Value::from("solid"));
        }
    }
    Ok(json)
}

// -- files ---------------------------------------------------------------------

/// The `.lumora` document that sits next to a capture's exported image
/// (same directory, same stem).
pub fn doc_path_for(image: &Path) -> PathBuf {
    image.with_extension(EXTENSION)
}

/// The PNG export that sits next to a `.lumora` document.
pub fn export_path_for(doc: &Path) -> PathBuf {
    doc.with_extension("png")
}

/// Whether `path` names a `.lumora` document (extension, case-insensitive).
pub fn is_doc_path(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case(EXTENSION))
}

/// Name-clash test for a new capture saved as `image`: taken when either
/// the image or its `.lumora` sibling already exists, so a fresh capture
/// never overwrites an orphaned document. Pass it to
/// [`crate::filename::unique_path`] as `|p| pair_taken(p, exists)`.
pub fn pair_taken(image: &Path, exists: impl Fn(&Path) -> bool) -> bool {
    exists(image) || exists(&doc_path_for(image))
}

/// The hidden sibling an atomic write of `path` goes through:
/// `.<file name>.<pid>.tmp` in the same directory (so the final rename
/// never crosses volumes).
pub fn temp_path_for(path: &Path) -> PathBuf {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "document".into());
    path.with_file_name(format!(".{name}.{}.tmp", std::process::id()))
}

/// Write `bytes` to `path` atomically: write and sync a temp sibling, then
/// rename it over `path`. Readers see the old file or the new one, never a
/// partial write. The temp file is removed on failure.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = temp_path_for(path);
    let result = (|| {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// Read and decode a `.lumora` file. I/O failures come back as
/// [`DocError::Io`].
pub fn read(path: &Path) -> Result<LumoraDoc, DocError> {
    let bytes = std::fs::read(path).map_err(|e| DocError::Io(e.to_string()))?;
    LumoraDoc::decode(&bytes)
}

/// Read only the JSON header of a `.lumora` file (not the embedded PNG):
/// enough for the history window's dimensions and dates.
pub fn read_header(path: &Path) -> Result<Header, DocError> {
    use std::io::Read;
    let mut f = std::fs::File::open(path).map_err(|e| DocError::Io(e.to_string()))?;
    let mut prefix = [0u8; PREFIX_LEN];
    f.read_exact(&mut prefix).map_err(|_| DocError::Truncated)?;
    if prefix[..7] != MAGIC[..7] {
        return Err(DocError::BadMagic);
    }
    if prefix[7] != MAGIC[7] {
        return Err(DocError::UnsupportedContainer(prefix[7]));
    }
    let len = u32::from_le_bytes([prefix[8], prefix[9], prefix[10], prefix[11]]) as usize;
    if len > MAX_HEADER_LEN {
        return Err(DocError::HeaderTooLarge(len));
    }
    let mut json = vec![0u8; len];
    f.read_exact(&mut json).map_err(|_| DocError::Truncated)?;
    parse_header(&json)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::annotate::{Editor, PALETTE};
    use crate::beautify::{Background, WindowChrome};

    const PNG: &[u8] = &[0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1A, b'\n', 0, 0, 0, 13, b'I', b'H', b'D', b'R'];
    const IMAGE: ImageInfo = ImageInfo { width_px: 1600, height_px: 1000, scale: 2.0 };

    fn every_kind() -> Vec<Shape> {
        let p = Point::new;
        vec![
            Shape::Arrow { start: p(1.5, 2.25), end: p(300.0, 120.125), control: None },
            Shape::Arrow { start: p(10.0, 10.0), end: p(200.0, 50.0), control: Some(p(0.1, 0.2)) },
            Shape::Line { start: p(3.0, 4.0), end: p(5.0, 6.0) },
            Shape::Rect { rect: Rect::new(10.0, 20.0, 30.0, 40.0), rounded: false },
            Shape::Rect { rect: Rect::new(11.0, 21.0, -31.0, 41.0), rounded: true },
            Shape::Ellipse { rect: Rect::new(0.0, 0.0, 99.9, 1.0 / 3.0) },
            Shape::Pen { points: vec![p(0.0, 0.0), p(1.0 / 7.0, 2.0), p(3.0, 4.000_001)] },
            Shape::Highlighter { points: vec![p(5.0, 5.0), p(50.0, 5.0)] },
            Shape::Text {
                origin: p(40.0, 40.0),
                text: "Hello\n世界 \"quoted\" 🎉".into(),
                size: Size::ZERO,
                background: false,
            },
            Shape::Text { origin: p(41.0, 42.0), text: "pill".into(), size: Size::new(80.5, 27.5), background: true },
            Shape::Step { center: p(100.0, 100.0) },
            Shape::Redact { rect: Rect::new(1.0, 2.0, 3.0, 4.0), mode: RedactMode::Pixelate },
            Shape::Redact { rect: Rect::new(5.0, 6.0, 7.0, 8.0), mode: RedactMode::Blur },
            Shape::Spotlight { rect: Rect::new(9.0, 9.0, 90.0, 90.0), ellipse: false },
            Shape::Spotlight { rect: Rect::new(8.0, 8.0, 80.0, 80.0), ellipse: true },
        ]
    }

    fn sample_document() -> Document {
        let mut d = Document::new(IMAGE.size_points());
        for (i, shape) in every_kind().into_iter().enumerate() {
            let style = Style {
                opacity: [100, 40, 75][i % 3],
                fill: Fill::ALL[i % 3],
                heads: ArrowHeads::ALL[i % 4],
                dash: Dash::ALL[(i / 2) % 3],
                ..Style::new(PALETTE[i % PALETTE.len()], StrokeWidth::ALL[i % 3])
            };
            d.add(shape, style);
        }
        d
    }

    fn sample() -> LumoraDoc {
        LumoraDoc::from_editor_state(&sample_document(), PNG.to_vec(), IMAGE, None, 1_763_000_000)
    }

    fn with_header(json: &str) -> Vec<u8> {
        let mut out = MAGIC.to_vec();
        out.extend_from_slice(&(json.len() as u32).to_le_bytes());
        out.extend_from_slice(json.as_bytes());
        out.extend_from_slice(PNG);
        out
    }

    #[test]
    fn every_annotation_kind_round_trips_losslessly() {
        let src = sample_document();
        let doc = LumoraDoc::decode(&sample().encode()).unwrap();
        assert_eq!(doc.png, PNG);
        assert_eq!(doc.header.schema_version, SCHEMA_VERSION);
        assert_eq!(doc.header.app_version, env!("CARGO_PKG_VERSION"));
        assert_eq!(doc.header.created_at, 1_763_000_000);
        assert_eq!(doc.header.image, IMAGE);
        assert_eq!(doc.to_annotations().unwrap(), src.items());
        let kinds: std::collections::HashSet<_> = doc
            .header
            .annotations
            .iter()
            .map(|a| serde_json::to_value(&a.shape).unwrap()["kind"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(kinds.len(), KNOWN_KINDS.len(), "every kind exercised");
    }

    #[test]
    fn dto_conversions_are_inverse_per_kind() {
        for (i, shape) in every_kind().into_iter().enumerate() {
            let a = Annotation {
                id: i as u64 + 1,
                shape,
                style: Style {
                    opacity: 60,
                    fill: Fill::Translucent,
                    heads: ArrowHeads::Open,
                    dash: Dash::Dotted,
                    ..Style::new(PALETTE[3], StrokeWidth::Thick)
                },
            };
            let json = serde_json::to_string(&AnnotationDto::from(&a)).unwrap();
            let back: AnnotationDto = serde_json::from_str(&json).unwrap();
            assert_eq!(Annotation::try_from(back).unwrap(), a, "{json}");
        }
    }

    #[test]
    fn to_document_restores_crop_order_and_empty_history() {
        let mut src = sample_document();
        let top = src.items()[0].id;
        src.execute(crate::annotate::Command::Reorder { id: top, from: 0, to: src.items().len() - 1 });
        src.execute(crate::annotate::Command::Crop { before: None, after: Some(Rect::new(10.0, 10.0, 300.0, 200.0)) });
        let file = LumoraDoc::from_editor_state(&src, PNG.to_vec(), IMAGE, None, 0);
        let doc = LumoraDoc::decode(&file.encode()).unwrap().to_document().unwrap();
        assert_eq!(doc.items(), src.items());
        assert_eq!(doc.crop(), src.crop());
        assert_eq!(doc.size(), Size::new(800.0, 500.0));
        assert!(!doc.history().can_undo() && !doc.history().can_redo());
    }

    #[test]
    fn editor_with_document_continues_step_numbers_and_ids() {
        let mut src = Document::new(IMAGE.size_points());
        let s = Style::default();
        let a = src.add(Shape::Step { center: Point::new(1.0, 1.0) }, s);
        let b = src.add(Shape::Step { center: Point::new(2.0, 2.0) }, s);
        src.add(Shape::Line { start: Point::ZERO, end: Point::new(5.0, 5.0) }, s);
        src.remove(a);
        let file = LumoraDoc::from_editor_state(&src, PNG.to_vec(), IMAGE, None, 0);
        let restored = LumoraDoc::decode(&file.encode()).unwrap().to_document().unwrap();
        let editor = Editor::with_document(restored);
        let d = editor.document();
        assert_eq!(d.step_number(b), Some(1));
        assert_eq!(d.next_step_number(), 2);
        assert!(!d.history().can_undo());
        // New annotations never reuse a restored id.
        let max = d.items().iter().map(|x| x.id).max().unwrap();
        let mut doc = editor.document().clone();
        let new = doc.add(Shape::Step { center: Point::new(3.0, 3.0) }, s);
        assert!(new > max);
        assert_eq!(doc.step_number(new), Some(2));
    }

    #[test]
    fn beautify_is_optional_and_round_trips() {
        let plain = sample().encode();
        let text = String::from_utf8_lossy(&plain);
        assert!(!text.contains("\"beautify\""), "absent when None");
        assert!(LumoraDoc::decode(&plain).unwrap().header.beautify.is_none());

        let style = BeautifyStyle {
            enabled: true,
            background: Background::default(),
            padding: 48.5,
            chrome: WindowChrome::default(),
            ..BeautifyStyle::default()
        };
        let mut file = sample();
        file.header.beautify = Some(style.clone());
        assert_eq!(LumoraDoc::decode(&file.encode()).unwrap().header.beautify, Some(style));
    }

    #[test]
    fn bad_magic_is_rejected() {
        assert_eq!(LumoraDoc::decode(PNG), Err(DocError::BadMagic));
        assert_eq!(LumoraDoc::decode(b"xx"), Err(DocError::BadMagic));
        let mut bytes = sample().encode();
        bytes[7] = 9;
        assert_eq!(LumoraDoc::decode(&bytes), Err(DocError::UnsupportedContainer(9)));
    }

    #[test]
    fn truncated_files_are_rejected_at_every_length() {
        let bytes = sample().encode();
        let header_end = PREFIX_LEN + u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize;
        for cut in [0, 3, 8, 11, 12, header_end - 1, header_end] {
            assert_eq!(LumoraDoc::decode(&bytes[..cut]), Err(DocError::Truncated), "cut at {cut}");
        }
    }

    #[test]
    fn oversized_length_is_rejected_without_allocating() {
        let mut bytes = MAGIC.to_vec();
        bytes.extend_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(LumoraDoc::decode(&bytes), Err(DocError::HeaderTooLarge(u32::MAX as usize)));
    }

    #[test]
    fn corrupt_json_and_non_png_payload_are_errors() {
        assert!(matches!(LumoraDoc::decode(&with_header("{not json")), Err(DocError::Json(_))));
        assert!(matches!(LumoraDoc::decode(&with_header("\u{0}")), Err(DocError::Json(_))));
        assert_eq!(LumoraDoc::decode(&with_header("{}")), Err(DocError::MissingSchemaVersion));
        let mut bytes = sample().encode();
        let n = bytes.len();
        bytes[n - PNG.len()] = b'X';
        assert_eq!(LumoraDoc::decode(&bytes), Err(DocError::NotPng));
    }

    #[test]
    fn future_schema_is_rejected_clearly() {
        let json = r#"{"schema_version":3,"app_version":"9.9","created_at":0,"image":{"width_px":1,"height_px":1,"scale":1.0}}"#;
        let err = LumoraDoc::decode(&with_header(json)).unwrap_err();
        assert_eq!(err, DocError::FutureSchema { found: 3, supported: SCHEMA_VERSION });
        assert!(err.to_string().contains("update Lumora"));
    }

    #[test]
    fn unknown_kind_is_an_error_not_a_panic() {
        let json = r##"{"schema_version":1,"app_version":"0.3.0","created_at":0,
            "image":{"width_px":10,"height_px":10,"scale":1.0},
            "annotations":[{"id":1,"color":"#FF0000","width":"thin","kind":"sparkle","at":[1,2]}]}"##;
        assert_eq!(LumoraDoc::decode(&with_header(json)), Err(DocError::UnknownAnnotationKind("sparkle".into())));
        let bad_field = r##"{"schema_version":1,"app_version":"0.3.0","created_at":0,
            "image":{"width_px":10,"height_px":10,"scale":1.0},
            "annotations":[{"id":1,"color":"#FF0000","width":"huge","kind":"step","center":[1,2]}]}"##;
        assert!(matches!(LumoraDoc::decode(&with_header(bad_field)), Err(DocError::Json(_))));
    }

    #[test]
    fn invalid_values_are_reported() {
        let base = |image: &str, ann: &str| {
            format!(
                r#"{{"schema_version":1,"app_version":"0.3.0","created_at":0,"image":{image},"annotations":[{ann}]}}"#
            )
        };
        let zero = base(r#"{"width_px":0,"height_px":10,"scale":1.0}"#, "");
        assert!(matches!(LumoraDoc::decode(&with_header(&zero)), Err(DocError::Invalid(_))));
        let colour = base(
            r#"{"width_px":10,"height_px":10,"scale":1.0}"#,
            r#"{"id":1,"color":"chartreuse","width":"thin","kind":"step","center":[1,2]}"#,
        );
        let doc = LumoraDoc::decode(&with_header(&colour)).unwrap();
        assert!(matches!(doc.to_annotations(), Err(DocError::Invalid(_))));
        let step = r##"{"id":4,"color":"#FF0000","width":"thin","kind":"step","center":[1,2]}"##;
        let dup = base(r#"{"width_px":10,"height_px":10,"scale":1.0}"#, &format!("{step},{step}"));
        let doc = LumoraDoc::decode(&with_header(&dup)).unwrap();
        assert!(matches!(doc.to_document(), Err(DocError::Invalid(_))));
    }

    #[test]
    fn v0_prototype_header_migrates_to_current() {
        let v0 = serde_json::json!({
            "schema_version": 0,
            "width": 400, "height": 200,
            "items": [{ "id": 3, "type": "step", "color": "#0A84FF", "width": "medium", "center": [10.0, 20.0] }],
        });
        let v1 = migrate(v0.clone(), 0).unwrap();
        assert_eq!(v1["schema_version"], SCHEMA_VERSION);
        assert_eq!(v1["annotations"][0]["kind"], "step");
        assert!(v1.get("items").is_none());

        let doc = LumoraDoc::decode(&with_header(&v0.to_string())).unwrap();
        assert_eq!(doc.header.image, ImageInfo { width_px: 400, height_px: 200, scale: 2.0 });
        let items = doc.to_annotations().unwrap();
        assert_eq!(items[0].shape, Shape::Step { center: Point::new(10.0, 20.0) });
        assert_eq!(items[0].style.color, PALETTE[3]);
    }

    /// A header exactly as Lumora 0.7 (schema 1) wrote it.
    fn v1_header() -> serde_json::Value {
        serde_json::json!({
            "schema_version": 1, "app_version": "0.7.0", "created_at": 1_763_000_000,
            "image": { "width_px": 400, "height_px": 200, "scale": 2.0 },
            "annotations": [
                { "id": 1, "color": "#FF3B30", "width": "medium", "kind": "rect",
                  "rect": { "x": 1.0, "y": 2.0, "w": 30.0, "h": 40.0 }, "rounded": true },
                { "id": 2, "color": "#0A84FF", "width": "thick", "kind": "arrow", "start": [0.0, 0.0], "end": [50.0, 9.0] },
                { "id": 3, "color": "#FFCC00", "width": "thin", "kind": "highlighter", "points": [[0.0, 0.0], [9.0, 0.0]] },
            ],
        })
    }

    #[test]
    fn v1_header_migrates_to_v2_with_its_old_look() {
        let v2 = migrate(v1_header(), 1).unwrap();
        assert_eq!(v2["schema_version"], 2);
        for a in v2["annotations"].as_array().unwrap() {
            assert_eq!(a["opacity"], 100, "{a}");
            assert_eq!(a["fill"], "none", "{a}");
            assert_eq!(a["heads"], "filled", "{a}");
            assert_eq!(a["dash"], "solid", "{a}");
        }
        let doc = LumoraDoc::decode(&with_header(&v1_header().to_string())).unwrap();
        assert_eq!(doc.header.schema_version, SCHEMA_VERSION);
        let items = doc.to_annotations().unwrap();
        assert_eq!(items.len(), 3);
        for a in &items {
            assert_eq!(a.style.opacity, 100);
            assert_eq!(a.style.fill, Fill::None);
            assert_eq!(a.style.heads, ArrowHeads::Filled);
            assert_eq!(a.style.dash, Dash::Solid);
        }
        assert_eq!(items[0].style, Style::new(PALETTE[0], StrokeWidth::Medium));
        // Re-saving writes the current schema and reads back identically.
        let again = LumoraDoc::decode(&doc.encode()).unwrap();
        assert_eq!(again.to_annotations().unwrap(), items);
    }

    #[test]
    fn migration_keeps_explicit_v2_values() {
        let mut h = v1_header();
        h["annotations"][0]["opacity"] = serde_json::json!(35);
        let v2 = migrate(h, 1).unwrap();
        assert_eq!(v2["annotations"][0]["opacity"], 35);
        assert_eq!(v2["annotations"][1]["opacity"], 100);
    }

    #[test]
    fn out_of_range_opacity_is_invalid() {
        let mut h = v1_header();
        h["schema_version"] = serde_json::json!(2);
        h["annotations"][0]["opacity"] = serde_json::json!(5);
        let doc = LumoraDoc::decode(&with_header(&h.to_string())).unwrap();
        assert!(matches!(doc.to_annotations(), Err(DocError::Invalid(m)) if m.contains("opacity")));
    }

    #[test]
    fn current_version_migration_is_identity() {
        let header = serde_json::to_value(&sample().header).unwrap();
        assert_eq!(migrate(header.clone(), SCHEMA_VERSION).unwrap(), header);
        assert!(matches!(migrate(header, 7), Err(DocError::FutureSchema { found: 7, .. })));
    }

    #[test]
    fn large_header_round_trips() {
        let mut d = Document::new(IMAGE.size_points());
        let points: Vec<Point> =
            (0..50_000).map(|i| Point::new(f64::from(i) * 0.37, (f64::from(i) * 0.01).sin() * 300.0)).collect();
        for _ in 0..4 {
            d.add(Shape::Pen { points: points.clone() }, Style::default());
        }
        let file = LumoraDoc::from_editor_state(&d, PNG.to_vec(), IMAGE, None, 0);
        let bytes = file.encode();
        assert!(bytes.len() > 1_000_000, "header is {} bytes", bytes.len());
        let back = LumoraDoc::decode(&bytes).unwrap();
        assert_eq!(back.to_annotations().unwrap(), d.items());
        assert_eq!(back, file);
    }

    // -- files -------------------------------------------------------------------

    #[test]
    fn sibling_paths() {
        let png = Path::new("/caps/Lumora 2026-11-16 at 00.40.12.png");
        let doc = doc_path_for(png);
        assert_eq!(doc, Path::new("/caps/Lumora 2026-11-16 at 00.40.12.lumora"));
        assert_eq!(export_path_for(&doc), png);
        assert!(is_doc_path(&doc));
        assert!(is_doc_path(Path::new("a/B.LUMORA")));
        assert!(!is_doc_path(png));
        assert!(!is_doc_path(Path::new("lumora")));
    }

    #[test]
    fn pair_clash_checks_both_files() {
        let png = Path::new("/c/shot.png");
        assert!(!pair_taken(png, |_| false));
        assert!(pair_taken(png, |p| p == Path::new("/c/shot.png")));
        assert!(pair_taken(png, |p| p == Path::new("/c/shot.lumora")));
        // With unique_path: an orphaned document pushes the new capture on.
        let taken = [PathBuf::from("/c/shot.lumora")];
        let got = crate::filename::unique_path(Path::new("/c"), "shot", "png", |p| {
            pair_taken(p, |q| taken.iter().any(|t| t == q))
        });
        assert_eq!(got, Path::new("/c/shot (2).png"));
    }

    #[test]
    fn temp_path_is_a_hidden_sibling() {
        let t = temp_path_for(Path::new("/c/shot.lumora"));
        assert_eq!(t.parent(), Some(Path::new("/c")));
        let name = t.file_name().unwrap().to_string_lossy().into_owned();
        assert!(name.starts_with(".shot.lumora.") && name.ends_with(".tmp"), "{name}");
    }

    #[test]
    fn atomic_write_and_read_back() {
        let dir = std::env::temp_dir().join(format!("lumora-doc-test-{}", std::process::id()));
        let path = dir.join("nested").join("shot.lumora");
        let d = sample_document();
        let file = LumoraDoc::from_editor_state(&d, PNG.to_vec(), IMAGE, None, 7);
        write_atomic(&path, &file.encode()).unwrap();
        assert_eq!(read(&path).unwrap(), file);
        assert_eq!(read_header(&path).unwrap(), file.header, "header without the PNG");
        // Overwrite in place; no temp file is left behind.
        write_atomic(&path, b"garbage").unwrap();
        assert_eq!(read(&path), Err(DocError::BadMagic));
        assert_eq!(read_header(&path), Err(DocError::Truncated));
        std::fs::write(&path, b"garbage garbage garbage").unwrap();
        assert_eq!(read_header(&path), Err(DocError::BadMagic));
        let leftovers: Vec<_> = std::fs::read_dir(path.parent().unwrap()).unwrap().flatten().collect();
        assert_eq!(leftovers.len(), 1);
        assert!(matches!(read(&dir.join("missing.lumora")), Err(DocError::Io(_))));
        assert!(matches!(read_header(&dir.join("missing.lumora")), Err(DocError::Io(_))));
        std::fs::write(&path, &MAGIC[..5]).unwrap();
        assert_eq!(read_header(&path), Err(DocError::Truncated));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
