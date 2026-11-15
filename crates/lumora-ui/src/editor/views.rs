//! The editor's AppKit subclasses. Each holds only a `Weak` back-reference
//! to the editor state and forwards events to it, so the state owns the
//! views and never the other way round.

use std::cell::{Cell, RefCell};
use std::rc::Weak;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObjectProtocol, ProtocolObject};
use objc2::{define_class, msg_send, AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSCursor, NSDragOperation, NSDraggingContext, NSDraggingItem, NSDraggingSession, NSDraggingSource, NSEvent,
    NSPasteboardWriting, NSResponder, NSView, NSWindowDelegate,
};
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_foundation::{NSArray, NSNotification, NSObject};

use super::State;

/// Pointer phases forwarded to the state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pointer {
    Down,
    Drag,
    Up,
}

// ---------------------------------------------------------------------------
// Canvas
// ---------------------------------------------------------------------------

pub struct CanvasIvars {
    pub(super) state: RefCell<Weak<State>>,
}

define_class!(
    /// The drawing surface: flipped, layer-hosting, first responder.
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "LumoraEditorCanvas"]
    #[ivars = CanvasIvars]
    pub struct CanvasView;

    unsafe impl NSObjectProtocol for CanvasView {}

    impl CanvasView {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        #[unsafe(method(acceptsFirstResponder))]
        fn accepts_first_responder(&self) -> bool {
            true
        }

        #[unsafe(method(acceptsFirstMouse:))]
        fn accepts_first_mouse(&self, _event: Option<&NSEvent>) -> bool {
            true
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            self.pointer(event, Pointer::Down);
        }

        #[unsafe(method(mouseDragged:))]
        fn mouse_dragged(&self, event: &NSEvent) {
            self.pointer(event, Pointer::Drag);
        }

        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self, event: &NSEvent) {
            self.pointer(event, Pointer::Up);
        }

        #[unsafe(method(keyDown:))]
        fn key_down(&self, event: &NSEvent) {
            if let Some(s) = self.ivars().state.borrow().upgrade() {
                State::key_down(&s, event);
            }
        }

        #[unsafe(method(performKeyEquivalent:))]
        fn perform_key_equivalent(&self, event: &NSEvent) -> bool {
            match self.ivars().state.borrow().upgrade() {
                Some(s) => State::key_equivalent(&s, event),
                None => false,
            }
        }

        #[unsafe(method(setFrameSize:))]
        fn set_frame_size(&self, size: CGSize) {
            let _: () = unsafe { msg_send![super(self), setFrameSize: size] };
            if let Some(s) = self.ivars().state.borrow().upgrade() {
                s.layout();
            }
        }

        #[unsafe(method(resetCursorRects))]
        fn reset_cursor_rects(&self) {
            self.addCursorRect_cursor(self.bounds(), &NSCursor::crosshairCursor());
        }
    }
);

impl CanvasView {
    pub fn new(mtm: MainThreadMarker, frame: CGRect) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(CanvasIvars { state: RefCell::new(Weak::new()) });
        unsafe { msg_send![super(this), initWithFrame: frame] }
    }

    fn pointer(&self, event: &NSEvent, phase: Pointer) {
        let p = self.convertPoint_fromView(event.locationInWindow(), None);
        let shift = event.modifierFlags().contains(objc2_app_kit::NSEventModifierFlags::Shift);
        let clicks = if phase == Pointer::Down { event.clickCount().max(1) as u32 } else { 1 };
        if let Some(s) = self.ivars().state.borrow().upgrade() {
            State::pointer(&s, phase, (p.x, p.y), clicks, shift);
        }
    }
}

// ---------------------------------------------------------------------------
// Drag-out tile
// ---------------------------------------------------------------------------

pub struct DragTileIvars {
    pub(super) state: RefCell<Weak<State>>,
    down: Cell<Option<CGPoint>>,
    dragging: Cell<bool>,
}

