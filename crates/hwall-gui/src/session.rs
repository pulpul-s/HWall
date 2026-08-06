use crate::history::{HistoryStore, SharedHistory};
use hwall_app::{LogWorker, MIN_REFRESH_INTERVAL_MS, SensorRow};
use hwall_core::{
    CollectOptions, CollectionProfile, MonitorCollector, MonitorPoll, MonitorRequestResult,
    MonitorWorker, Snapshot, SnapshotStatistics,
};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Activity {
    Discovering,
    Live,
    Paused,
    Disconnected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum HealthRefreshReason {
    View,
    Manual,
    ElevatedManual,
}

impl HealthRefreshReason {
    const fn priority(self) -> u8 {
        match self {
            Self::View => 0,
            Self::Manual => 1,
            Self::ElevatedManual => 2,
        }
    }
}

#[derive(Debug, Default)]
pub(super) struct TickResult {
    pub(super) snapshot_changed: bool,
    pub(super) telemetry_sample_changed: bool,
    pub(super) activity_changed: bool,
    pub(super) logging_error: Option<String>,
    pub(super) storage_health_changed: bool,
}

#[derive(Debug, Clone)]
struct PendingHealthRefresh {
    device_ids: Vec<String>,
    reason: HealthRefreshReason,
}

fn normalize_refresh_interval(interval: Duration) -> Duration {
    interval.max(Duration::from_millis(MIN_REFRESH_INTERVAL_MS))
}

fn next_refresh_deadline(started: Instant, interval: Duration) -> Instant {
    started + interval
}

pub(super) struct Session {
    initialization: Option<Receiver<MonitorCollector>>,
    worker: Option<MonitorWorker>,
    snapshot: Snapshot,
    statistics: SnapshotStatistics,
    history: SharedHistory,
    interval: Duration,
    next_refresh: Instant,
    refresh_in_flight: bool,
    discard_in_flight_update: bool,
    force_rediscovery: bool,
    desired_sensitive: bool,
    pending_sensitive: Option<bool>,
    in_flight_sensitive: Option<bool>,
    pending_health_refresh: Option<PendingHealthRefresh>,
    in_flight_health_refresh: Option<PendingHealthRefresh>,
    disconnected: bool,
    paused: bool,
    logger: Option<LogWorker>,
    log_path: Option<PathBuf>,
}

impl Session {
    pub(super) fn spawn(
        interval: Duration,
        rediscover: Duration,
        health_interval: Duration,
        include_sensitive: bool,
        history_retention: Duration,
    ) -> Self {
        let interval = normalize_refresh_interval(interval);
        let (tx, rx) = mpsc::channel();
        let initialization = thread::Builder::new()
            .name("hwall-initial-discovery".to_owned())
            .spawn(move || {
                let options = CollectOptions {
                    profile: CollectionProfile::Full,
                    allow_helper_commands: true,
                    include_sensitive,
                    include_storage_health: false,
                };
                let collector = MonitorCollector::new(options, rediscover, health_interval);
                let _ = tx.send(collector);
            })
            .ok()
            .map(|_| rx);
        let disconnected = initialization.is_none();

        Self {
            initialization,
            worker: None,
            snapshot: Snapshot::new(),
            statistics: SnapshotStatistics::new(),
            history: HistoryStore::shared(interval, history_retention),
            interval,
            next_refresh: Instant::now() + interval,
            refresh_in_flight: false,
            discard_in_flight_update: false,
            force_rediscovery: false,
            desired_sensitive: include_sensitive,
            pending_sensitive: None,
            in_flight_sensitive: None,
            pending_health_refresh: None,
            in_flight_health_refresh: None,
            disconnected,
            paused: false,
            logger: None,
            log_path: None,
        }
    }

    pub(super) fn tick(&mut self) -> TickResult {
        let before = self.activity();
        let mut result = TickResult::default();
        self.finish_initialization(&mut result);
        self.receive_update(&mut result);
        self.request_refresh();
        if let Some(logger) = &self.logger {
            result.logging_error = logger.try_error();
        }
        result.activity_changed = before != self.activity();
        result
    }

    fn finish_initialization(&mut self, result: &mut TickResult) {
        let Some(receiver) = self.initialization.as_ref() else {
            return;
        };
        match receiver.try_recv() {
            Ok(collector) => {
                self.snapshot = collector.initial_snapshot();
                self.statistics.observe(&self.snapshot);
                self.history.borrow_mut().observe(&self.snapshot);
                match MonitorWorker::spawn(collector) {
                    Ok(worker) => self.worker = Some(worker),
                    Err(_) => self.disconnected = true,
                }
                self.initialization = None;
                self.next_refresh = Instant::now() + self.interval;
                result.snapshot_changed = true;
                result.telemetry_sample_changed = true;
            }
            Err(TryRecvError::Disconnected) => {
                self.initialization = None;
                self.disconnected = true;
            }
            Err(TryRecvError::Empty) => {}
        }
    }

    fn receive_update(&mut self, result: &mut TickResult) {
        let Some(worker) = &self.worker else {
            return;
        };
        loop {
            match worker.poll() {
                MonitorPoll::Update(update) => {
                    let storage_health_update = !update.storage_health_device_ids.is_empty();
                    if !storage_health_update {
                        self.refresh_in_flight = false;
                    }
                    let sensitive_changed = update.include_sensitive.is_some()
                        && self.in_flight_sensitive.take().is_some();
                    let sensitive_is_current =
                        update.include_sensitive == Some(self.desired_sensitive);
                    let storage_health_changed =
                        storage_health_update && self.in_flight_health_refresh.take().is_some();
                    let administrative_update =
                        storage_health_changed || (sensitive_changed && sensitive_is_current);
                    let apply = if sensitive_changed {
                        sensitive_is_current
                    } else {
                        administrative_update || (!self.paused && !self.discard_in_flight_update)
                    };
                    if !storage_health_update {
                        self.discard_in_flight_update = false;
                    }
                    if apply {
                        self.snapshot = update.snapshot;
                        if !administrative_update {
                            self.statistics.observe(&self.snapshot);
                            self.history.borrow_mut().observe(&self.snapshot);
                            result.telemetry_sample_changed = true;
                        }
                        result.snapshot_changed = true;
                    }
                    result.storage_health_changed |= storage_health_changed;
                }
                MonitorPoll::Idle => break,
                MonitorPoll::Disconnected => {
                    self.disconnected = true;
                    self.refresh_in_flight = false;
                    break;
                }
            }
        }
    }

    fn request_refresh(&mut self) {
        let Some(worker) = self.worker.as_ref() else {
            return;
        };
        if self.disconnected {
            return;
        }

        if let Some(include_sensitive) = self.pending_sensitive {
            if self.refresh_in_flight {
                return;
            }
            match worker.request_sensitive(include_sensitive) {
                MonitorRequestResult::Accepted => {
                    self.refresh_in_flight = true;
                    self.in_flight_sensitive = Some(include_sensitive);
                    self.pending_sensitive = None;
                }
                MonitorRequestResult::Busy => {}
                MonitorRequestResult::Disconnected => self.disconnected = true,
            }
            return;
        }

        if self.in_flight_health_refresh.is_none()
            && let Some(pending) = self.pending_health_refresh.clone()
        {
            let elevated = pending.reason == HealthRefreshReason::ElevatedManual;
            match worker.request_storage_health(pending.device_ids.clone(), elevated) {
                MonitorRequestResult::Accepted => {
                    self.in_flight_health_refresh = Some(pending);
                    self.pending_health_refresh = None;
                }
                MonitorRequestResult::Busy => {}
                MonitorRequestResult::Disconnected => self.disconnected = true,
            }
            return;
        }

        if self.paused || self.refresh_in_flight {
            return;
        }
        let now = Instant::now();
        if !self.force_rediscovery && now < self.next_refresh {
            return;
        }
        let force = self.force_rediscovery;
        match worker.request(force) {
            MonitorRequestResult::Accepted => {
                self.refresh_in_flight = true;
                self.force_rediscovery = false;
                self.next_refresh = next_refresh_deadline(now, self.interval);
            }
            MonitorRequestResult::Busy => {}
            MonitorRequestResult::Disconnected => self.disconnected = true,
        }
    }

    pub(super) fn activity(&self) -> Activity {
        if self.disconnected {
            Activity::Disconnected
        } else if self.initialization.is_some() {
            Activity::Discovering
        } else if self.paused {
            Activity::Paused
        } else {
            Activity::Live
        }
    }

    pub(super) fn snapshot(&self) -> &Snapshot {
        &self.snapshot
    }

    pub(super) fn statistics(&self) -> &SnapshotStatistics {
        &self.statistics
    }

    pub(super) fn history(&self) -> SharedHistory {
        self.history.clone()
    }

    pub(super) fn extended_history_count(&self) -> usize {
        self.history.borrow().extended_count()
    }

    pub(super) fn is_paused(&self) -> bool {
        self.paused
    }

    fn set_paused(&mut self, paused: bool) {
        if self.paused == paused {
            return;
        }
        self.paused = paused;
        if paused {
            self.discard_in_flight_update = self.refresh_in_flight;
        } else {
            self.next_refresh = Instant::now();
        }
    }

    pub(super) fn toggle_paused(&mut self) {
        self.set_paused(!self.paused);
    }

    pub(super) fn reset_statistics(&mut self) {
        self.statistics.reset_with(&self.snapshot);
        self.history.borrow_mut().reset_with(&self.snapshot);
    }

    pub(super) fn refresh_storage_health(
        &mut self,
        device_ids: Vec<String>,
        reason: HealthRefreshReason,
    ) {
        if device_ids.is_empty() {
            return;
        }
        let mut device_ids = device_ids;
        device_ids.sort();
        device_ids.dedup();

        device_ids.retain(|id| {
            !self
                .in_flight_health_refresh
                .as_ref()
                .is_some_and(|pending| {
                    pending.reason.priority() >= reason.priority()
                        && pending.device_ids.iter().any(|known| known == id)
                })
        });
        if reason == HealthRefreshReason::View {
            device_ids.retain(|id| {
                !self
                    .pending_health_refresh
                    .as_ref()
                    .is_some_and(|pending| pending.device_ids.iter().any(|known| known == id))
            });
        } else {
            device_ids.retain(|id| {
                !self.pending_health_refresh.as_ref().is_some_and(|pending| {
                    pending.reason == reason && pending.device_ids.iter().any(|known| known == id)
                })
            });
        }
        if device_ids.is_empty() {
            return;
        }

        match self.pending_health_refresh.as_mut() {
            Some(pending) if pending.reason == reason => {
                pending.device_ids.extend(device_ids);
                pending.device_ids.sort();
                pending.device_ids.dedup();
            }
            Some(pending) if pending.reason.priority() > reason.priority() => {}
            _ => {
                self.pending_health_refresh = Some(PendingHealthRefresh { device_ids, reason });
            }
        }
    }

    pub(super) fn rediscover(&mut self) {
        self.force_rediscovery = true;
        self.next_refresh = Instant::now();
    }

    pub(super) fn set_interval(&mut self, interval: Duration) {
        let interval = normalize_refresh_interval(interval);
        self.interval = interval;
        self.history.borrow_mut().set_expected_interval(interval);
        self.next_refresh = Instant::now() + interval;
    }

    pub(super) fn set_history_retention(&mut self, retention: Duration) {
        self.history.borrow_mut().set_global_retention(retention);
    }

    pub(super) fn set_identifying_information(&mut self, include_sensitive: bool) {
        self.desired_sensitive = include_sensitive;
        self.pending_sensitive = Some(include_sensitive);
    }

    pub(super) fn identifying_information_pending(&self) -> bool {
        self.pending_sensitive.is_some() || self.in_flight_sensitive.is_some()
    }

    pub(super) fn logging(&self) -> bool {
        self.logger.is_some()
    }

    pub(super) fn log_path(&self) -> Option<&Path> {
        self.log_path.as_deref()
    }

    pub(super) fn start_logging(
        &mut self,
        path: PathBuf,
        format: hwall_app::LogFormat,
    ) -> std::io::Result<()> {
        self.stop_logging();
        self.logger = Some(LogWorker::start(&path, format)?);
        self.log_path = Some(path);
        Ok(())
    }

    pub(super) fn log_rows(&self, rows: Vec<SensorRow>) -> bool {
        self.logger
            .as_ref()
            .is_none_or(|logger| logger.sample(self.snapshot.captured_at_unix_ms, rows))
    }

    pub(super) fn stop_logging(&mut self) {
        if let Some(logger) = self.logger.take() {
            logger.stop();
        }
        self.log_path = None;
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.stop_logging();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refresh_deadline_is_start_to_start() {
        let started = Instant::now();
        let interval = Duration::from_millis(200);
        let finished = started + Duration::from_millis(150);
        let deadline = next_refresh_deadline(started, interval);

        assert_eq!(
            deadline.saturating_duration_since(finished),
            Duration::from_millis(50),
        );
    }

    #[test]
    fn refresh_overrun_has_no_additional_delay() {
        let started = Instant::now();
        let interval = Duration::from_millis(200);
        let finished = started + Duration::from_millis(341);
        let deadline = next_refresh_deadline(started, interval);

        assert_eq!(deadline.saturating_duration_since(finished), Duration::ZERO,);
    }
}
