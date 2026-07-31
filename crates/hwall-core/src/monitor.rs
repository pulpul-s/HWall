//! Nonblocking snapshot collection for realtime clients.
//!
//! [`MonitorWorker`] runs realtime telemetry and storage health on separate
//! worker threads. Terminal and GUI event loops can consume completed snapshots
//! without blocking input.

use crate::collect::energy::EnergyCollector;
use crate::collect::{
    collect_snapshot, collect_storage_health_targets, reconcile_snapshot, storage_health_target,
    CollectOptions, CollectionProfile,
};
use crate::model::{
    CollectorId, Device, ReadingFreshness, Sensor, SensorKind, Snapshot, StorageHealthAvailability,
    STORAGE_HEALTH_PROPERTY_KEYS,
};
use crate::telemetry::TelemetryDeriver;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, SyncSender, TryRecvError, TrySendError};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

pub struct MonitorCollector {
    full_options: CollectOptions,
    fast_options: CollectOptions,
    base: Snapshot,
    rediscover: Duration,
    health_interval: Duration,
    storage_health_enabled: bool,
    last_discovery: Instant,
    last_health: Option<Instant>,
    telemetry: TelemetryDeriver,
    energy: EnergyCollector,
    persistent_warnings: Vec<String>,
}

impl MonitorCollector {
    pub fn new(
        mut full_options: CollectOptions,
        rediscover: Duration,
        health_interval: Duration,
    ) -> Self {
        let storage_health_requested = full_options.include_storage_health;
        let storage_health_enabled = storage_health_requested && full_options.allow_helper_commands;
        full_options.include_storage_health = false;
        let mut energy = EnergyCollector::new();
        let mut raw_base = collect_snapshot(&full_options);
        let mut persistent_warnings = Vec::new();
        if storage_health_requested && !storage_health_enabled {
            let warning = "Storage health requested, but helper commands were disabled.".to_owned();
            raw_base.warnings.push(warning.clone());
            persistent_warnings.push(warning);
        }
        energy.sample(&mut raw_base);
        let mut telemetry = TelemetryDeriver::default();
        let mut base = telemetry.apply(raw_base);
        let observed = sensor_ids(&base);
        stamp_observed_readings(&mut base, &observed);
        let fast_options = CollectOptions {
            profile: CollectionProfile::Fast,
            allow_helper_commands: full_options.allow_helper_commands,
            include_sensitive: full_options.include_sensitive,
            include_storage_health: false,
        };
        Self {
            full_options,
            fast_options,
            base,
            rediscover,
            health_interval,
            storage_health_enabled,
            last_discovery: Instant::now(),
            last_health: None,
            telemetry,
            energy,
            persistent_warnings,
        }
    }

    pub fn initial_snapshot(&self) -> Snapshot {
        self.base.clone()
    }

    pub fn snapshot(&mut self, force_rediscovery: bool) -> Snapshot {
        let snapshot = self.snapshot_telemetry(force_rediscovery);
        if !self.storage_health_due(force_rediscovery) {
            return snapshot;
        }
        let targets = self.all_storage_health_targets();
        if targets.is_empty() {
            return snapshot;
        }
        let health =
            collect_storage_health_targets(&targets, self.full_options.include_sensitive, false);
        self.apply_storage_health(health)
    }

    fn snapshot_telemetry(&mut self, force_rediscovery: bool) -> Snapshot {
        let previous = self.base.clone();
        if force_rediscovery || self.last_discovery.elapsed() >= self.rediscover {
            self.energy.refresh_sources();
            let mut refreshed = collect_snapshot(&self.full_options);
            self.energy.sample(&mut refreshed);
            discard_degraded_libsensors_readings(&previous, &mut refreshed);
            merge_storage_health_cache(&previous, &mut refreshed);
            let mut derived = self.telemetry.apply(refreshed);
            let observed = sensor_ids(&derived);
            reconcile_missing_readings(&previous, &mut derived, &observed, false);
            apply_persistent_warnings(&mut derived, &self.persistent_warnings);
            self.base = derived;
            self.last_discovery = Instant::now();
            return self.base.clone();
        }

        let mut dynamic = collect_snapshot(&self.fast_options);
        self.energy.sample(&mut dynamic);
        discard_degraded_libsensors_readings(&previous, &mut dynamic);
        let mut observed = sensor_ids(&dynamic);

        let mut static_base = previous.clone();
        for device in &mut static_base.devices {
            device.counters.clear();
        }
        static_base.warnings.clear();
        static_base.successful_collectors.clear();
        static_base.failed_collectors.clear();

        let mut merged = static_base.overlay(dynamic);
        reconcile_snapshot(&mut merged);
        let mut derived = self.telemetry.apply(merged);
        observed.extend(derived_telemetry_sensor_ids(&derived));
        reconcile_missing_readings(&previous, &mut derived, &observed, true);
        apply_persistent_warnings(&mut derived, &self.persistent_warnings);
        self.base = derived.clone();
        derived
    }

