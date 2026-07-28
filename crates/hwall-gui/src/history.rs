use hwall_app::{
    DEFAULT_HISTORY_RETENTION_SECONDS, MAX_HISTORY_RETENTION_SECONDS, MIN_HISTORY_RETENTION_SECONDS,
};
use hwall_core::{Sensor, SensorStatus, Snapshot, Unit};
use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};
use std::rc::Rc;
use std::time::{Duration, Instant};

pub(super) const MIN_HISTORY_RETENTION: Duration =
    Duration::from_secs(MIN_HISTORY_RETENTION_SECONDS);
pub(super) const DEFAULT_HISTORY_RETENTION: Duration =
    Duration::from_secs(DEFAULT_HISTORY_RETENTION_SECONDS);
pub(super) const MAX_HISTORY_RETENTION: Duration =
    Duration::from_secs(MAX_HISTORY_RETENTION_SECONDS);
const MIN_SAMPLE_PERIOD: Duration = Duration::from_secs(1);
const MIN_CONNECTED_GAP: Duration = Duration::from_millis(2_500);

pub(super) type SharedHistory = Rc<RefCell<HistoryStore>>;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct SensorKey {
    pub(super) device_id: String,
    pub(super) sensor_id: String,
}

impl SensorKey {
    pub(super) fn new(device_id: impl Into<String>, sensor_id: impl Into<String>) -> Self {
        Self {
            device_id: device_id.into(),
            sensor_id: sensor_id.into(),
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct TimedSample {
    captured_at: Instant,
    timestamp_ms: u128,
    value: Option<f64>,
    status: RecordedStatus,
}

#[derive(Debug, Clone, Copy)]
enum RecordedStatus {
    LimitsNotConfigured,
    Sensor(SensorStatus),
}

impl RecordedStatus {
    fn label(self) -> &'static str {
        match self {
            Self::LimitsNotConfigured => "Limits not configured",
            Self::Sensor(SensorStatus::Ok) => "Normal",
            Self::Sensor(SensorStatus::Alarm) => "Alarm",
            Self::Sensor(SensorStatus::Fault) => "Fault",
            Self::Sensor(SensorStatus::Unavailable) => "Unavailable",
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct NumericSample {
    captured_at: Instant,
    value: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ExtendedRecording {
    pub(super) retention: Duration,
    pub(super) persistent: bool,
}

#[derive(Debug, Default)]
struct SeriesHistory {
    samples: VecDeque<TimedSample>,
    recording: Option<ExtendedRecording>,
}

impl SeriesHistory {
    fn retention(&self, global_retention: Duration) -> Duration {
        self.recording
            .map(|recording| recording.retention.max(global_retention))
            .unwrap_or(global_retention)
    }

    fn push(&mut self, sample: TimedSample) {
        let Some(last) = self.samples.back_mut() else {
            self.samples.push_back(sample);
            return;
        };
        if sample
            .captured_at
            .saturating_duration_since(last.captured_at)
            < MIN_SAMPLE_PERIOD
        {
            last.timestamp_ms = sample.timestamp_ms;
            last.value = sample.value;
            last.status = sample.status;
            return;
        }
        self.samples.push_back(sample);
    }

    fn prune(&mut self, now: Instant, global_retention: Duration) {
        let cutoff = now
            .checked_sub(self.retention(global_retention))
            .unwrap_or(now);
        while self
            .samples
            .front()
            .is_some_and(|sample| sample.captured_at < cutoff)
        {
            self.samples.pop_front();
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(super) struct ChartPoint {
    pub(super) x: f64,
    pub(super) value: Option<f64>,
}

#[derive(Debug, Clone)]
pub(super) struct HistorySample {
    pub(super) timestamp_ms: u128,
    pub(super) value: Option<f64>,
    pub(super) status: &'static str,
}

#[derive(Debug)]
pub(super) struct HistoryStore {
    series: BTreeMap<SensorKey, SeriesHistory>,
    expected_interval: Duration,
    global_retention: Duration,
}

impl Default for HistoryStore {
    fn default() -> Self {
        Self {
            series: BTreeMap::new(),
            expected_interval: Duration::from_secs(1),
            global_retention: DEFAULT_HISTORY_RETENTION,
        }
    }
}

impl HistoryStore {
    pub(super) fn shared(expected_interval: Duration, global_retention: Duration) -> SharedHistory {
        let mut history = Self::default();
        history.set_expected_interval(expected_interval);
        history.set_global_retention(global_retention);
        Rc::new(RefCell::new(history))
    }

    pub(super) fn set_expected_interval(&mut self, interval: Duration) {
        self.expected_interval = interval.max(Duration::from_millis(100));
    }

    pub(super) fn global_retention(&self) -> Duration {
        self.global_retention
    }

    pub(super) fn set_global_retention(&mut self, retention: Duration) {
        self.set_global_retention_at(retention, Instant::now());
    }

    fn set_global_retention_at(&mut self, retention: Duration, now: Instant) {
        self.global_retention = retention.clamp(MIN_HISTORY_RETENTION, MAX_HISTORY_RETENTION);
        let global_retention = self.global_retention;
        for series in self.series.values_mut() {
            series.prune(now, global_retention);
        }
        self.series.retain(|_, series| !series.samples.is_empty());
    }

    pub(super) fn observe(&mut self, snapshot: &Snapshot) {
        self.observe_at(snapshot, Instant::now());
    }

    fn observe_at(&mut self, snapshot: &Snapshot, now: Instant) {
        for device in &snapshot.devices {
            for sensor in &device.sensors {
                if sensor.is_intermittent() || !chartable_unit(sensor.unit) {
                    continue;
                }
                let key = SensorKey::new(&device.id, &sensor.id);
                let series = self.series.entry(key).or_default();
                series.push(TimedSample {
                    captured_at: now,
                    timestamp_ms: snapshot.captured_at_unix_ms,
                    value: sensor.value.filter(|value| value.is_finite()),
                    status: recorded_status(sensor),
                });
            }
        }

        let global_retention = self.global_retention;
        for series in self.series.values_mut() {
            series.prune(now, global_retention);
        }
        self.series.retain(|_, series| !series.samples.is_empty());
    }

    pub(super) fn reset_with(&mut self, snapshot: &Snapshot) {
        self.series.retain(|_, series| {
            series.samples.clear();
            series.recording.is_some()
        });
        self.observe(snapshot);
    }

    pub(super) fn start_extended(
        &mut self,
        key: &SensorKey,
        retention: Duration,
        persistent: bool,
    ) {
        let now = Instant::now();
        let global_retention = self.global_retention;
        let series = self.series.entry(key.clone()).or_default();
        series.recording = Some(ExtendedRecording {
            retention: retention.clamp(MIN_HISTORY_RETENTION, MAX_HISTORY_RETENTION),
            persistent,
        });
        series.prune(now, global_retention);
    }

    pub(super) fn set_persistent(&mut self, key: &SensorKey, persistent: bool) {
        if let Some(recording) = self
            .series
            .get_mut(key)
            .and_then(|series| series.recording.as_mut())
        {
            recording.persistent = persistent;
        }
    }

    pub(super) fn stop_extended(&mut self, key: &SensorKey) {
        let global_retention = self.global_retention;
        if let Some(series) = self.series.get_mut(key) {
            series.recording = None;
            series.prune(Instant::now(), global_retention);
        }
        self.series.retain(|_, series| !series.samples.is_empty());
    }

    pub(super) fn close_details(&mut self, key: &SensorKey) {
        let temporary = self
            .series
            .get(key)
            .and_then(|series| series.recording)
            .is_some_and(|recording| !recording.persistent);
        if temporary {
            self.stop_extended(key);
        }
    }

    pub(super) fn recording(&self, key: &SensorKey) -> Option<ExtendedRecording> {
        self.series.get(key).and_then(|series| series.recording)
    }

    pub(super) fn extended_count(&self) -> usize {
        self.series
            .values()
            .filter(|series| series.recording.is_some())
            .count()
    }

    pub(super) fn available_range(&self, key: &SensorKey) -> Option<(Instant, Duration)> {
        let series = self.series.get(key)?;
        let first = series.samples.front()?;
        let last = series.samples.back()?;
        Some((
            last.captured_at,
            last.captured_at
                .saturating_duration_since(first.captured_at),
        ))
    }

    pub(super) fn available_duration(&self, key: &SensorKey) -> Duration {
        self.available_range(key)
            .map(|(_, duration)| duration)
            .unwrap_or_default()
    }

    pub(super) fn samples(
        &self,
        key: &SensorKey,
        window: Option<Duration>,
        now: Instant,
    ) -> Vec<HistorySample> {
        let Some(series) = self.series.get(key) else {
            return Vec::new();
        };
        let cutoff = window.map(|window| {
            let window = window.clamp(MIN_HISTORY_RETENTION, MAX_HISTORY_RETENTION);
            now.checked_sub(window).unwrap_or(now)
        });
        series
            .samples
            .iter()
            .filter(|sample| cutoff.is_none_or(|cutoff| sample.captured_at >= cutoff))
            .map(|sample| HistorySample {
                timestamp_ms: sample.timestamp_ms,
                value: sample.value,
                status: sample.status.label(),
            })
            .collect()
    }

    pub(super) fn chart_points(
        &self,
        key: &SensorKey,
        window: Duration,
        now: Instant,
        max_points: usize,
    ) -> Vec<ChartPoint> {
        let Some(series) = self.series.get(key) else {
            return Vec::new();
        };
        let window = window.clamp(MIN_HISTORY_RETENTION, MAX_HISTORY_RETENTION);
        let cutoff = now.checked_sub(window).unwrap_or(now);
        let max_connected_gap = self
            .expected_interval
            .saturating_mul(3)
            .max(MIN_CONNECTED_GAP);

        let mut output = Vec::new();
        let mut segment = Vec::new();
        let mut previous_at = None;
        for sample in series
            .samples
            .iter()
            .filter(|sample| sample.captured_at >= cutoff)
        {
            let discontinuity = previous_at.is_some_and(|previous| {
                sample.captured_at.saturating_duration_since(previous) > max_connected_gap
            });
            if sample.value.is_none() || discontinuity {
                append_segment(&mut output, &segment, cutoff, window, max_points);
                segment.clear();
                if output.last().is_some_and(|point| point.value.is_some()) {
                    output.push(ChartPoint {
                        x: normalized_x(sample.captured_at, cutoff, window),
                        value: None,
                    });
                }
            }
            if let Some(value) = sample.value {
                segment.push(NumericSample {
                    captured_at: sample.captured_at,
                    value,
                });
            }
            previous_at = Some(sample.captured_at);
        }
        append_segment(&mut output, &segment, cutoff, window, max_points);
        output
    }
}

fn append_segment(
    output: &mut Vec<ChartPoint>,
    samples: &[NumericSample],
    cutoff: Instant,
    window: Duration,
    max_points: usize,
) {
    if samples.is_empty() {
        return;
    }
    let max_points = max_points.max(8);
    if samples.len() <= max_points {
        output.extend(samples.iter().map(|sample| ChartPoint {
            x: normalized_x(sample.captured_at, cutoff, window),
            value: Some(sample.value),
        }));
        return;
    }

    let bucket_count = (max_points / 4).max(1);
    let bucket_size = samples.len().div_ceil(bucket_count);
    for bucket in samples.chunks(bucket_size) {
        let mut candidates = Vec::with_capacity(4);
        candidates.push(bucket[0]);
        if let Some(minimum) = bucket
            .iter()
            .copied()
            .min_by(|left, right| left.value.total_cmp(&right.value))
        {
            candidates.push(minimum);
        }
        if let Some(maximum) = bucket
            .iter()
            .copied()
            .max_by(|left, right| left.value.total_cmp(&right.value))
        {
            candidates.push(maximum);
        }
        candidates.push(bucket[bucket.len() - 1]);
        candidates.sort_by_key(|sample| sample.captured_at);
        candidates.dedup_by_key(|sample| sample.captured_at);
        output.extend(candidates.into_iter().map(|sample| ChartPoint {
            x: normalized_x(sample.captured_at, cutoff, window),
            value: Some(sample.value),
        }));
    }
}

fn normalized_x(captured_at: Instant, cutoff: Instant, window: Duration) -> f64 {
    (captured_at.saturating_duration_since(cutoff).as_secs_f64() / window.as_secs_f64())
        .clamp(0.0, 1.0)
}

fn recorded_status(sensor: &Sensor) -> RecordedStatus {
    if sensor.has_unconfigured_hardware_alarm() {
        RecordedStatus::LimitsNotConfigured
    } else {
        RecordedStatus::Sensor(sensor.status)
    }
}

pub(super) fn chartable_unit(unit: Unit) -> bool {
    !matches!(unit, Unit::Boolean | Unit::Raw)
}

#[cfg(test)]
mod tests {
    use super::*;
    use hwall_core::{Device, DeviceClass, Identification, SensorKind};

    fn snapshot(value: Option<f64>) -> Snapshot {
        let mut snapshot = Snapshot::new();
        let mut device = Device::new("cpu:0", DeviceClass::Cpu, "CPU");
        device.sensors.push(Sensor::new(
            "usage",
            "Usage",
            SensorKind::Utilization,
            Unit::Percent,
            value,
            "/proc/stat",
            Identification::Inferred,
        ));
        snapshot.devices.push(device);
        snapshot
    }

    #[test]
    fn global_history_is_pruned_to_configured_duration() {
        let start = Instant::now();
        let mut history = HistoryStore::default();
        for seconds in 0..=90 {
            history.observe_at(
                &snapshot(Some(seconds as f64)),
                start + Duration::from_secs(seconds),
            );
        }
        let key = SensorKey::new("cpu:0", "usage");
        assert!(history.available_duration(&key) <= DEFAULT_HISTORY_RETENTION);
    }

    #[test]
    fn temporary_recording_stops_when_details_close() {
        let mut history = HistoryStore::default();
        let key = SensorKey::new("cpu:0", "usage");
        history.start_extended(&key, DEFAULT_HISTORY_RETENTION, false);
        history.close_details(&key);
        assert_eq!(history.extended_count(), 0);
    }

    #[test]
    fn persistent_recording_survives_details_close() {
        let mut history = HistoryStore::default();
        let key = SensorKey::new("cpu:0", "usage");
        history.start_extended(&key, MAX_HISTORY_RETENTION, true);
        history.close_details(&key);
        assert_eq!(history.extended_count(), 1);
    }

    #[test]
    fn expired_removed_sensor_drops_its_recording() {
        let start = Instant::now();
        let mut history = HistoryStore::default();
        let key = SensorKey::new("cpu:0", "usage");
        history.observe_at(&snapshot(Some(1.0)), start);
        history.start_extended(&key, MAX_HISTORY_RETENTION, true);
        history.observe_at(
            &Snapshot::new(),
            start + MAX_HISTORY_RETENTION + Duration::from_secs(1),
        );
        assert_eq!(history.extended_count(), 0);
    }

    #[test]
    fn changing_global_retention_prunes_existing_history() {
        let start = Instant::now();
        let mut history = HistoryStore::default();
        history.set_global_retention(Duration::from_secs(5 * 60));
        for seconds in 0..=180 {
            history.observe_at(
                &snapshot(Some(seconds as f64)),
                start + Duration::from_secs(seconds),
            );
        }
        let key = SensorKey::new("cpu:0", "usage");
        assert!(history.available_duration(&key) > DEFAULT_HISTORY_RETENTION);

        history
            .set_global_retention_at(DEFAULT_HISTORY_RETENTION, start + Duration::from_secs(180));
        assert!(history.available_duration(&key) <= DEFAULT_HISTORY_RETENTION);
    }

    #[test]
    fn per_sensor_retention_never_shortens_global_history() {
        let mut history = HistoryStore::default();
        history.set_global_retention(Duration::from_secs(60 * 60));
        let key = SensorKey::new("cpu:0", "usage");
        history.start_extended(&key, Duration::from_secs(5 * 60), false);
        assert_eq!(
            history.series[&key].retention(history.global_retention()),
            Duration::from_secs(60 * 60),
        );
    }

    #[test]
    fn faster_refreshes_do_not_create_more_than_one_point_per_second() {
        let start = Instant::now();
        let mut history = HistoryStore::default();
        for milliseconds in [0, 100, 500, 999, 1_000] {
            history.observe_at(
                &snapshot(Some(milliseconds as f64)),
                start + Duration::from_millis(milliseconds),
            );
        }
        let key = SensorKey::new("cpu:0", "usage");
        let points = history.chart_points(
            &key,
            Duration::from_secs(60),
            start + Duration::from_secs(1),
            100,
        );
        assert_eq!(
            points.iter().filter(|point| point.value.is_some()).count(),
            2
        );
    }

    #[test]
    fn downsampling_preserves_local_extrema() {
        let start = Instant::now();
        let mut history = HistoryStore::default();
        let key = SensorKey::new("cpu:0", "usage");
        history.start_extended(&key, Duration::from_secs(120), false);
        for seconds in 0..100 {
            let value = if seconds == 50 { 100.0 } else { 10.0 };
            history.observe_at(&snapshot(Some(value)), start + Duration::from_secs(seconds));
        }
        let points = history.chart_points(
            &key,
            Duration::from_secs(120),
            start + Duration::from_secs(100),
            16,
        );
        assert!(points.iter().any(|point| {
            point
                .value
                .is_some_and(|value| (value - 100.0).abs() < f64::EPSILON)
        }));
    }
}
