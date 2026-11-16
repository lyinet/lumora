//! Icons drawn in code (no binary assets): the menu-bar template glyph, the
//! recording indicator, and the large onboarding mark.

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::Bool;
use objc2_app_kit::{NSBezierPath, NSColor, NSImage, NSLineCapStyle, NSLineJoinStyle};
use objc2_core_foundation::{CGPoint, CGRect, CGSize};

use crate::tokens::color;
use crate::util::ns_color;

/// Draw an image with a closure receiving the destination rect.
fn draw_image(size: f64, template: bool, draw: impl Fn(CGRect) + 'static) -> Retained<NSImage> {
    let block = RcBlock::new(move |rect: CGRect| -> Bool {
        draw(rect);
        Bool::YES
    });
    let image = NSImage::imageWithSize_flipped_drawingHandler(CGSize::new(size, size), false, &block);
    image.setTemplate(template);
    image
}

/// The Lumora mark: four viewfinder corner brackets around a lens dot.
/// `r` is the drawing rect; strokes scale with its size.
fn draw_mark(r: CGRect, stroke_color: &NSColor, dot_color: &NSColor) {
    let s = r.size.width;
    let lw = (s / 11.0).max(1.4);
    let inset = lw / 2.0 + s * 0.06;
    let arm = s * 0.28;
    let (x0, y0) = (r.origin.x + inset, r.origin.y + inset);
    let (x1, y1) = (r.origin.x + s - inset, r.origin.y + s - inset);
    let path = NSBezierPath::bezierPath();
    path.setLineWidth(lw);
    path.setLineCapStyle(NSLineCapStyle::Round);
    path.setLineJoinStyle(NSLineJoinStyle::Round);
    // Each corner: (corner point, horizontal direction, vertical direction).
    for (cx, cy, dx, dy) in [(x0, y0, 1.0, 1.0), (x1, y0, -1.0, 1.0), (x1, y1, -1.0, -1.0), (x0, y1, 1.0, -1.0)] {
        path.moveToPoint(CGPoint::new(cx + dx * arm, cy));
        path.lineToPoint(CGPoint::new(cx, cy));
        path.lineToPoint(CGPoint::new(cx, cy + dy * arm));
    }
    stroke_color.setStroke();
    path.stroke();
    // Lens: a filled dot with a small highlight notch cut out (even-odd).
    let d = s * 0.30;
    let dot_rect = CGRect::new(CGPoint::new(r.origin.x + (s - d) / 2.0, r.origin.y + (s - d) / 2.0), CGSize::new(d, d));
    let dot = NSBezierPath::bezierPathWithOvalInRect(dot_rect);
    dot_color.setFill();
    dot.fill();
}

/// A filled colour dot of `size` points (menu items for palette colours).
pub fn swatch_image(c: lumora_core::color::Rgb8, size: f64) -> Retained<NSImage> {
    draw_image(size, false, move |r| {
        let inset = CGRect::new(
            CGPoint::new(r.origin.x + 1.0, r.origin.y + 1.0),
            CGSize::new(r.size.width - 2.0, r.size.height - 2.0),
        );
        crate::util::ns_rgb8(c).setFill();
        NSBezierPath::bezierPathWithOvalInRect(inset).fill();
    })
}

/// 18 pt template image for the status item (adapts to light/dark menu bars).
pub fn status_icon() -> Retained<NSImage> {
    draw_image(18.0, true, |r| {
        let black = NSColor::blackColor();
        draw_mark(r, &black, &black);
    })
}

/// Red recording dot for the status item while recording (not a template:
/// it must stay red).
pub fn recording_dot() -> Retained<NSImage> {
    draw_image(18.0, false, |r| {
        let d = 9.0;
        let rect = CGRect::new(
            CGPoint::new(r.origin.x + (r.size.width - d) / 2.0, r.origin.y + (r.size.height - d) / 2.0),
            CGSize::new(d, d),
        );
        ns_color(color::RECORD).setFill();
        NSBezierPath::bezierPathWithOvalInRect(rect).fill();
    })
}

/// Large colour mark for onboarding and About.
pub fn app_mark(size: f64) -> Retained<NSImage> {
    draw_image(size, false, |r| {
        let bg = NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(r, r.size.width * 0.225, r.size.width * 0.225);
        ns_color(color::PILL_BG.with_alpha(1.0)).setFill();
        bg.fill();
        let inner = CGRect::new(
            CGPoint::new(r.origin.x + r.size.width * 0.2, r.origin.y + r.size.height * 0.2),
            CGSize::new(r.size.width * 0.6, r.size.height * 0.6),
        );
        draw_mark(inner, &NSColor::whiteColor(), &ns_color(color::ACCENT));
    })
}