    pub fn set_include_sensitive(&mut self, include_sensitive: bool) -> Snapshot {
        self.full_options.include_sensitive = include_sensitive;
        self.fast_options.include_sensitive = include_sensitive;
        self.snapshot(true)
    }

    fn set_include_sensitive_telemetry(&mut self, include_sensitive: bool) -> Snapshot {
        self.full_options.include_sensitive = include_sensitive;
        self.fast_options.include_sensitive = include_sensitive;
        let mut snapshot = self.snapshot_telemetry(true);
        if !include_sensitive {
            remove_sensitive_storage_properties(&mut snapshot);
            self.base = snapshot.clone();
        }
        snapshot
    }

    fn storage_health_due(&self, force: bool) -> bool {
        self.storage_health_enabled
            && (force
                || self
                    .last_health
                    .is_none_or(|last_health| last_health.elapsed() >= self.health_interval))
    }

    fn all_storage_health_targets(&self) -> Vec<crate::collect::StorageHealthTarget> {
        self.base
            .devices
            .iter()
            .filter_map(storage_health_target)
            .collect()
    }

    fn storage_health_targets(
        &self,
        device_ids: &[String],
    ) -> Vec<crate::collect::StorageHealthTarget> {
        self.base
            .devices
            .iter()
            .filter(|device| device_ids.iter().any(|id| id == &device.id))
            .filter_map(storage_health_target)
            .collect()
    }

    fn include_sensitive(&self) -> bool {
        self.full_options.include_sensitive
    }

    fn apply_storage_health(&mut self, mut health: Snapshot) -> Snapshot {
        preserve_failed_health_results(&self.base, &mut health);
        let mut base = self.base.clone();
        clear_replaced_health(&mut base, &health);
        let mut merged = base.overlay(health);
        remove_duplicate_intermittent_temperatures(&mut merged);
        self.base = merged.clone();
        self.last_health = Some(Instant::now());
        merged
    }
}

