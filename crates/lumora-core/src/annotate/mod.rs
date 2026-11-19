//! The annotation editor's model, independent of AppKit.
//!
//! * [`style`] — the curated palette, three stroke widths and opacity.
//! * [`path`] — vector paths, Bézier sampling, freehand smoothing and the
//!   tapered arrow outline.
//! * [`shape`] — what each annotation kind stores; bounds, hit-testing,
//!   translation and handle-driven resizing.
//! * [`document`] — z-ordered annotations, crop, and undo/redo via the
//!   command pattern; numbered steps derive their numbers from it.
//! * [`render`] — the display list both the live layers and the flattened
//!   export are drawn from.
//! * [`editor`] — the pointer/keyboard interaction state machine.
//!
//! Coordinates are **document points**: y down, origin at the top-left of
//! the uncropped capture, one unit per point (`pixels / scale`).

pub mod document;
pub mod editor;
pub mod path;
pub mod render;
pub mod shape;
pub mod style;

pub use document::{Annotation, AnnotationId, Command, Document};
pub use editor::{Editor, Effect, Input, Key, Tool, ToolOptions};
pub use render::RenderOp;
pub use shape::{EditHandle, RedactMode, Shape};
pub use style::{StrokeWidth, Style, OPACITY_MAX, OPACITY_MIN, PALETTE};
