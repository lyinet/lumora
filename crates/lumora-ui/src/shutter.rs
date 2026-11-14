//! The shutter: a brief white flash over the captured region, then the
//! capture shrinks and flies into the Quick Access card on a spring.

use lumora_core::geometry::Rect;
use lumora_core::layout::fly_transform;
use objc2::rc::Retained;
use objc2::MainThreadMarker;
use objc2_app_kit::{NSPanel, NSScreen, NSStatusWindowLevel};
use objc2_core_graphics::CGImage;
use objc2_foundation::{NSArray, NSNumber, NSString};
use objc2_quartz_core::{
    kCAFillModeForwards, kCAGravityResizeAspect, CABasicAnimation, CAKeyframeAnimation, CALayer, CAMediaTiming,
};

use crate::tokens::{color, motion, radius};
use crate::util::{cf_as_object, cg_color, cg_rect, make_panel, rect, spring_scalar, with_completion, PanelSpec};

/// Play the shutter. `from` is the captured region and `to` the card slot,
/// both in AppKit global points. `on_done` runs when the flight lands.
pub fn play(mtm: MainThreadMarker, image: &CGImage, from: Rect, to: Option<Rect>, on_done: impl Fn() + 'static) {
    // Host on the screen containing the capture.
    let screen =
        NSScreen::screens(mtm).iter().map(|s| rect(s.frame())).find(|s| s.contains(from.mid())).unwrap_or(from);
    let panel: Retained<NSPanel> = make_panel(
        mtm,
        cg_rect(screen),
        PanelSpec { level: NSStatusWindowLevel + 1, keyable: false, shadow: false, ignores_mouse: true },
    );
    let (view, root) = crate::util::layer_host_view(mtm, cg_rect(Rect::new(0.0, 0.0, screen.width(), screen.height())));
    panel.setContentView(Some(&view));
    panel.orderFrontRegardless();

    // AppKit global (y-up) → this view's y-down local space.
    let local = |r: Rect| Rect::new(r.min_x() - screen.min_x(), screen.max_y() - r.max_y(), r.width(), r.height());
    let from_l = local(from);

    let flash = CALayer::new();
    flash.setFrame(cg_rect(from_l));
    flash.setBackgroundColor(Some(&cg_color(color::FLASH)));
    flash.setOpacity(0.0);
    root.addSublayer(&flash);

    let shot = CALayer::new();
    shot.setFrame(cg_rect(from_l));
    shot.setContentsGravity(unsafe { kCAGravityResizeAspect });
    // SAFETY: CGImage is a valid layer `contents`.
    unsafe { shot.setContents(Some(cf_as_object(image))) };
    // Clip so the (implicitly animated) corner radius rounds the image as
    // it shrinks toward the card's thumbnail.
    shot.setMasksToBounds(true);
    root.addSublayer(&shot);

    let panel_for_done = panel.clone();
    with_completion(
        || {
            // Flash: 0 → 0.9 → 0.
            let flash_anim = CAKeyframeAnimation::animationWithKeyPath(Some(&NSString::from_str("opacity")));
            let (a, b, c) =
                (NSNumber::numberWithDouble(0.0), NSNumber::numberWithDouble(0.9), NSNumber::numberWithDouble(0.0));
            let values = NSArray::<objc2::runtime::AnyObject>::from_slice(&[&*a, &*b, &*c]);
            // SAFETY: an NSArray of NSNumbers for a scalar key path.
            unsafe { flash_anim.setValues(Some(&values)) };
            flash_anim.setKeyTimes(Some(&NSArray::from_retained_slice(&[
                NSNumber::numberWithDouble(0.0),
                NSNumber::numberWithDouble(0.25),
                NSNumber::numberWithDouble(1.0),
            ])));
            flash_anim.setDuration(motion::FLASH);
            flash.addAnimation_forKey(&flash_anim, Some(&NSString::from_str("flash")));

            if let Some(to) = to {
                // Fly: uniform scale + translation onto the card, then fade.
                let t = fly_transform(from_l, local(to));
                let pos = shot.position();
                spring_scalar(&shot, "position.x", pos.x, pos.x + t.dx, motion::FLY);
                spring_scalar(&shot, "position.y", pos.y, pos.y + t.dy, motion::FLY);
                spring_scalar(&shot, "transform.scale", 1.0, t.scale, motion::FLY);
                shot.setCornerRadius(radius::CARD_THUMB / t.scale.max(0.05));
                let fade = CABasicAnimation::animationWithKeyPath(Some(&NSString::from_str("opacity")));
                // SAFETY: NSNumber values for a scalar key path.
                unsafe {
                    fade.setFromValue(Some(&NSNumber::numberWithDouble(1.0)));
                    fade.setToValue(Some(&NSNumber::numberWithDouble(0.0)));
                    fade.setFillMode(kCAFillModeForwards);
                }
                fade.setRemovedOnCompletion(false);
                fade.setBeginTime(objc2_quartz_core::CACurrentMediaTime() + 0.28);
                fade.setDuration(0.14);
                shot.addAnimation_forKey(&fade, Some(&NSString::from_str("fade")));
            } else {
                shot.setHidden(true);
            }
        },
        move || {
            panel_for_done.orderOut(None);
            on_done();
        },
    );
}