#[derive(Debug)]
pub struct MonitorUpdate {
    pub snapshot: Snapshot,
    pub elapsed: Duration,
    pub forced_rediscovery: bool,
    pub storage_health_device_ids: Vec<String>,
    pub include_sensitive: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MonitorRequestResult {
    Accepted,
    Busy,
    Disconnected,
}

#[derive(Debug)]
pub enum MonitorPoll {
    Update(MonitorUpdate),
    Idle,
    Disconnected,
}

#[derive(Debug)]
enum MonitorCommand {
    Refresh {
        force_rediscovery: bool,
    },
    StorageHealth {
        device_ids: Vec<String>,
        elevated: bool,
    },
    SetSensitive {
        include_sensitive: bool,
    },
}

#[derive(Debug, Clone, Copy)]
enum WorkerWake {
    Work,
    Shutdown,
}

struct StorageHealthJob {
    device_ids: Vec<String>,
    targets: Vec<crate::collect::StorageHealthTarget>,
    include_sensitive: bool,
    elevated: bool,
}

struct StorageHealthResult {
    device_ids: Vec<String>,
    health: Snapshot,
    included_sensitive: bool,
    elapsed: Duration,
}

pub struct MonitorWorker {
    requests: SyncSender<MonitorCommand>,
    wake: Sender<WorkerWake>,
    updates: Receiver<MonitorUpdate>,
    desired_sensitive: Arc<AtomicBool>,
    storage_busy: Arc<AtomicBool>,
    _thread: thread::JoinHandle<()>,
    _storage_thread: thread::JoinHandle<()>,
}

impl MonitorWorker {
    pub fn spawn(mut collector: MonitorCollector) -> io::Result<Self> {
        let desired_sensitive = Arc::new(AtomicBool::new(collector.include_sensitive()));
        let desired_sensitive_worker = Arc::clone(&desired_sensitive);
        let storage_busy = Arc::new(AtomicBool::new(false));
        let storage_busy_worker = Arc::clone(&storage_busy);
        let (request_tx, request_rx) = mpsc::sync_channel::<MonitorCommand>(1);
        let (wake_tx, wake_rx) = mpsc::channel::<WorkerWake>();
        let (update_tx, update_rx) = mpsc::sync_channel::<MonitorUpdate>(1);
        let (storage_tx, storage_rx) = mpsc::channel::<StorageHealthJob>();
        let (storage_result_tx, storage_result_rx) = mpsc::channel::<StorageHealthResult>();
        let storage_wake_tx = wake_tx.clone();

        let storage_thread = thread::Builder::new()
            .name("hwall-storage-health".to_owned())
            .spawn(move || {
                while let Ok(job) = storage_rx.recv() {
                    let started = Instant::now();
                    let health = collect_storage_health_targets(
                        &job.targets,
                        job.include_sensitive,
                        job.elevated,
                    );
                    if storage_result_tx
                        .send(StorageHealthResult {
                            device_ids: job.device_ids,
                            health,
                            included_sensitive: job.include_sensitive,
                            elapsed: started.elapsed(),
                        })
                        .is_err()
                        || storage_wake_tx.send(WorkerWake::Work).is_err()
                    {
                        break;
                    }
                }
            })?;

        let worker_thread = thread::Builder::new()
            .name("hwall-collector".to_owned())
            .spawn(move || {
                'worker: while let Ok(wake) = wake_rx.recv() {
                    if matches!(wake, WorkerWake::Shutdown) {
                        break;
                    }

                    loop {
                        match storage_result_rx.try_recv() {
                            Ok(mut result) => {
                                storage_busy_worker.store(false, Ordering::Release);
                                if result.included_sensitive
                                    && !desired_sensitive_worker.load(Ordering::Acquire)
                                {
                                    remove_sensitive_storage_properties(&mut result.health);
                                }
                                let update = MonitorUpdate {
                                    snapshot: collector.apply_storage_health(result.health),
                                    elapsed: result.elapsed,
                                    forced_rediscovery: false,
                                    storage_health_device_ids: result.device_ids,
                                    include_sensitive: None,
                                };
                                if update_tx.send(update).is_err() {
                                    break 'worker;
                                }
                            }
                            Err(TryRecvError::Empty) => break,
                            Err(TryRecvError::Disconnected) => break,
                        }
                    }

                    loop {
                        let command = match request_rx.try_recv() {
                            Ok(command) => command,
                            Err(TryRecvError::Empty) => break,
                            Err(TryRecvError::Disconnected) => break 'worker,
                        };
                        let started = Instant::now();
                        let update = match command {
                            MonitorCommand::Refresh { force_rediscovery } => {
                                let snapshot = collector.snapshot_telemetry(force_rediscovery);
                                if collector.storage_health_due(force_rediscovery)
                                    && storage_busy_worker
                                        .compare_exchange(
                                            false,
                                            true,
                                            Ordering::AcqRel,
                                            Ordering::Acquire,
                                        )
                                        .is_ok()
                                {
                                    let targets = collector.all_storage_health_targets();
                                    if targets.is_empty() {
                                        storage_busy_worker.store(false, Ordering::Release);
                                    } else {
                                        let device_ids = targets
                                            .iter()
                                            .map(|target| target.id.clone())
                                            .collect();
                                        if storage_tx
                                            .send(StorageHealthJob {
                                                device_ids,
                                                targets,
                                                include_sensitive: collector.include_sensitive(),
                                                elevated: false,
                                            })
                                            .is_err()
                                        {
                                            storage_busy_worker.store(false, Ordering::Release);
                                            break 'worker;
                                        }
                                    }
                                }
                                MonitorUpdate {
                                    snapshot,
                                    elapsed: started.elapsed(),
                                    forced_rediscovery: force_rediscovery,
                                    storage_health_device_ids: Vec::new(),
                                    include_sensitive: None,
                                }
                            }
                            MonitorCommand::StorageHealth {
                                device_ids,
                                elevated,
                            } => {
                                let targets = collector.storage_health_targets(&device_ids);
                                if targets.is_empty() {
                                    storage_busy_worker.store(false, Ordering::Release);
                                    MonitorUpdate {
                                        snapshot: collector.initial_snapshot(),
                                        elapsed: started.elapsed(),
                                        forced_rediscovery: false,
                                        storage_health_device_ids: device_ids,
                                        include_sensitive: None,
                                    }
                                } else {
                                    if storage_tx
                                        .send(StorageHealthJob {
                                            device_ids,
                                            targets,
                                            include_sensitive: collector.include_sensitive(),
                                            elevated,
                                        })
                                        .is_err()
                                    {
                                        storage_busy_worker.store(false, Ordering::Release);
                                        break 'worker;
                                    }
                                    continue;
                                }
                            }
                            MonitorCommand::SetSensitive { include_sensitive } => MonitorUpdate {
                                snapshot: collector
                                    .set_include_sensitive_telemetry(include_sensitive),
                                elapsed: started.elapsed(),
                                forced_rediscovery: true,
                                storage_health_device_ids: Vec::new(),
                                include_sensitive: Some(include_sensitive),
                            },
                        };
                        if update_tx.send(update).is_err() {
                            break 'worker;
                        }
                    }
                }
            })?;

