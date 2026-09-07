//! Delivering one session's output to every attached viewer.
//!
//! The rule this module exists to enforce: **a slow viewer must never slow
//! the agent down.** The pty reader thread is the one thing in a session that
//! cannot be allowed to block - on Windows a ConPTY whose output pipe stops
//! draining can deadlock at teardown, and on every platform a stalled phone
//! would otherwise apply backpressure all the way to the child process.
//!
//! So every subscriber gets a bounded queue and output is offered with
//! `try_send`. A queue that is full drops the frame and sets the
//! subscriber's `lagging` flag; the subscriber's own thread notices, clears
//! it, and asks for a fresh screen snapshot instead of the delta it missed.
//! Dropping output is correct here: a terminal's current screen is the truth,
//! and a snapshot is both smaller and more useful than a replayed backlog.
//!
//! LOCK ORDER, and the reason it is written down: a broadcast copies the
//! subscriber handles out under a short lock and sends OUTSIDE it. Nothing
//! in this module may hold the fanout lock while touching a session's
//! terminal state, or two threads doing the same in the other order deadlock.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::sync::{Arc, Mutex};

/// How many frames a viewer may fall behind before it is told to resync.
/// Deep enough to absorb a burst of scrollback, shallow enough that a dead
/// connection is noticed within a screenful.
const QUEUE_DEPTH: usize = 512;

/// One message on its way to a viewer.
#[derive(Debug)]
pub(crate) enum Frame {
    /// A pre-encoded binary frame (see `wire::encode_binary`).
    Binary(Vec<u8>),
    /// A JSON control frame.
    Text(String),
    /// The session is over; the writer should close the socket.
    Close,
}

/// The sending half held by the fanout, plus the flag the receiving thread
/// reads to discover it missed something.
#[derive(Debug)]
pub(crate) struct Subscriber {
    id: u64,
    tx: SyncSender<Arc<Frame>>,
    lagging: AtomicBool,
}

impl Subscriber {
    pub(crate) fn id(&self) -> u64 {
        self.id
    }

    /// Takes the lagging flag, clearing it. A `true` means this viewer
    /// missed output and needs a snapshot rather than the next delta.
    pub(crate) fn take_lagging(&self) -> bool {
        self.lagging.swap(false, Ordering::AcqRel)
    }
}

/// A viewer's end of the fanout: the queue to drain and the handle whose
/// lagging flag it must check.
pub(crate) struct Subscription {
    pub(crate) rx: Receiver<Arc<Frame>>,
    pub(crate) handle: Arc<Subscriber>,
}

#[derive(Debug, Default)]
pub(crate) struct Fanout {
    subscribers: Mutex<Vec<Arc<Subscriber>>>,
    next_id: AtomicU64,
}

impl Fanout {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn subscribe(&self) -> Subscription {
        let (tx, rx) = sync_channel(QUEUE_DEPTH);
        let handle = Arc::new(Subscriber {
            id: self.next_id.fetch_add(1, Ordering::Relaxed),
            tx,
            lagging: AtomicBool::new(false),
        });
        if let Ok(mut subscribers) = self.subscribers.lock() {
            subscribers.push(Arc::clone(&handle));
        }
        Subscription { rx, handle }
    }

    pub(crate) fn unsubscribe(&self, id: u64) {
        if let Ok(mut subscribers) = self.subscribers.lock() {
            subscribers.retain(|subscriber| subscriber.id != id);
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.subscribers.lock().map_or(0, |list| list.len())
    }

    /// Offers `frame` to every viewer, never blocking. A viewer whose queue
    /// is full is marked lagging and skipped; one whose receiver is gone is
    /// dropped from the list.
    pub(crate) fn broadcast(&self, frame: Frame) {
        let frame = Arc::new(frame);
        // Copied out under a short lock; the sends happen with the lock
        // released. See the lock-order note at the top of this module.
        let targets: Vec<Arc<Subscriber>> = match self.subscribers.lock() {
            Ok(subscribers) => subscribers.clone(),
            Err(_) => return,
        };

        let mut dead = Vec::new();
        for subscriber in targets {
            match subscriber.tx.try_send(Arc::clone(&frame)) {
                Ok(()) => {}
                Err(TrySendError::Full(_)) => {
                    subscriber.lagging.store(true, Ordering::Release);
                }
                Err(TrySendError::Disconnected(_)) => dead.push(subscriber.id),
            }
        }

        if !dead.is_empty()
            && let Ok(mut subscribers) = self.subscribers.lock()
        {
            subscribers.retain(|subscriber| !dead.contains(&subscriber.id));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(frame: &Frame) -> &str {
        match frame {
            Frame::Text(value) => value,
            other => panic!("expected text, got {other:?}"),
        }
    }

    #[test]
    fn a_broadcast_reaches_every_subscriber() {
        let fanout = Fanout::new();
        let first = fanout.subscribe();
        let second = fanout.subscribe();

        fanout.broadcast(Frame::Text("hello".to_owned()));

        assert_eq!(text(&first.rx.recv().unwrap()), "hello");
        assert_eq!(text(&second.rx.recv().unwrap()), "hello");
    }

    #[test]
    fn a_full_queue_marks_the_subscriber_lagging_instead_of_blocking() {
        // The property the pty reader depends on: broadcast returns even
        // when a viewer has stopped reading entirely.
        let fanout = Fanout::new();
        let stalled = fanout.subscribe();

        for index in 0..QUEUE_DEPTH + 10 {
            fanout.broadcast(Frame::Text(format!("{index}")));
        }

        assert!(stalled.handle.take_lagging());
        // And taking it clears it, so one missed burst asks for one resync.
        assert!(!stalled.handle.take_lagging());
    }

    #[test]
    fn a_lagging_subscriber_does_not_stop_a_healthy_one() {
        let fanout = Fanout::new();
        let stalled = fanout.subscribe();
        let healthy = fanout.subscribe();
        for index in 0..QUEUE_DEPTH + 10 {
            fanout.broadcast(Frame::Text(format!("{index}")));
            // The healthy viewer drains as it goes.
            let _ = healthy.rx.try_recv();
        }

        assert!(stalled.handle.take_lagging());
        assert_eq!(fanout.len(), 2, "neither subscriber was dropped");
    }

    #[test]
    fn a_dropped_receiver_is_removed_from_the_list() {
        let fanout = Fanout::new();
        let subscription = fanout.subscribe();
        assert_eq!(fanout.len(), 1);

        drop(subscription);
        fanout.broadcast(Frame::Text("anyone there".to_owned()));

        assert_eq!(fanout.len(), 0);
    }

    #[test]
    fn unsubscribe_removes_only_the_named_subscriber() {
        let fanout = Fanout::new();
        let first = fanout.subscribe();
        let second = fanout.subscribe();

        fanout.unsubscribe(first.handle.id());

        assert_eq!(fanout.len(), 1);
        fanout.broadcast(Frame::Text("still here".to_owned()));
        assert!(first.rx.try_recv().is_err());
        assert_eq!(text(&second.rx.recv().unwrap()), "still here");
    }
}
