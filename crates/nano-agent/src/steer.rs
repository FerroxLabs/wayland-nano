//! Mid-turn steer input queue (C9 §3.2): turn-scoped, synchronized, bounded,
//! and lifecycle-bound to exactly one turn.
//!
//! Semantics (codex input_queue parity, honestly scoped):
//! - callers (the ACP request handler, any in-process host) hold clones of
//!   the [`SteerHandle`]; all enqueue/drain/close operations take the mutex
//!   — there is no lock-free fast path;
//! - every enqueue is acknowledged synchronously ([`EnqueueAck`]); a full or
//!   closed queue is a typed, visible rejection, never a silent drop;
//! - the engine drains ONLY at the turn loop top (never mid-tool-batch) and
//!   closes the queue exactly once when the turn ends — on completion,
//!   failure, or cancel — dropping every still-queued item WITH per-item
//!   notification through `on_drop` (nothing is dropped silently, nothing
//!   closed is ever drained);
//! - the handle carries its `turn_id`: the engine refuses to drain a handle
//!   that does not belong to the running turn, so a stale handle can never
//!   inject into a later turn.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard};

/// Who submitted a steer: the wire request id (ACP) or a composer-local id
/// (in-process hosts) — carried so drop notification reaches the right
/// caller.
pub type SubmitterId = String;

/// One queued steer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SteerItem {
    pub submitter: SubmitterId,
    pub text: String,
}

/// The synchronous enqueue acknowledgment. `Queued { position }` is the
/// submitter's proof of acceptance — it is what the ACP `session/steer`
/// response carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnqueueAck {
    /// Accepted; drains in FIFO order at the next loop top.
    Queued { position: usize },
    /// The turn ended / the queue is closed.
    RejectedClosed,
    /// The bounded capacity is reached.
    RejectedFull,
}

/// The default bounded capacity (design §3.2).
pub const DEFAULT_CAPACITY: usize = 32;

#[derive(Debug)]
struct SteerQueue {
    items: VecDeque<SteerItem>,
    open: bool,
    capacity: usize,
}

/// The shared synchronized queue handle (same ownership pattern as the
/// cancel flag): created per turn, cloned to callers.
#[derive(Clone)]
pub struct SteerHandle {
    inner: Arc<Mutex<SteerQueue>>,
    turn_id: String,
    /// Per-item drop notification, fired by `close` for every still-queued
    /// item. The host translates it (ACP: one later `session/update` notice
    /// per dropped steer; TUI: a typed note).
    on_drop: Arc<dyn Fn(SteerItem) + Send + Sync>,
}

impl std::fmt::Debug for SteerHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SteerHandle")
            .field("turn_id", &self.turn_id)
            .finish_non_exhaustive()
    }
}

impl SteerHandle {
    pub fn new(
        turn_id: impl Into<String>,
        capacity: usize,
        on_drop: Arc<dyn Fn(SteerItem) + Send + Sync>,
    ) -> Self {
        Self {
            inner: Arc::new(Mutex::new(SteerQueue {
                items: VecDeque::new(),
                open: true,
                capacity,
            })),
            turn_id: turn_id.into(),
            on_drop,
        }
    }

    /// A handle whose drop notification goes nowhere — tests and hosts
    /// without a notification channel.
    pub fn silent(turn_id: impl Into<String>) -> Self {
        Self::new(turn_id, DEFAULT_CAPACITY, Arc::new(|_| {}))
    }

    pub fn turn_id(&self) -> &str {
        &self.turn_id
    }

    fn lock(&self) -> MutexGuard<'_, SteerQueue> {
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Synchronous, acknowledged enqueue. Closed and full queues reject
    /// loudly; positions are 1-based (the drained order).
    pub fn enqueue(&self, submitter: SubmitterId, text: String) -> EnqueueAck {
        let mut queue = self.lock();
        if !queue.open {
            return EnqueueAck::RejectedClosed;
        }
        if queue.items.len() >= queue.capacity {
            return EnqueueAck::RejectedFull;
        }
        queue.items.push_back(SteerItem { submitter, text });
        EnqueueAck::Queued {
            position: queue.items.len(),
        }
    }