        Ok(Self {
            requests: request_tx,
            wake: wake_tx,
            updates: update_rx,
            desired_sensitive,
            storage_busy,
            _thread: worker_thread,
            _storage_thread: storage_thread,
        })
    }

    pub fn request(&self, force_rediscovery: bool) -> MonitorRequestResult {
        self.try_request(MonitorCommand::Refresh { force_rediscovery })
    }

    pub fn request_sensitive(&self, include_sensitive: bool) -> MonitorRequestResult {
        self.desired_sensitive
            .store(include_sensitive, Ordering::Release);
        self.try_request(MonitorCommand::SetSensitive { include_sensitive })
    }

    pub fn request_storage_health(
        &self,
        device_ids: Vec<String>,
        elevated: bool,
    ) -> MonitorRequestResult {
        if self
            .storage_busy
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return MonitorRequestResult::Busy;
        }
        let result = self.try_request(MonitorCommand::StorageHealth {
            device_ids,
            elevated,
        });
        if result != MonitorRequestResult::Accepted {
            self.storage_busy.store(false, Ordering::Release);
        }
        result
    }

    fn try_request(&self, request: MonitorCommand) -> MonitorRequestResult {
        match self.requests.try_send(request) {
            Ok(()) => match self.wake.send(WorkerWake::Work) {
                Ok(()) => MonitorRequestResult::Accepted,
                Err(_) => MonitorRequestResult::Disconnected,
            },
            Err(TrySendError::Full(_)) => MonitorRequestResult::Busy,
            Err(TrySendError::Disconnected(_)) => MonitorRequestResult::Disconnected,
        }
    }

    pub fn poll(&self) -> MonitorPoll {
        match self.updates.try_recv() {
            Ok(update) => MonitorPoll::Update(update),
            Err(TryRecvError::Empty) => MonitorPoll::Idle,
            Err(TryRecvError::Disconnected) => MonitorPoll::Disconnected,
        }
    }
}

impl Drop for MonitorWorker {
    fn drop(&mut self) {
        let _ = self.wake.send(WorkerWake::Shutdown);
    }
}

fn discard_degraded_libsensors_readings(previous: &Snapshot, current: &mut Snapshot) {
    if !current.collector_failed(CollectorId::LibSensors) {
        return;
    }
    let dependent = previous
        .devices
        .iter()
        .flat_map(|device| {
            device
                .sensors
                .iter()
                .filter(|sensor| depends_on_libsensors(sensor))
                .map(move |sensor| (device.id.clone(), sensor.id.clone()))
        })
        .collect::<BTreeSet<_>>();
    for device in &mut current.devices {
        device
            .sensors
            .retain(|sensor| !dependent.contains(&(device.id.clone(), sensor.id.clone())));
    }
}

