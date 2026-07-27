//! Nonblocking snapshot collection for realtime clients.
//!
//! [`MonitorWorker`] owns the potentially blocking collectors on a dedicated
//! worker thread. Terminal and GUI event loops can request one refresh at a
//! time and consume completed snapshots without blocking input handling.

use crate::collect::{
    collect_snapshot, collect_storage_health_targets, reconcile_snapshot, storage_health_target,
    CollectOptions, CollectionProfile,
};
use crate::model::{SensorKind, Snapshot, StorageHealthAvailability, STORAGE_HEALTH_PROPERTY_KEYS};
use crate::telemetry::TelemetryDeriver;
use std::io;
use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError};
use std::thread;
use std::time::{Duration, Instant};

pub struct MonitorCollector {
    full_options: CollectOptions,
    fast_options: CollectOptions,
    base: Snapshot,
    rediscover: Duration,
    health_interval: Duration,
    last_discovery: Instant,
    last_health: Instant,
    telemetry: TelemetryDeriver,
}

impl MonitorCollector {
    pub fn new(
        full_options: CollectOptions,
        rediscover: Duration,
        health_interval: Duration,
    ) -> Self {
        let raw_base = collect_snapshot(&full_options);
        let mut telemetry = TelemetryDeriver::default();
        let base = telemetry.apply(raw_base);
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
            last_discovery: Instant::now(),
            last_health: Instant::now(),
            telemetry,
        }
    }

    pub fn initial_snapshot(&self) -> Snapshot {
        self.base.clone()
    }

    pub fn snapshot(&mut self, force_rediscovery: bool) -> Snapshot {
        if force_rediscovery || self.last_discovery.elapsed() >= self.rediscover {
            let refresh_health = self.full_options.include_storage_health
                && (force_rediscovery || self.last_health.elapsed() >= self.health_interval);
            let mut rediscovery_options = self.full_options.clone();
            rediscovery_options.include_storage_health = refresh_health;
            let mut refreshed = collect_snapshot(&rediscovery_options);
            merge_storage_health_cache(&self.base, &mut refreshed);
            if refresh_health {
                self.last_health = Instant::now();
            }
            self.base = self.telemetry.apply(refreshed);
            self.last_discovery = Instant::now();
            return self.base.clone();
        }

        let dynamic = collect_snapshot(&self.fast_options);
        let mut static_base = self.base.clone();
        for device in &mut static_base.devices {
            device.counters.clear();
        }
        let mut merged = static_base.overlay(dynamic);
        reconcile_snapshot(&mut merged);
        let derived = self.telemetry.apply(merged);
        self.base = derived.clone();
        derived
    }

    pub fn refresh_storage_health(&mut self, device_ids: &[String], elevated: bool) -> Snapshot {
        let targets = self
            .base
            .devices
            .iter()
            .filter(|device| device_ids.iter().any(|id| id == &device.id))
            .filter_map(storage_health_target)
            .collect::<Vec<_>>();
        if targets.is_empty() {
            return self.base.clone();
        }

        let mut health =
            collect_storage_health_targets(&targets, self.full_options.include_sensitive, elevated);
        preserve_failed_health_results(&self.base, &mut health);
        let mut base = self.base.clone();
        clear_replaced_health(&mut base, &health);
        let mut merged = base.overlay(health);
        remove_duplicate_intermittent_temperatures(&mut merged);
        self.base = merged.clone();
        self.last_health = Instant::now();
        merged
    }
}

#[derive(Debug)]
pub struct MonitorUpdate {
    pub snapshot: Snapshot,
    pub elapsed: Duration,
    pub forced_rediscovery: bool,
    pub storage_health_device_ids: Vec<String>,
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
}

pub struct MonitorWorker {
    requests: SyncSender<MonitorCommand>,
    updates: Receiver<MonitorUpdate>,
    _thread: thread::JoinHandle<()>,
}

impl MonitorWorker {
    pub fn spawn(mut collector: MonitorCollector) -> io::Result<Self> {
        let (request_tx, request_rx) = mpsc::sync_channel::<MonitorCommand>(1);
        let (update_tx, update_rx) = mpsc::sync_channel::<MonitorUpdate>(1);

        let worker_thread = thread::Builder::new()
            .name("hwall-collector".to_owned())
            .spawn(move || {
                while let Ok(command) = request_rx.recv() {
                    let started = Instant::now();
                    let (snapshot, forced_rediscovery, storage_health_device_ids) = match command {
                        MonitorCommand::Refresh { force_rediscovery } => (
                            collector.snapshot(force_rediscovery),
                            force_rediscovery,
                            Vec::new(),
                        ),
                        MonitorCommand::StorageHealth {
                            device_ids,
                            elevated,
                        } => {
                            let snapshot = collector.refresh_storage_health(&device_ids, elevated);
                            (snapshot, false, device_ids)
                        }
                    };
                    let update = MonitorUpdate {
                        snapshot,
                        elapsed: started.elapsed(),
                        forced_rediscovery,
                        storage_health_device_ids,
                    };
                    if update_tx.send(update).is_err() {
                        break;
                    }
                }
            })?;

        Ok(Self {
            requests: request_tx,
            updates: update_rx,
            _thread: worker_thread,
        })
    }

    pub fn request(&self, force_rediscovery: bool) -> MonitorRequestResult {
        self.try_request(MonitorCommand::Refresh { force_rediscovery })
    }

    pub fn request_storage_health(
        &self,
        device_ids: Vec<String>,
        elevated: bool,
    ) -> MonitorRequestResult {
        self.try_request(MonitorCommand::StorageHealth {
            device_ids,
            elevated,
        })
    }

    fn try_request(&self, request: MonitorCommand) -> MonitorRequestResult {
        match self.requests.try_send(request) {
            Ok(()) => MonitorRequestResult::Accepted,
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
