/// Wrapper asserting that an Objective-C object may cross threads.
///
/// objc2 conservatively marks most framework classes `!Send`. Some objects we
/// need to hand between ScreenCaptureKit's callback queues and the main
/// thread are documented immutable snapshots (`SCShareableContent`,
/// `SCWindow`, `SCDisplay`) or are only ever *used* from one serial queue at a
/// time (`AVAssetWriter` and its inputs). Each construction site states why
/// it is sound.
pub struct SendBox<T>(T);

// SAFETY: see the type-level documentation; constructing a `SendBox` is
// `unsafe` and every call site justifies it.
unsafe impl<T> Send for SendBox<T> {}
unsafe impl<T> Sync for SendBox<T> {}

impl<T> SendBox<T> {
    /// # Safety
    /// `value` must be safe to use from whichever thread ends up holding the
    /// box, given how the caller synchronises access.
    pub unsafe fn new(value: T) -> Self {
        SendBox(value)
    }

    pub fn into_inner(self) -> T {
        self.0
    }
}

impl<T> std::ops::Deref for SendBox<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.0
    }
}

impl<T> std::fmt::Debug for SendBox<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SendBox(..)")
    }
}