fn reconcile_missing_readings(
    previous: &Snapshot,
    current: &mut Snapshot,
    observed: &BTreeSet<(String, String)>,
    keep_unavailable: bool,
) {
    stamp_observed_readings(current, observed);

    let missing_ids = previous
        .devices
        .iter()
        .flat_map(|device| {
            device
                .sensors
                .iter()
                .filter(|sensor| {
                    sensor.collector.is_some()
                        && !sensor.is_intermittent()
                        && !observed.contains(&(device.id.clone(), sensor.id.clone()))
                })
                .map(move |sensor| (device.id.clone(), sensor.id.clone()))
        })
        .collect::<BTreeSet<_>>();
    for device in &mut current.devices {
        device
            .sensors
            .retain(|sensor| !missing_ids.contains(&(device.id.clone(), sensor.id.clone())));
    }

    let mut missing_by_device: BTreeMap<String, Vec<Sensor>> = BTreeMap::new();
    for device in &previous.devices {
        for sensor in &device.sensors {
            if sensor.collector.is_none()
                || sensor.is_intermittent()
                || observed.contains(&(device.id.clone(), sensor.id.clone()))
            {
                continue;
            }
            let freshness = missing_freshness(sensor, current);
            if freshness == ReadingFreshness::Unavailable && !keep_unavailable {
                continue;
            }
            let mut retained = sensor.clone();
            retained.freshness = freshness;
            if retained.last_updated_unix_ms.is_none() {
                retained.last_updated_unix_ms = Some(previous.captured_at_unix_ms);
            }
            missing_by_device
                .entry(device.id.clone())
                .or_default()
                .push(retained);
        }
    }

    for (device_id, sensors) in missing_by_device {
        if let Some(device) = current
            .devices
            .iter_mut()
            .find(|device| device.id == device_id)
        {
            device.sensors.extend(sensors);
            continue;
        }
        let Some(previous_device) = previous
            .devices
            .iter()
            .find(|device| device.id == device_id)
        else {
            continue;
        };
        let mut retained_device = device_shell(previous_device);
        retained_device.sensors = sensors;
        current.devices.push(retained_device);
    }
    current.sort();
}

fn sensor_ids(snapshot: &Snapshot) -> BTreeSet<(String, String)> {
    snapshot
        .devices
        .iter()
        .flat_map(|device| {
            device
                .sensors
                .iter()
                .map(move |sensor| (device.id.clone(), sensor.id.clone()))
        })
        .collect()
}

fn derived_telemetry_sensor_ids(snapshot: &Snapshot) -> BTreeSet<(String, String)> {
    snapshot
        .devices
        .iter()
        .flat_map(|device| {
            device
                .sensors
                .iter()
                .filter(|sensor| sensor.metadata_str("derived_by") == Some("hwall-telemetry"))
                .map(move |sensor| (device.id.clone(), sensor.id.clone()))
        })
        .collect()
}

fn stamp_observed_readings(snapshot: &mut Snapshot, observed: &BTreeSet<(String, String)>) {
    let captured_at = snapshot.captured_at_unix_ms;
    for device in &mut snapshot.devices {
        for sensor in &mut device.sensors {
            if observed.contains(&(device.id.clone(), sensor.id.clone())) && sensor.is_current() {
                sensor.last_updated_unix_ms = Some(captured_at);
            }
        }
    }
}

fn missing_freshness(sensor: &Sensor, current: &Snapshot) -> ReadingFreshness {
    if sensor
        .collector
        .is_some_and(|collector| current.collector_failed(collector))
        || (depends_on_libsensors(sensor) && current.collector_failed(CollectorId::LibSensors))
    {
        return ReadingFreshness::Stale;
    }
    if source_still_present(&sensor.source) {
        return ReadingFreshness::Stale;
    }
    if sensor
        .collector
        .is_some_and(|collector| current.collector_succeeded(collector))
    {
        return ReadingFreshness::Unavailable;
    }
    ReadingFreshness::Stale
}

fn depends_on_libsensors(sensor: &Sensor) -> bool {
    sensor.identification == crate::model::Identification::LibSensorsConfig
        || sensor.metadata_str("computed_by") == Some("lm-sensors")
}

fn source_still_present(source: &str) -> bool {
    source.split(',').any(|candidate| {
        let candidate = candidate.trim();
        let candidate = candidate
            .split_once(" (CPU ")
            .map_or(candidate, |(path, _)| path);
        if !candidate.starts_with("/sys/") {
            return false;
        }
        let Some((prefix, suffix)) = candidate.split_once('*') else {
            return Path::new(candidate).exists();
        };
        wildcard_path_exists(prefix, suffix)
    })
}

fn wildcard_path_exists(prefix: &str, suffix: &str) -> bool {
    let prefix = Path::new(prefix);
    let Some(parent) = prefix.parent() else {
        return false;
    };
    let Some(name_prefix) = prefix.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    let suffix = suffix.trim_start_matches('/');
    let Ok(entries) = fs::read_dir(parent) else {
        return false;
    };
    entries.flatten().any(|entry| {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            return false;
        };
        name.starts_with(name_prefix) && entry.path().join(suffix).exists()
    })
}

