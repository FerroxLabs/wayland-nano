//! Frame draw scheduling (design doc §4: `tui/frame_requester.rs` pattern,
//! ~50 lines): a coalesced redraw requester. Multiple requests collapse into
//! one `AppEvent::Redraw`; the app drains pending requests after each draw.

use tokio::sync::mpsc;

/// Cloneable handle widgets/tasks use to ask for a redraw.
#[derive(Clone, Debug)]
pub struct FrameRequester {
    tx: mpsc::UnboundedSender<()>,
}

impl FrameRequester {
    pub fn new(tx: mpsc::UnboundedSender<()>) -> Self {
        Self { tx }
    }

    /// Schedule a frame draw as soon as the loop gets to it. Coalesced with
    /// any other pending request.
    pub fn schedule_frame(&self) {
        let _ = self.tx.send(());
    }
}

/// Drain every pending request after a draw, so a burst of schedule_frame
/// calls costs one redraw (returns how many were coalesced).
pub fn drain_pending(rx: &mut mpsc::UnboundedReceiver<()>) -> usize {
    let mut n = 0;
    while rx.try_recv().is_ok() {
        n += 1;
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(flavor = "current_thread")]
    async fn requests_coalesce_into_one_draw() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let requester = FrameRequester::new(tx);
        requester.schedule_frame();
        requester.schedule_frame();
        requester.schedule_frame();
        // The loop takes the first...
        assert!(rx.recv().await.is_some());
        // ...and drains the rest after drawing.
        assert_eq!(drain_pending(&mut rx), 2);
        assert_eq!(drain_pending(&mut rx), 0);
    }
}