    pub fn has_pending(&self) -> bool {
        !self.lock().items.is_empty()
    }

    /// Engine-side drain (loop top only). A handle bound to a DIFFERENT turn
    /// drains nothing — a stale handle can never inject.
    pub(crate) fn drain_for(&self, turn_id: &str) -> Vec<SteerItem> {
        if self.turn_id != turn_id {
            return Vec::new();
        }
        let mut queue = self.lock();
        if !queue.open {
            return Vec::new();
        }
        queue.items.drain(..).collect()
    }

    /// Close the queue exactly once per turn (idempotent): every still-
    /// queued item is dropped WITH per-item notification. After close,
    /// enqueue rejects and drain yields nothing.
    pub fn close(&self) {
        let dropped: Vec<SteerItem> = {
            let mut queue = self.lock();
            if !queue.open {
                return;
            }
            queue.open = false;
            queue.items.drain(..).collect()
        };
        for item in dropped {
            (self.on_drop)(item);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex as StdMutex;

    #[test]
    fn enqueue_acks_carry_fifo_positions() {
        let handle = SteerHandle::silent("t1");
        assert_eq!(
            handle.enqueue("a".into(), "one".into()),
            EnqueueAck::Queued { position: 1 }
        );
        assert_eq!(
            handle.enqueue("b".into(), "two".into()),
            EnqueueAck::Queued { position: 2 }
        );
        let items = handle.drain_for("t1");
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].text, "one");
        assert_eq!(items[1].text, "two");
        assert!(!handle.has_pending());
    }

    #[test]
    fn closed_queue_rejects_and_never_drains() {
        let drops = Arc::new(StdMutex::new(Vec::new()));
        let handle = SteerHandle::new("t1", 8, {
            let drops = drops.clone();
            Arc::new(move |item| drops.lock().unwrap().push(item))
        });
        handle.enqueue("a".into(), "pending".into());
        handle.close();
        assert_eq!(
            handle.enqueue("b".into(), "late".into()),
            EnqueueAck::RejectedClosed
        );
        assert!(handle.drain_for("t1").is_empty(), "closed never drains");
        // The queued item was dropped WITH notification, exactly once.
        let drops = drops.lock().unwrap();
        assert_eq!(drops.len(), 1);
        assert_eq!(drops[0].submitter, "a");
        assert_eq!(drops[0].text, "pending");
    }

    #[test]
    fn close_is_idempotent_and_notifies_each_drop_once() {
        let drops = Arc::new(StdMutex::new(Vec::new()));
        let handle = SteerHandle::new("t1", 8, {
            let drops = drops.clone();
            Arc::new(move |item| drops.lock().unwrap().push(item))
        });
        handle.enqueue("a".into(), "one".into());
        handle.enqueue("b".into(), "two".into());
        handle.close();
        handle.close();
        assert_eq!(drops.lock().unwrap().len(), 2);
    }

    #[test]
    fn bounded_capacity_rejects_full_loudly() {
        let handle = SteerHandle::new("t1", 2, Arc::new(|_| {}));
        handle.enqueue("a".into(), "1".into());
        handle.enqueue("b".into(), "2".into());
        assert_eq!(
            handle.enqueue("c".into(), "3".into()),
            EnqueueAck::RejectedFull
        );
        // The rejected item is not queued; the two accepted ones drain.
        assert_eq!(handle.drain_for("t1").len(), 2);
    }

    #[test]
    fn stale_turn_handle_can_never_inject() {
        let handle = SteerHandle::silent("turn-1");
        handle.enqueue("a".into(), "stale".into());
        assert!(
            handle.drain_for("turn-2").is_empty(),
            "a mismatched turn drains nothing"
        );
        // …and the item stays put for its OWN turn (or close-notification).
        assert_eq!(handle.drain_for("turn-1").len(), 1);
    }
}
