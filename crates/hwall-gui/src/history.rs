use hwall_app::{
    DEFAULT_HISTORY_RETENTION_SECONDS, MAX_HISTORY_RETENTION_SECONDS,
    MIN_HISTORY_RETENTION_SECONDS, MIN_REFRESH_INTERVAL_MS,
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
    expected_interval: Duration,
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
    pub(super) captured_at: Instant,
    pub(super) timestamp_ms: u128,
    pub(super) expected_interval: Duration,
    pub(super) x: f64,
    pub(super) value: Option<f64>,
}

impl ChartPoint {
    fn from_sample(sample: TimedSample, cutoff: Instant, window: Duration) -> Self {
        Self {
            captured_at: sample.captured_at,
            timestamp_ms: sample.timestamp_ms,
            expected_interval: sample.expected_interval,
            x: normalized_x(sample.captured_at, cutoff, window),
            value: sample.value,
        }
    }

    fn history_point(self) -> HistoryPoint {
        HistoryPoint {
            captured_at: self.captured_at,
            timestamp_ms: self.timestamp_ms,
            expected_interval: self.expected_interval,
            value: self.value,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(super) struct HistoryPoint {
    pub(super) captured_at: Instant,
    pub(super) timestamp_ms: u128,
    pub(super) expected_interval: Duration,
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
    revision: u64,
}

impl Default for HistoryStore {
    fn default() -> Self {
        Self {
            series: BTreeMap::new(),
            expected_interval: Duration::from_secs(1),
            global_retention: DEFAULT_HISTORY_RETENTION,
            revision: 0,
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
        self.expected_interval = interval.max(Duration::from_millis(MIN_REFRESH_INTERVAL_MS));
    }

    pub(super) fn revision(&self) -> u64 {
        self.revision
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
        self.bump_revision();
    }

    pub(super) fn observe(&mut self, snapshot: &Snapshot) {
        self.observe_at(snapshot, Instant::now());
    }

    fn observe_at(&mut self, snapshot: &Snapshot, now: Instant) {
        let expected_interval = self.expected_interval;
        for device in &snapshot.devices {
            for sensor in &device.sensors {
                if sensor.is_intermittent() || !sensor.is_current() || !chartable_unit(sensor.unit)
                {
                    continue;
                }
                let key = SensorKey::new(&device.id, &sensor.id);
                self.series
                    .entry(key)
                    .or_default()
                    .samples
                    .push_back(TimedSample {
                        captured_at: now,
                        timestamp_ms: snapshot.captured_at_unix_ms,
                        expected_interval,
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
        self.bump_revision();
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
        self.bump_revision();
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
        self.bump_revision();
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

    fn bump_revision(&mut self) {
        self.revision = self.revision.wrapping_add(1);
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
            .filter(|sample| {
                cutoff.is_none_or(|cutoff| sample.captured_at >= cutoff)
                    && sample.captured_at <= now
            })
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

        let mut output = Vec::new();
        let mut segment = Vec::new();
        let mut previous: Option<TimedSample> = None;
        for sample in series
            .samples
            .iter()
            .filter(|sample| sample.captured_at >= cutoff && sample.captured_at <= now)
        {
            let discontinuity = previous.is_some_and(|previous| {
                let max_connected_gap = previous
                    .expected_interval
                    .max(sample.expected_interval)
                    .saturating_mul(3);
                sample
                    .captured_at
                    .saturating_duration_since(previous.captured_at)
                    > max_connected_gap
            });
            if sample.value.is_none() || discontinuity {
                append_segment(&mut output, &segment, cutoff, window, max_points);
                segment.clear();
                if output.last().is_some_and(|point| point.value.is_some()) {
                    output.push(ChartPoint::from_sample(
                        TimedSample {
                            value: None,
                            ..*sample
                        },
                        cutoff,
                        window,
                    ));
                }
            }
            if let Some(value) = sample.value {
                segment.push(TimedSample {
                    value: Some(value),
                    ..*sample
                });
            }
            previous = Some(*sample);
        }
        append_segment(&mut output, &segment, cutoff, window, max_points);
        output
    }
}

fn instant_distance(left: Instant, right: Instant) -> Duration {
    if left >= right {
        left.duration_since(right)
    } else {
        right.duration_since(left)
    }
}

fn append_segment(
    output: &mut Vec<ChartPoint>,
    samples: &[TimedSample],
    cutoff: Instant,
    window: Duration,
    max_points: usize,
) {
    if samples.is_empty() {
        return;
    }

    let selected = largest_triangle_three_buckets(samples, max_points.max(8));
    output.extend(
        selected
            .into_iter()
            .map(|sample| ChartPoint::from_sample(sample, cutoff, window)),
    );
}

fn largest_triangle_three_buckets(samples: &[TimedSample], threshold: usize) -> Vec<TimedSample> {
    if samples.len() <= threshold || threshold < 3 {
        return samples.to_vec();
    }

    let every = (samples.len() - 2) as f64 / (threshold - 2) as f64;
    let mut selected = Vec::with_capacity(threshold);
    let mut selected_index = 0;
    selected.push(samples[0]);

    for bucket in 0..threshold - 2 {
        let average_start =
            (((bucket + 1) as f64 * every).floor() as usize + 1).min(samples.len() - 1);
        let average_end = (((bucket + 2) as f64 * every).floor() as usize + 1).min(samples.len());
        let average_range = &samples[average_start..average_end];
        let (average_x, average_y) = if average_range.is_empty() {
            sample_coordinates(samples[samples.len() - 1], samples[0].captured_at)
        } else {
            let (x, y) = average_range.iter().fold((0.0, 0.0), |sum, sample| {
                let (x, y) = sample_coordinates(*sample, samples[0].captured_at);
                (sum.0 + x, sum.1 + y)
            });
            let count = average_range.len() as f64;
            (x / count, y / count)
        };

        let range_start = ((bucket as f64 * every).floor() as usize + 1).min(samples.len() - 1);
        let range_end = (((bucket + 1) as f64 * every).floor() as usize + 1)
            .min(samples.len() - 1)
            .max(range_start + 1);
        let anchor = samples[selected_index];
        let (anchor_x, anchor_y) = sample_coordinates(anchor, samples[0].captured_at);

        let mut next_index = range_start;
        let mut largest_area = f64::NEG_INFINITY;
        for (offset, sample) in samples[range_start..range_end].iter().enumerate() {
            let (x, y) = sample_coordinates(*sample, samples[0].captured_at);
            let area = ((anchor_x - average_x) * (y - anchor_y)
                - (anchor_x - x) * (average_y - anchor_y))
                .abs();
            if area > largest_area {
                largest_area = area;
                next_index = range_start + offset;
            }
        }

        selected.push(samples[next_index]);
        selected_index = next_index;
    }

    selected.push(samples[samples.len() - 1]);
    selected
}

fn sample_coordinates(sample: TimedSample, origin: Instant) -> (f64, f64) {
    (
        sample
            .captured_at
            .saturating_duration_since(origin)
            .as_secs_f64(),
        sample.value.unwrap_or_default(),
    )
}

pub(super) fn nearest_chart_point(points: &[ChartPoint], target: Instant) -> Option<HistoryPoint> {
    points
        .iter()
        .filter(|point| point.value.is_some())
        .min_by_key(|point| instant_distance(point.captured_at, target))
        .copied()
        .map(ChartPoint::history_point)
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
    fn stale_readings_are_not_recorded_as_new_samples() {
        let start = Instant::now();
        let mut history = HistoryStore::default();
        history.observe_at(&snapshot(Some(10.0)), start);

        let mut stale = snapshot(Some(99.0));
        stale.devices[0].sensors[0].freshness = hwall_core::ReadingFreshness::Stale;
        history.observe_at(&stale, start + Duration::from_secs(1));

        let key = SensorKey::new("cpu:0", "usage");
        assert_eq!(history.series[&key].samples.len(), 1);
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
    fn subsecond_refreshes_are_all_retained() {
        let start = Instant::now();
        let mut history = HistoryStore::default();
        history.set_expected_interval(Duration::from_millis(200));
        for milliseconds in [0, 200, 400, 600, 800, 1_000] {
            let mut sample_snapshot = snapshot(Some(milliseconds as f64));
            sample_snapshot.captured_at_unix_ms = milliseconds;
            history.observe_at(
                &sample_snapshot,
                start + Duration::from_millis(milliseconds as u64),
            );
        }
        let key = SensorKey::new("cpu:0", "usage");
        assert_eq!(history.series[&key].samples.len(), 6);
        assert_eq!(
            history
                .samples(&key, None, start + Duration::from_secs(1))
                .into_iter()
                .map(|sample| sample.timestamp_ms)
                .collect::<Vec<_>>(),
            vec![0, 200, 400, 600, 800, 1_000]
        );

        let points = history.chart_points(
            &key,
            Duration::from_secs(60),
            start + Duration::from_secs(1),
            100,
        );
        assert_eq!(
            points.iter().filter(|point| point.value.is_some()).count(),
            6
        );
    }

    #[test]
    fn chart_connects_samples_across_interval_changes() {
        let start = Instant::now();
        let mut history = HistoryStore::default();
        for milliseconds in [0, 1_000, 2_000] {
            history.observe_at(
                &snapshot(Some(milliseconds as f64)),
                start + Duration::from_millis(milliseconds),
            );
        }

        history.set_expected_interval(Duration::from_millis(200));
        for milliseconds in [2_200, 2_400] {
            history.observe_at(
                &snapshot(Some(milliseconds as f64)),
                start + Duration::from_millis(milliseconds),
            );
        }

        let key = SensorKey::new("cpu:0", "usage");
        let points = history.chart_points(
            &key,
            Duration::from_secs(60),
            start + Duration::from_millis(2_400),
            100,
        );
        assert_eq!(
            points.iter().filter(|point| point.value.is_some()).count(),
            5
        );
        assert!(!points.iter().any(|point| point.value.is_none()));
    }

    #[test]
    fn chart_still_breaks_genuine_gaps_after_interval_changes() {
        let start = Instant::now();
        let mut history = HistoryStore::default();
        history.observe_at(&snapshot(Some(1.0)), start);
        history.observe_at(&snapshot(Some(2.0)), start + Duration::from_secs(1));

        history.set_expected_interval(Duration::from_millis(200));
        history.observe_at(&snapshot(Some(3.0)), start + Duration::from_millis(1_200));
        history.observe_at(&snapshot(Some(4.0)), start + Duration::from_millis(2_200));

        let key = SensorKey::new("cpu:0", "usage");
        let points = history.chart_points(
            &key,
            Duration::from_secs(60),
            start + Duration::from_millis(2_200),
            100,
        );
        assert_eq!(
            points.iter().filter(|point| point.value.is_none()).count(),
            1
        );
    }

    #[test]
    fn hover_selects_the_closest_rendered_sample() {
        let start = Instant::now();
        let mut history = HistoryStore::default();
        for seconds in [0, 10, 20] {
            history.observe_at(
                &snapshot(Some(seconds as f64)),
                start + Duration::from_secs(seconds),
            );
        }
        let key = SensorKey::new("cpu:0", "usage");
        let points = history.chart_points(
            &key,
            Duration::from_secs(60),
            start + Duration::from_secs(20),
            100,
        );
        let point = nearest_chart_point(&points, start + Duration::from_secs(14))
            .expect("nearest rendered point");

        assert_eq!(point.captured_at, start + Duration::from_secs(10));
        assert_eq!(point.expected_interval, Duration::from_secs(1));
        assert_eq!(point.value, Some(10.0));
    }

    #[test]
    fn chart_uses_only_samples_inside_the_visible_window() {
        let start = Instant::now();
        let mut history = HistoryStore::default();
        history.set_global_retention(MAX_HISTORY_RETENTION);
        for seconds in 0..=7_200 {
            history.observe_at(
                &snapshot(Some(seconds as f64)),
                start + Duration::from_secs(seconds),
            );
        }
        let key = SensorKey::new("cpu:0", "usage");
        let window_end = start + Duration::from_secs(3_600);
        let points = history.chart_points(&key, Duration::from_secs(5 * 60), window_end, 320);

        assert!(points.iter().all(|point| {
            point.captured_at >= window_end - Duration::from_secs(5 * 60)
                && point.captured_at <= window_end
        }));
        assert!(points.iter().all(|point| (0.0..=1.0).contains(&point.x)));
    }

    #[test]
    fn long_history_is_decimated_to_actual_recorded_samples() {
        let start = Instant::now();
        let mut samples = VecDeque::new();
        for second in 0..=43_200 {
            samples.push_back(TimedSample {
                captured_at: start + Duration::from_secs(second),
                timestamp_ms: u128::from(second) * 1_000,
                expected_interval: Duration::from_secs(1),
                value: Some(if second == 21_600 { 100.0 } else { 10.0 }),
                status: RecordedStatus::Sensor(SensorStatus::Ok),
            });
        }
        let key = SensorKey::new("cpu:0", "usage");
        let mut history = HistoryStore::default();
        history.series.insert(
            key.clone(),
            SeriesHistory {
                samples,
                recording: Some(ExtendedRecording {
                    retention: MAX_HISTORY_RETENTION,
                    persistent: false,
                }),
            },
        );

        let points = history.chart_points(
            &key,
            Duration::from_secs(12 * 60 * 60),
            start + Duration::from_secs(43_200),
            600,
        );
        let numeric = points
            .iter()
            .filter(|point| point.value.is_some())
            .collect::<Vec<_>>();

        assert_eq!(numeric.len(), 600);
        assert!(numeric.iter().any(|point| point.value == Some(100.0)));
        assert!(numeric.iter().all(|point| {
            let second = point.captured_at.duration_since(start).as_secs();
            point.timestamp_ms == u128::from(second) * 1_000
        }));
    }

    #[test]
    fn hover_point_is_always_one_of_the_rendered_points() {
        let start = Instant::now();
        let mut history = HistoryStore::default();
        history.set_global_retention(Duration::from_secs(15 * 60));
        for second in 0..=600 {
            history.observe_at(
                &snapshot(Some((second % 17) as f64)),
                start + Duration::from_secs(second),
            );
        }
        let key = SensorKey::new("cpu:0", "usage");
        let points = history.chart_points(
            &key,
            Duration::from_secs(10 * 60),
            start + Duration::from_secs(600),
            64,
        );
        let hovered = nearest_chart_point(&points, start + Duration::from_secs(333))
            .expect("rendered hover point");

        assert!(points.iter().any(|point| {
            point.captured_at == hovered.captured_at && point.value == hovered.value
        }));
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
