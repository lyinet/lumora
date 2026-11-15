//! # lumora-core
//!
//! Everything in Lumora that does not need to talk to the operating system
//! lives here, so it can be unit-tested on any host (CI runs these tests on
//! Linux as well as macOS).
//!
//! Coordinate convention: unless stated otherwise, geometry in this crate is
//! expressed in **global Quartz display space** — points, origin at the
//! top-left of the primary display, `y` growing downwards. This matches what
//! ScreenCaptureKit reports for window frames and what `CGDisplayBounds`
//! returns, so capture code never has to flip. The UI layer converts AppKit's
//! bottom-left coordinates into this space at the edge (see
//! [`geometry::flip_y`]).

pub mod action;
pub mod annotate;
pub mod beautify;
pub mod color;
pub mod filename;
pub mod format;
pub mod geometry;
pub mod hotkey;
pub mod i18n;
pub mod layout;
pub mod lumora_doc;
pub mod ocr;
pub mod recording;
pub mod selection;
pub mod settings;
pub mod snap;
pub mod spring;
pub mod stitch;

pub use action::Action;
pub use geometry::{Point, Rect, Size};
pub use spring::Spring;
