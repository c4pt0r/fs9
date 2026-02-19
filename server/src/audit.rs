use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

/// Types of filesystem modification events.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EventType {
    Create,
    Write,
    Delete,
    Mkdir,
    Rename,
    Chmod,
    Truncate,
    Upload,
}

impl EventType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Create => "create",
            Self::Write => "write",
            Self::Delete => "delete",
            Self::Mkdir => "mkdir",
            Self::Rename => "rename",
            Self::Chmod => "chmod",
            Self::Truncate => "truncate",
            Self::Upload => "upload",
        }
    }
}

impl std::fmt::Display for EventType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for EventType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "create" => Ok(Self::Create),
            "write" => Ok(Self::Write),
            "delete" => Ok(Self::Delete),
            "mkdir" => Ok(Self::Mkdir),
            "rename" => Ok(Self::Rename),
            "chmod" => Ok(Self::Chmod),
            "truncate" => Ok(Self::Truncate),
            "upload" => Ok(Self::Upload),
            _ => Err(format!("unknown event type: {}", s)),
        }
    }
}

/// A single audit event recording a filesystem modification.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEvent {
    /// Unix timestamp (seconds) of the first occurrence in this window.
    pub timestamp: u64,
    /// Type of modification.
    pub event_type: EventType,
    /// Path that was modified.
    pub path: String,
    /// User who performed the action.
    pub user: String,
    /// Number of occurrences aggregated in this 1-second window.
    pub count: u64,
}

/// Default maximum number of events to retain per namespace.
pub const DEFAULT_MAX_EVENTS: usize = 100_000;

/// Duration of the dedup window in seconds.
const DEDUP_WINDOW_SECS: u64 = 1;

/// Per-namespace in-memory audit log with ring-buffer semantics and 1-second dedup.
pub struct AuditLog {
    inner: Mutex<AuditLogInner>,
}

struct AuditLogInner {
    events: VecDeque<AuditEvent>,
    max_events: usize,
}

impl AuditLog {
    /// Create a new audit log with the given maximum capacity.
    pub fn new(max_events: usize) -> Self {
        Self {
            inner: Mutex::new(AuditLogInner {
                events: VecDeque::with_capacity(max_events.min(4096)),
                max_events,
            }),
        }
    }

    /// Record a filesystem modification event.
    ///
    /// If the most recent event has the same `(path, event_type)` and its
    /// timestamp is within the 1-second dedup window, the existing event's
    /// `count` is incremented instead of appending a new entry.
    pub fn record(&self, event_type: EventType, path: &str, user: &str) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());

        // Check if we can dedup with the last event
        if let Some(last) = inner.events.back_mut() {
            if last.event_type == event_type
                && last.path == path
                && last.user == user
                && now.saturating_sub(last.timestamp) < DEDUP_WINDOW_SECS
            {
                last.count += 1;
                return;
            }
        }

        // Evict oldest if at capacity
        if inner.events.len() >= inner.max_events {
            inner.events.pop_front();
        }

        inner.events.push_back(AuditEvent {
            timestamp: now,
            event_type,
            path: path.to_string(),
            user: user.to_string(),
            count: 1,
        });
    }

    /// Query events with optional filters.
    ///
    /// Returns events in reverse chronological order (newest first), up to `limit`.
    pub fn query(
        &self,
        limit: usize,
        path_filter: Option<&str>,
        type_filter: Option<EventType>,
    ) -> Vec<AuditEvent> {
        let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());

        inner
            .events
            .iter()
            .rev()
            .filter(|e| {
                if let Some(p) = path_filter {
                    if !e.path.starts_with(p) {
                        return false;
                    }
                }
                if let Some(t) = type_filter {
                    if e.event_type != t {
                        return false;
                    }
                }
                true
            })
            .take(limit)
            .cloned()
            .collect()
    }

    /// Return total number of events stored.
    pub fn len(&self) -> usize {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .events
            .len()
    }
}

impl Default for AuditLog {
    fn default() -> Self {
        Self::new(DEFAULT_MAX_EVENTS)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_record_and_query() {
        let log = AuditLog::new(10);
        log.record(EventType::Create, "/foo.txt", "alice");
        log.record(EventType::Write, "/bar.txt", "bob");

        let events = log.query(10, None, None);
        assert_eq!(events.len(), 2);
        // Newest first
        assert_eq!(events[0].path, "/bar.txt");
        assert_eq!(events[1].path, "/foo.txt");
    }

    #[test]
    fn dedup_within_window() {
        let log = AuditLog::new(10);
        // Same path, same type, same user, within 1 second → dedup
        log.record(EventType::Write, "/foo.txt", "alice");
        log.record(EventType::Write, "/foo.txt", "alice");
        log.record(EventType::Write, "/foo.txt", "alice");

        let events = log.query(10, None, None);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].count, 3);
    }

    #[test]
    fn no_dedup_different_path() {
        let log = AuditLog::new(10);
        log.record(EventType::Write, "/foo.txt", "alice");
        log.record(EventType::Write, "/bar.txt", "alice");

        let events = log.query(10, None, None);
        assert_eq!(events.len(), 2);
    }

    #[test]
    fn ring_buffer_eviction() {
        let log = AuditLog::new(3);
        log.record(EventType::Create, "/a", "u");
        log.record(EventType::Create, "/b", "u");
        log.record(EventType::Create, "/c", "u");
        log.record(EventType::Create, "/d", "u");

        let events = log.query(10, None, None);
        assert_eq!(events.len(), 3);
        // Oldest (/a) should have been evicted
        assert_eq!(events[0].path, "/d");
        assert_eq!(events[2].path, "/b");
    }

    #[test]
    fn filter_by_path() {
        let log = AuditLog::new(10);
        log.record(EventType::Create, "/dir/a.txt", "u");
        log.record(EventType::Create, "/other/b.txt", "u");
        log.record(EventType::Create, "/dir/c.txt", "u");

        let events = log.query(10, Some("/dir/"), None);
        assert_eq!(events.len(), 2);
    }

    #[test]
    fn filter_by_type() {
        let log = AuditLog::new(10);
        log.record(EventType::Create, "/a", "u");
        log.record(EventType::Write, "/b", "u");
        log.record(EventType::Delete, "/c", "u");

        let events = log.query(10, None, Some(EventType::Write));
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].path, "/b");
    }

    #[test]
    fn event_type_roundtrip() {
        for &t in &[
            EventType::Create,
            EventType::Write,
            EventType::Delete,
            EventType::Mkdir,
            EventType::Rename,
            EventType::Chmod,
            EventType::Truncate,
            EventType::Upload,
        ] {
            let s = t.as_str();
            let parsed: EventType = s.parse().unwrap();
            assert_eq!(parsed, t);
        }
    }
}