define_class!(
    /// A toolbar glyph you drag into another app to drop the flattened PNG.
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "LumoraEditorDragTile"]
    #[ivars = DragTileIvars]
    pub struct DragTile;

    unsafe impl NSObjectProtocol for DragTile {}

    unsafe impl NSDraggingSource for DragTile {
        #[unsafe(method(draggingSession:sourceOperationMaskForDraggingContext:))]
        fn source_operation_mask(&self, _session: &NSDraggingSession, _context: NSDraggingContext) -> NSDragOperation {
            NSDragOperation::Copy
        }

        #[unsafe(method(draggingSession:endedAtPoint:operation:))]
        fn drag_ended(&self, _session: &NSDraggingSession, _point: CGPoint, _operation: NSDragOperation) {
            self.ivars().dragging.set(false);
        }
    }

    impl DragTile {
        #[unsafe(method(acceptsFirstMouse:))]
        fn accepts_first_mouse(&self, _event: Option<&NSEvent>) -> bool {
            true
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            self.ivars().down.set(Some(event.locationInWindow()));
        }

        #[unsafe(method(mouseDragged:))]
        fn mouse_dragged(&self, event: &NSEvent) {
            let Some(start) = self.ivars().down.get() else { return };
            let p = event.locationInWindow();
            if self.ivars().dragging.get() || (p.x - start.x).hypot(p.y - start.y) < 3.0 {
                return;
            }
            self.ivars().dragging.set(true);
            self.begin_drag(event);
        }

        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self, _event: &NSEvent) {
            self.ivars().down.set(None);
        }

        #[unsafe(method(resetCursorRects))]
        fn reset_cursor_rects(&self) {
            self.addCursorRect_cursor(self.bounds(), &NSCursor::openHandCursor());
        }
    }
);

impl DragTile {
    pub fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(DragTileIvars {
            state: RefCell::new(Weak::new()),
            down: Cell::new(None),
            dragging: Cell::new(false),
        });
        let this: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: CGRect::default()] };
        if let Some(img) = crate::util::symbol("hand.draw", "")
            .or_else(|| crate::util::symbol("arrow.up.and.down.and.arrow.left.and.right", ""))
        {
            let iv = objc2_app_kit::NSImageView::imageViewWithImage(&img, mtm);
            iv.setFrame(CGRect::new(CGPoint::new(7.0, 7.0), CGSize::new(16.0, 16.0)));
            iv.setContentTintColor(Some(&objc2_app_kit::NSColor::labelColor()));
            this.addSubview(&iv);
        }
        this
    }

    fn begin_drag(&self, event: &NSEvent) {
        let Some(state) = self.ivars().state.borrow().upgrade() else { return };
        let Some((path, image)) = state.export_for_drag() else {
            self.ivars().dragging.set(false);
            return;
        };
        let Some(url) = lumora_capture::clipboard::file_url(&path) else { return };
        let writer: Retained<ProtocolObject<dyn NSPasteboardWriting>> = ProtocolObject::from_retained(url);
        let item = NSDraggingItem::initWithPasteboardWriter(NSDraggingItem::alloc(), &writer);
        let size = image.size();
        let scale = (96.0 / size.width.max(size.height).max(1.0)).min(1.0);
        let b = self.bounds();
        let w = size.width * scale;
        let h = size.height * scale;
        let frame =
            CGRect::new(CGPoint::new(b.size.width / 2.0 - w / 2.0, b.size.height / 2.0 - h / 2.0), CGSize::new(w, h));
        // SAFETY: contents is an NSImage, as the API expects.
        unsafe { item.setDraggingFrame_contents(frame, Some(&*image as &AnyObject)) };
        let source = ProtocolObject::from_ref(self);
        let _ = self.beginDraggingSessionWithItems_event_source(&NSArray::from_retained_slice(&[item]), event, source);
    }
}

// ---------------------------------------------------------------------------
// Window delegate
// ---------------------------------------------------------------------------

pub struct DelegateIvars {
    pub(super) state: RefCell<Weak<State>>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "LumoraEditorWindowDelegate"]
    #[ivars = DelegateIvars]
    pub struct EditorWindowDelegate;

    unsafe impl NSObjectProtocol for EditorWindowDelegate {}

    unsafe impl NSWindowDelegate for EditorWindowDelegate {
        #[unsafe(method(windowWillClose:))]
        fn window_will_close(&self, _notification: &NSNotification) {
            if let Some(s) = self.ivars().state.borrow().upgrade() {
                s.window_closed();
            }
        }
    }
);

impl EditorWindowDelegate {
    pub fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(DelegateIvars { state: RefCell::new(Weak::new()) });
        unsafe { msg_send![super(this), init] }
    }
}

// ---------------------------------------------------------------------------
// Flipped container (beautify panel document view)
// ---------------------------------------------------------------------------

define_class!(
    /// A plain top-down view so panel rows can be laid out from the top.
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "LumoraEditorFlippedView"]
    pub struct FlippedView;

    unsafe impl NSObjectProtocol for FlippedView {}

    impl FlippedView {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }
    }
);

impl FlippedView {
    pub fn new(mtm: MainThreadMarker, frame: CGRect) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(());
        unsafe { msg_send![super(this), initWithFrame: frame] }
    }
}