fn device_shell(device: &Device) -> Device {
    let mut shell = device.clone();
    shell.sensors.clear();
    shell.counters.clear();
    shell
}

fn apply_persistent_warnings(snapshot: &mut Snapshot, persistent: &[String]) {
    for warning in persistent {
        if !snapshot.warnings.iter().any(|known| known == warning) {
            snapshot.warnings.push(warning.clone());
        }
    }
    snapshot.warnings.sort();
    snapshot.warnings.dedup();
}

fn remove_sensitive_storage_properties(snapshot: &mut Snapshot) {
    for device in &mut snapshot.devices {
        device.properties.remove("serial");
        device.properties.remove("wwid");
    }
}

fn merge_storage_health_cache(previous: &Snapshot, refreshed: &mut Snapshot) {
    for device in &mut refreshed.devices {
        let Some(old) = previous.devices.iter().find(|old| old.id == device.id) else {
            continue;
        };
        match device
            .storage_health
            .as_ref()
            .map(|health| health.availability)
        {
            Some(StorageHealthAvailability::Current) => {}
            Some(_) => {
                preserve_failed_health(device, old);
                copy_storage_health_data(old, device);
            }
            None => {
                device.storage_health = old.storage_health.clone();
                copy_storage_health_data(old, device);
            }
        }
    }
    remove_duplicate_intermittent_temperatures(refreshed);
    refreshed.sort();
}

fn copy_storage_health_data(source: &crate::model::Device, target: &mut crate::model::Device) {
    for key in STORAGE_HEALTH_PROPERTY_KEYS {
        if !target.properties.contains_key(*key) {
            if let Some(value) = source.properties.get(*key) {
                target.properties.insert((*key).to_owned(), value.clone());
            }
        }
    }
    for sensor in &source.sensors {
        if sensor.is_intermittent() && !target.sensors.iter().any(|current| current.id == sensor.id)
        {
            target.sensors.push(sensor.clone());
        }
    }
}

fn clear_replaced_health(previous: &mut Snapshot, refreshed: &Snapshot) {
    for result in &refreshed.devices {
        let Some(health) = result.storage_health.as_ref() else {
            continue;
        };
        if health.availability != StorageHealthAvailability::Current {
            continue;
        }
        let Some(device) = previous
            .devices
            .iter_mut()
            .find(|device| device.id == result.id)
        else {
            continue;
        };
        for key in STORAGE_HEALTH_PROPERTY_KEYS {
            device.properties.remove(*key);
        }
        device.sensors.retain(|sensor| !sensor.is_intermittent());
    }
}

fn preserve_failed_health_results(previous: &Snapshot, refreshed: &mut Snapshot) {
    for device in &mut refreshed.devices {
        let Some(old) = previous.devices.iter().find(|old| old.id == device.id) else {
            continue;
        };
        preserve_failed_health(device, old);
    }
}

fn preserve_failed_health(device: &mut crate::model::Device, old: &crate::model::Device) {
    let Some(current) = device.storage_health.as_mut() else {
        return;
    };
    if current.availability == StorageHealthAvailability::Current {
        return;
    }
    let Some(old_health) = old.storage_health.as_ref() else {
        return;
    };
    if old_health.last_success_unix_ms.is_some() {
        current.status = old_health.status;
        current.last_success_unix_ms = old_health.last_success_unix_ms;
        current.sources = old_health.sources.clone();
    }
}

