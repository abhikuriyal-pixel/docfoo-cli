//! In-memory event log behind the visualizer's polling endpoint.
//!
//! One query runs at a time. The hub keeps a bounded, monotonically numbered
//! log of frames (`start`, `stage`, `delta`, `done`, `cancelled`, `error`)
//! that the page reads with a cursor (`/api/events?since=N`). The log is
//! cleared when a new query begins so a page refresh mid-turn replays that
//! turn from the beginning instead of the whole session.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// Frames kept in memory; the page consumes a turn as it runs, so this only
/// bounds multi-tab lag and late joins.
const LOG_CAP: usize = 4096;

#[derive(Clone, Debug)]
pub struct LoggedEvent {
    pub id: u64,
    /// Event name (`stage`, `delta`, `done`, `cancelled`, `error`, `start`).
    pub event: &'static str,
    /// Serialized JSON payload (also carries a `type` field).
    pub data: String,
}

struct Inner {
    log: VecDeque<LoggedEvent>,
    next_id: u64,
    busy: bool,
    cancel: Option<Arc<AtomicBool>>,
}

pub struct EventHub {
    inner: Mutex<Inner>,
}

impl EventHub {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(Inner {
                log: VecDeque::new(),
                next_id: 1,
                busy: false,
                cancel: None,
            }),
        }
    }

    /// Append one frame.
    pub fn emit(&self, event: &'static str, data: String) {
        let mut inner = self.inner.lock().expect("event hub lock");
        let id = inner.next_id;
        inner.next_id += 1;
        inner.log.push_back(LoggedEvent { id, event, data });
        while inner.log.len() > LOG_CAP {
            inner.log.pop_front();
        }
    }

    /// Every frame newer than `cursor`.
    pub fn since(&self, cursor: u64) -> Vec<LoggedEvent> {
        self.inner
            .lock()
            .expect("event hub lock")
            .log
            .iter()
            .filter(|event| event.id > cursor)
            .cloned()
            .collect()
    }

    /// Claim the single query slot. The log starts fresh for the new turn.
    /// Returns `false` when a query is already running.
    pub fn begin_query(&self, cancel: Arc<AtomicBool>) -> bool {
        let mut inner = self.inner.lock().expect("event hub lock");
        if inner.busy {
            return false;
        }
        inner.busy = true;
        inner.cancel = Some(cancel);
        inner.log.clear();
        true
    }

    /// Release the query slot after `done` / `cancelled` / `error`.
    pub fn finish_query(&self) {
        let mut inner = self.inner.lock().expect("event hub lock");
        inner.busy = false;
        inner.cancel = None;
    }

    /// Request cancellation of the running query (no-op when idle).
    pub fn cancel(&self) {
        let inner = self.inner.lock().expect("event hub lock");
        if let Some(cancel) = inner.cancel.as_ref() {
            cancel.store(true, Ordering::SeqCst);
        }
    }

    pub fn is_busy(&self) -> bool {
        self.inner.lock().expect("event hub lock").busy
    }
}

impl Default for EventHub {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emits_monotonic_ids_and_reads_after_a_cursor() {
        let hub = EventHub::new();
        hub.emit("stage", "{\"n\":1}".to_string());
        hub.emit("stage", "{\"n\":2}".to_string());
        let all = hub.since(0);
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].id, 1);
        assert_eq!(all[1].id, 2);
        let tail = hub.since(1);
        assert_eq!(tail.len(), 1);
        assert_eq!(tail[0].id, 2);
        assert!(hub.since(2).is_empty());
    }

    #[test]
    fn a_new_query_clears_the_log_but_ids_stay_monotonic() {
        let hub = EventHub::new();
        hub.emit("stage", "{}".to_string());
        let cancel = Arc::new(AtomicBool::new(false));
        assert!(hub.begin_query(Arc::clone(&cancel)));
        assert!(hub.since(0).is_empty(), "the new turn starts clean");
        hub.emit("start", "{}".to_string());
        let events = hub.since(0);
        assert_eq!(events.len(), 1);
        assert!(events[0].id > 1, "ids never restart");
    }

    #[test]
    fn only_one_query_runs_at_a_time_and_cancel_flips_the_flag() {
        let hub = EventHub::new();
        let cancel = Arc::new(AtomicBool::new(false));
        assert!(hub.begin_query(Arc::clone(&cancel)));
        assert!(!hub.begin_query(Arc::new(AtomicBool::new(false))));
        hub.cancel();
        assert!(cancel.load(Ordering::SeqCst));
        hub.finish_query();
        assert!(!hub.is_busy());
        assert!(hub.begin_query(Arc::new(AtomicBool::new(false))));
    }
}
