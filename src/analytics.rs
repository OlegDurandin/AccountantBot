use std::{
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use tokio::time::MissedTickBehavior;
use tracing::info;

pub struct Analytics {
    started_at: Instant,
    interval: Duration,
    vk_events: AtomicU64,
    ignored_events: AtomicU64,
    recognized_commands: AtomicU64,
    processed_commands: AtomicU64,
    duplicate_events: AtomicU64,
    invalid_commands: AtomicU64,
    errors: AtomicU64,
    reconnects: AtomicU64,
}

impl Analytics {
    pub fn new(interval: Duration) -> Self {
        Self {
            started_at: Instant::now(),
            interval,
            vk_events: AtomicU64::new(0),
            ignored_events: AtomicU64::new(0),
            recognized_commands: AtomicU64::new(0),
            processed_commands: AtomicU64::new(0),
            duplicate_events: AtomicU64::new(0),
            invalid_commands: AtomicU64::new(0),
            errors: AtomicU64::new(0),
            reconnects: AtomicU64::new(0),
        }
    }

    pub fn start_reporter(self: &Arc<Self>) {
        let analytics = Arc::clone(self);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(analytics.interval);
            interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
            interval.tick().await;
            loop {
                interval.tick().await;
                analytics.log_snapshot();
            }
        });
    }

    pub fn event_received(&self) {
        self.vk_events.fetch_add(1, Ordering::Relaxed);
    }

    pub fn event_ignored(&self) {
        self.ignored_events.fetch_add(1, Ordering::Relaxed);
    }

    pub fn command_recognized(&self) {
        self.recognized_commands.fetch_add(1, Ordering::Relaxed);
    }

    pub fn command_processed(&self) {
        self.processed_commands.fetch_add(1, Ordering::Relaxed);
    }

    pub fn duplicate_event(&self) {
        self.duplicate_events.fetch_add(1, Ordering::Relaxed);
    }

    pub fn invalid_command(&self) {
        self.invalid_commands.fetch_add(1, Ordering::Relaxed);
    }

    pub fn error(&self) {
        self.errors.fetch_add(1, Ordering::Relaxed);
    }

    pub fn reconnect(&self) {
        self.reconnects.fetch_add(1, Ordering::Relaxed);
    }

    fn log_snapshot(&self) {
        let snapshot = self.snapshot();
        info!(
            target: "analytics",
            uptime_seconds = snapshot.uptime_seconds,
            vk_events = snapshot.vk_events,
            ignored_events = snapshot.ignored_events,
            recognized_commands = snapshot.recognized_commands,
            processed_commands = snapshot.processed_commands,
            duplicate_events = snapshot.duplicate_events,
            invalid_commands = snapshot.invalid_commands,
            errors = snapshot.errors,
            reconnects = snapshot.reconnects,
            "application analytics snapshot"
        );
    }

    fn snapshot(&self) -> AnalyticsSnapshot {
        AnalyticsSnapshot {
            uptime_seconds: self.started_at.elapsed().as_secs(),
            vk_events: self.vk_events.load(Ordering::Relaxed),
            ignored_events: self.ignored_events.load(Ordering::Relaxed),
            recognized_commands: self.recognized_commands.load(Ordering::Relaxed),
            processed_commands: self.processed_commands.load(Ordering::Relaxed),
            duplicate_events: self.duplicate_events.load(Ordering::Relaxed),
            invalid_commands: self.invalid_commands.load(Ordering::Relaxed),
            errors: self.errors.load(Ordering::Relaxed),
            reconnects: self.reconnects.load(Ordering::Relaxed),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
struct AnalyticsSnapshot {
    uptime_seconds: u64,
    vk_events: u64,
    ignored_events: u64,
    recognized_commands: u64,
    processed_commands: u64,
    duplicate_events: u64,
    invalid_commands: u64,
    errors: u64,
    reconnects: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_counters() {
        let analytics = Analytics::new(Duration::from_secs(300));
        analytics.event_received();
        analytics.command_recognized();
        analytics.command_processed();
        analytics.duplicate_event();
        analytics.invalid_command();
        analytics.error();
        analytics.reconnect();

        let snapshot = analytics.snapshot();
        assert_eq!(snapshot.vk_events, 1);
        assert_eq!(snapshot.recognized_commands, 1);
        assert_eq!(snapshot.processed_commands, 1);
        assert_eq!(snapshot.duplicate_events, 1);
        assert_eq!(snapshot.invalid_commands, 1);
        assert_eq!(snapshot.errors, 1);
        assert_eq!(snapshot.reconnects, 1);
    }
}