fn remove_duplicate_intermittent_temperatures(snapshot: &mut Snapshot) {
    for device in &mut snapshot.devices {
        let has_live = device
            .sensors
            .iter()
            .any(|sensor| sensor.kind == SensorKind::Temperature && !sensor.is_intermittent());
        if has_live {
            device.sensors.retain(|sensor| {
                sensor.kind != SensorKind::Temperature || !sensor.is_intermittent()
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{
        Device, DeviceClass, Identification, PropertyValue, Sensor, StorageHealth,
        StorageHealthStatus, Unit,
    };

    fn health(
        availability: StorageHealthAvailability,
        status: StorageHealthStatus,
        attempted: u128,
        succeeded: Option<u128>,
    ) -> StorageHealth {
        StorageHealth {
            status,
            availability,
            last_attempt_unix_ms: Some(attempted),
            last_success_unix_ms: succeeded,
            message: None,
            sources: vec!["test".to_owned()],
        }
    }

    fn storage_device(id: &str) -> Device {
        Device::new(id, DeviceClass::Storage, "Test drive")
    }

    fn intermittent_temperature(id: &str, value: f64) -> Sensor {
        let mut sensor = Sensor::new(
            id,
            "Drive temperature",
            SensorKind::Temperature,
            Unit::Celsius,
            Some(value),
            "test helper",
            Identification::VendorApi,
        );
        sensor
            .metadata
            .insert("intermittent".to_owned(), true.into());
        sensor
    }

    fn dynamic_sensor(id: &str, source: &str, collector: CollectorId) -> Sensor {
        let mut sensor = Sensor::new(
            id,
            id,
            SensorKind::Temperature,
            Unit::Celsius,
            Some(42.0),
            source,
            Identification::KernelLabel,
        );
        sensor.mark_collector(collector);
        sensor.last_updated_unix_ms = Some(1_000);
        sensor
    }

    fn snapshot_with_sensor(sensor: Sensor) -> Snapshot {
        let mut snapshot = Snapshot::new();
        let mut device = Device::new("cpu:0", DeviceClass::Cpu, "CPU");
        device.sensors.push(sensor);
        snapshot.devices.push(device);
        snapshot
    }

    #[test]
    fn successful_omission_is_unavailable_until_full_rediscovery() {
        let previous = snapshot_with_sensor(dynamic_sensor(
            "cpu:0:temp",
            "/sys/does-not-exist/hwall-temp",
            CollectorId::Hwmon,
        ));
        let mut current = Snapshot::new();
        current.mark_collector_succeeded(CollectorId::Hwmon);

        let observed = sensor_ids(&current);
        reconcile_missing_readings(&previous, &mut current, &observed, true);
        let sensor = &current.devices[0].sensors[0];
        assert_eq!(sensor.freshness, ReadingFreshness::Unavailable);
        assert_eq!(sensor.value, Some(42.0));

        let mut rediscovered = Snapshot::new();
        rediscovered.mark_collector_succeeded(CollectorId::Hwmon);
        let observed = sensor_ids(&rediscovered);
        reconcile_missing_readings(&previous, &mut rediscovered, &observed, false);
        assert!(rediscovered.devices.is_empty());
    }

    #[test]
    fn failed_collector_keeps_last_value_as_stale() {
        let previous = snapshot_with_sensor(dynamic_sensor(
            "pci:01:00.0:nvidia:temperature",
            "nvidia-smi",
            CollectorId::NvidiaSmi,
        ));
        let mut current = Snapshot::new();
        current.mark_collector_failed(CollectorId::NvidiaSmi);

        let observed = sensor_ids(&current);
        reconcile_missing_readings(&previous, &mut current, &observed, true);
        let sensor = &current.devices[0].sensors[0];
        assert_eq!(sensor.freshness, ReadingFreshness::Stale);
        assert_eq!(sensor.value, Some(42.0));
        assert_eq!(sensor.last_updated_unix_ms, Some(1_000));
    }

    #[test]
    fn failed_libsensors_enrichment_keeps_enriched_reading_stale() {
        let mut previous_sensor = dynamic_sensor(
            "cpu:0:temp",
            "/sys/does-not-exist/hwall-temp",
            CollectorId::Hwmon,
        );
        previous_sensor.identification = Identification::LibSensorsConfig;
        previous_sensor
            .metadata
            .insert("computed_by".to_owned(), "lm-sensors".into());
        let previous = snapshot_with_sensor(previous_sensor);

        let mut current_sensor = dynamic_sensor(
            "cpu:0:temp",
            "/sys/does-not-exist/hwall-temp",
            CollectorId::Hwmon,
        );
        current_sensor.identification = Identification::Unidentified;
        let mut current = snapshot_with_sensor(current_sensor);
        current.mark_collector_succeeded(CollectorId::Hwmon);
        current.mark_collector_failed(CollectorId::LibSensors);

        discard_degraded_libsensors_readings(&previous, &mut current);
        let observed = sensor_ids(&current);
        reconcile_missing_readings(&previous, &mut current, &observed, true);
        let sensor = &current.devices[0].sensors[0];
        assert_eq!(sensor.freshness, ReadingFreshness::Stale);
        assert_eq!(sensor.identification, Identification::LibSensorsConfig);
    }

    #[test]
    fn fresh_reading_replaces_stale_state() {
        let mut stale_sensor = dynamic_sensor("cpu:0:temp", "nvidia-smi", CollectorId::NvidiaSmi);
        stale_sensor.freshness = ReadingFreshness::Stale;
        let previous = snapshot_with_sensor(stale_sensor);

        let mut fresh = snapshot_with_sensor(dynamic_sensor(
            "cpu:0:temp",
            "nvidia-smi",
            CollectorId::NvidiaSmi,
        ));
        fresh.mark_collector_succeeded(CollectorId::NvidiaSmi);
        let observed = sensor_ids(&fresh);
        reconcile_missing_readings(&previous, &mut fresh, &observed, true);

        assert_eq!(
            fresh.devices[0].sensors[0].freshness,
            ReadingFreshness::Current
        );
    }

    #[test]
    fn successful_partial_refresh_replaces_the_previous_health_set() {
        let mut previous = Snapshot::new();
        let mut old = storage_device("block:nvme0");
        old.storage_health = Some(health(
            StorageHealthAvailability::Current,
            StorageHealthStatus::Passed,
            1_000,
            Some(1_000),
        ));
        old.properties
            .insert("percentage_used".to_owned(), 5_u64.into());
        old.properties
            .insert("power_cycles".to_owned(), 10_u64.into());
        old.sensors
            .push(intermittent_temperature("old-temperature", 35.0));
        previous.devices.push(old);

        let mut refreshed = Snapshot::new();
        let mut current = storage_device("block:nvme0");
        current.storage_health = Some(health(
            StorageHealthAvailability::Current,
            StorageHealthStatus::Passed,
            2_000,
            Some(2_000),
        ));
        current
            .properties
            .insert("power_cycles".to_owned(), 11_u64.into());
        refreshed.devices.push(current);

        clear_replaced_health(&mut previous, &refreshed);
        let merged = previous.overlay(refreshed);
        let device = &merged.devices[0];
        assert!(!device.properties.contains_key("percentage_used"));
        assert_eq!(
            device.properties.get("power_cycles"),
            Some(&PropertyValue::Unsigned(11))
        );
        assert!(device.sensors.is_empty());
    }

    #[test]
    fn failed_refresh_preserves_the_last_successful_health_data() {
        let mut previous = Snapshot::new();
        let mut old = storage_device("block:nvme0");
        old.storage_health = Some(health(
            StorageHealthAvailability::Current,
            StorageHealthStatus::Passed,
            1_000,
            Some(1_000),
        ));
        old.properties
            .insert("percentage_used".to_owned(), 5_u64.into());
        old.sensors
            .push(intermittent_temperature("old-temperature", 35.0));
        previous.devices.push(old);

        let mut failed = Snapshot::new();
        let mut current = storage_device("block:nvme0");
        current.storage_health = Some(StorageHealth {
            message: Some("Permission denied".to_owned()),
            ..health(
                StorageHealthAvailability::PermissionDenied,
                StorageHealthStatus::Unknown,
                2_000,
                None,
            )
        });
        failed.devices.push(current);

        preserve_failed_health_results(&previous, &mut failed);
        let merged = previous.overlay(failed);
        let device = &merged.devices[0];
        let health = device.storage_health.as_ref().unwrap();
        assert_eq!(
            health.availability,
            StorageHealthAvailability::PermissionDenied
        );
        assert_eq!(health.status, StorageHealthStatus::Passed);
        assert_eq!(health.last_success_unix_ms, Some(1_000));
        assert!(device.properties.contains_key("percentage_used"));
        assert_eq!(device.sensors.len(), 1);
    }

    #[test]
    fn live_temperature_suppresses_cached_helper_temperature() {
        let mut snapshot = Snapshot::new();
        let mut device = storage_device("block:sda");
        device
            .sensors
            .push(intermittent_temperature("smart-temperature", 34.0));
        device.sensors.push(Sensor::new(
            "hwmon-temperature",
            "Composite",
            SensorKind::Temperature,
            Unit::Celsius,
            Some(35.0),
            "/sys/class/hwmon/hwmon0/temp1_input",
            Identification::KernelLabel,
        ));
        snapshot.devices.push(device);

        remove_duplicate_intermittent_temperatures(&mut snapshot);

        assert_eq!(snapshot.devices[0].sensors.len(), 1);
        assert_eq!(snapshot.devices[0].sensors[0].id, "hwmon-temperature");
    }
}
