//! Trackpad pinch.
//!
//! gpui 0.2 does not turn macOS magnify gestures (`NSEventTypeMagnify`) into
//! events at all, so they are picked up with an AppKit local event monitor.
//! The monitor runs on the main thread inside AppKit's event dispatch, where
//! it must not touch gpui state; it only forwards each gesture step's
//! magnification down a channel, which the workspace drains on its own
//! executor (see `Workspace::new`). The event itself is passed on untouched.

use futures::channel::mpsc::UnboundedReceiver;

/// Start forwarding pinch gestures. Returns the receiving end the first time
/// it is called; `None` afterwards, and on platforms without the gesture.
#[cfg(target_os = "macos")]
pub fn install() -> Option<UnboundedReceiver<f64>> {
    use block2::RcBlock;
    use objc2_app_kit::{NSEvent, NSEventMask};
    use std::ptr::NonNull;
    use std::sync::atomic::{AtomicBool, Ordering};

    static INSTALLED: AtomicBool = AtomicBool::new(false);
    if INSTALLED.swap(true, Ordering::SeqCst) {
        return None;
    }
    let (tx, rx) = futures::channel::mpsc::unbounded::<f64>();
    let block = RcBlock::new(move |event: NonNull<NSEvent>| -> *mut NSEvent {
        // SAFETY: AppKit hands the monitor a valid event for the duration of
        // the call.
        let magnification = unsafe { event.as_ref() }.magnification();
        let _ = tx.unbounded_send(magnification);
        event.as_ptr()
    });
    // SAFETY: the handler returns the event it was given, which is valid.
    let monitor = unsafe { NSEvent::addLocalMonitorForEventsMatchingMask_handler(NSEventMask::Magnify, &block) };
    // The monitor lasts as long as the app; never remove it.
    std::mem::forget(monitor);
    Some(rx)
}

#[cfg(not(target_os = "macos"))]
pub fn install() -> Option<UnboundedReceiver<f64>> {
    None
}

/// Combine a run of magnification steps into one zoom factor. Each step is a
/// relative change (`0.05` is 5% larger), so they multiply.
pub fn factor(steps: impl IntoIterator<Item = f64>) -> f32 {
    steps.into_iter().fold(1.0f64, |acc, m| acc * (1.0 + m).max(0.01)) as f32
}

#[cfg(test)]
mod tests {
    use super::factor;

    #[test]
    fn steps_multiply_and_cancel() {
        assert!((factor([0.1, 0.1]) - 1.21).abs() < 1e-6);
        assert!((factor([0.25, -0.2]) - 1.0).abs() < 1e-6);
        assert_eq!(factor([]), 1.0);
        // A wild step can never flip or zero the zoom.
        assert!(factor([-5.0]) > 0.0);
    }
}
