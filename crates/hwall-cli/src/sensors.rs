use clap::{Args as ClapArgs, ValueEnum};
use hwall_core::render::{
    escape_delimited, format_reading_age_compact, format_value, sensor_kind_name,
};
use hwall_core::{
    collect_snapshot, CollectOptions, DeviceClass, MonitorCollector, ReadingFreshness, Sensor,
    SensorKind, SensorStatus, Snapshot, Unit,
};
use serde_json::{json, Value};
use std::io::{self, Write};
use std::process::ExitCode;
use std::thread;
use std::time::Duration;

#[derive(Debug, ClapArgs)]
pub(crate) struct Args {
    /// Filter by device class.
    #[arg(long, value_enum)]
    class: Option<DeviceClassFilter>,

    /// Filter by device ID or a case-insensitive part of the device name.
    #[arg(long)]
    device: Option<String>,

    /// Filter by sensor kind.
    #[arg(long, value_enum)]
    kind: Option<SensorKindFilter>,

    /// Filter by sensor status.
    #[arg(long, value_enum)]
    status: Option<SensorStatusFilter>,

    /// Output format.
    #[arg(long, value_enum, default_value = "table")]
    format: OutputFormat,

    /// Sampling interval used to derive CPU, network, and storage rates.
    #[arg(long, default_value = "1s", value_parser = crate::parse_duration)]
    sample: Duration,

    /// Skip the second sample; rate-based sensors will be omitted.
    #[arg(long)]
    instant: bool,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum OutputFormat {
    Table,
    Csv,
    Tsv,
    Json,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum DeviceClassFilter {
    System,
    Motherboard,
    Bios,
    Cpu,
    Memory,
    Gpu,
    Storage,
    Network,
    Usb,
    Pci,
    PowerSupply,
    Battery,
    Thermal,
    SensorController,
    Thunderbolt,
    Other,
}

impl From<DeviceClassFilter> for DeviceClass {
    fn from(value: DeviceClassFilter) -> Self {
        match value {
            DeviceClassFilter::System => Self::System,
            DeviceClassFilter::Motherboard => Self::Motherboard,
            DeviceClassFilter::Bios => Self::Bios,
            DeviceClassFilter::Cpu => Self::Cpu,
            DeviceClassFilter::Memory => Self::Memory,
            DeviceClassFilter::Gpu => Self::Gpu,
            DeviceClassFilter::Storage => Self::Storage,
            DeviceClassFilter::Network => Self::Network,
            DeviceClassFilter::Usb => Self::Usb,
            DeviceClassFilter::Pci => Self::Pci,
            DeviceClassFilter::PowerSupply => Self::PowerSupply,
            DeviceClassFilter::Battery => Self::Battery,
            DeviceClassFilter::Thermal => Self::Thermal,
            DeviceClassFilter::SensorController => Self::SensorController,
            DeviceClassFilter::Thunderbolt => Self::Thunderbolt,
            DeviceClassFilter::Other => Self::Other,
        }
    }
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum SensorKindFilter {
    Temperature,
    Voltage,
    Current,
    Power,
    Energy,
    Fan,
    Frequency,
    EffectiveClock,
    Throughput,
    Utilization,
    Capacity,
    Humidity,
    Counter,
    Boolean,
    Other,
}

impl From<SensorKindFilter> for SensorKind {
    fn from(value: SensorKindFilter) -> Self {
        match value {
            SensorKindFilter::Temperature => Self::Temperature,
            SensorKindFilter::Voltage => Self::Voltage,
            SensorKindFilter::Current => Self::Current,
            SensorKindFilter::Power => Self::Power,
            SensorKindFilter::Energy => Self::Energy,
            SensorKindFilter::Fan => Self::Fan,
            SensorKindFilter::Frequency => Self::Frequency,
            SensorKindFilter::EffectiveClock => Self::EffectiveClock,
            SensorKindFilter::Throughput => Self::Throughput,
            SensorKindFilter::Utilization => Self::Utilization,
            SensorKindFilter::Capacity => Self::Capacity,
            SensorKindFilter::Humidity => Self::Humidity,
            SensorKindFilter::Counter => Self::Counter,
            SensorKindFilter::Boolean => Self::Boolean,
            SensorKindFilter::Other => Self::Other,
        }
    }
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum SensorStatusFilter {
    Ok,
    Alarm,
    Fault,
    Stale,
    Unavailable,
    Offline,
}

impl SensorStatusFilter {
    fn matches(self, sensor: &Sensor) -> bool {
        match self {
            Self::Stale => sensor.freshness == ReadingFreshness::Stale,
            Self::Offline => sensor.freshness == ReadingFreshness::Offline,
            Self::Unavailable => {
                sensor.freshness == ReadingFreshness::Unavailable
                    || (sensor.is_current() && sensor.status == SensorStatus::Unavailable)
            }
            Self::Ok => sensor.is_current() && sensor.status == SensorStatus::Ok,
            Self::Alarm => sensor.is_current() && sensor.status == SensorStatus::Alarm,
            Self::Fault => sensor.is_current() && sensor.status == SensorStatus::Fault,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Filters<'a> {
    class: Option<DeviceClass>,
    device: Option<&'a str>,
    kind: Option<SensorKind>,
    status: Option<SensorStatusFilter>,
}

struct Record<'a> {
    device_id: &'a str,
    device_name: &'a str,
    class: DeviceClass,
    sensor_id: &'a str,
    sensor_label: &'a str,
    kind: SensorKind,
    formatted: String,
    value: Option<f64>,
    raw_value: Option<&'a str>,
    unit: Unit,
    status: SensorStatus,
    freshness: ReadingFreshness,
    last_updated_unix_ms: Option<u128>,
    limits_unconfigured: bool,
}

pub(crate) fn run(args: Args, options: CollectOptions) -> ExitCode {
    let snapshot = if args.instant {
        collect_snapshot(&options)
    } else {
        let mut collector = MonitorCollector::new(
            options,
            Duration::from_secs(30),
            Duration::from_secs(30 * 60),
        );
        thread::sleep(args.sample);
        collector.snapshot(false)
    };
    let filters = Filters {
        class: args.class.map(Into::into),
        device: args.device.as_deref(),
        kind: args.kind.map(Into::into),
        status: args.status,
    };
    let records = records(&snapshot, filters);
    let result = match args.format {
        OutputFormat::Table => write_table(io::stdout().lock(), &records),
        OutputFormat::Csv => write_delimited(io::stdout().lock(), &records, ','),
        OutputFormat::Tsv => write_delimited(io::stdout().lock(), &records, '\t'),
        OutputFormat::Json => write_json(io::stdout().lock(), &records),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("output failed: {error}");
            ExitCode::FAILURE
        }
    }
}

fn records<'a>(snapshot: &'a Snapshot, filters: Filters<'_>) -> Vec<Record<'a>> {
    let device_query = filters
        .device
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_ascii_lowercase);
    let mut records = Vec::new();
    for device in &snapshot.devices {
        if filters.class.is_some_and(|class| class != device.class) {
            continue;
        }
        if let Some(query) = device_query.as_deref() {
            if !device.id.to_ascii_lowercase().contains(query)
                && !device.name.to_ascii_lowercase().contains(query)
            {
                continue;
            }
        }
        for sensor in &device.sensors {
            let limits_unconfigured = sensor.has_unconfigured_hardware_alarm();
            if filters.kind.is_some_and(|kind| kind != sensor.kind)
                || filters.status.is_some_and(|status| {
                    !status.matches(sensor)
                        || (matches!(status, SensorStatusFilter::Alarm) && limits_unconfigured)
                })
            {
                continue;
            }
            let formatted = sensor
                .value
                .map(|value| format_value(value, &sensor.unit))
                .or_else(|| sensor.raw_value.clone())
                .unwrap_or_else(|| "—".to_owned());
            records.push(Record {
                device_id: &device.id,
                device_name: &device.name,
                class: device.class,
                sensor_id: &sensor.id,
                sensor_label: &sensor.label,
                kind: sensor.kind,
                formatted,
                value: sensor.value,
                raw_value: sensor.raw_value.as_deref(),
                unit: sensor.unit,
                status: sensor.status,
                freshness: sensor.freshness,
                last_updated_unix_ms: sensor.last_updated_unix_ms,
                limits_unconfigured,
            });
        }
    }
    records
}

fn write_table(mut out: impl Write, records: &[Record<'_>]) -> io::Result<()> {
    writeln!(
        out,
        "{:<22}  {:<34}  {:<18}  {:>18}  {:<22}",
        "Device", "Sensor", "Kind", "Current", "Status"
    )?;
    writeln!(out, "{}", "─".repeat(122))?;
    for record in records {
        writeln!(
            out,
            "{:<22}  {:<34}  {:<18}  {:>18}  {:<22}",
            truncate(record.device_name, 22),
            truncate(record.sensor_label, 34),
            sensor_kind_name(record.kind),
            truncate(&record.formatted, 18),
            status_name(record)
        )?;
    }
    Ok(())
}

fn write_delimited(mut out: impl Write, records: &[Record<'_>], delimiter: char) -> io::Result<()> {
    let separator = delimiter.to_string();
    writeln!(
        out,
        "{}",
        [
            "device_id",
            "device",
            "class",
            "sensor_id",
            "sensor",
            "kind",
            "value",
            "raw_value",
            "unit",
            "formatted",
            "status",
        ]
        .join(&separator)
    )?;
    for record in records {
        let fields = [
            record.device_id.to_owned(),
            record.device_name.to_owned(),
            record.class.as_str().to_owned(),
            record.sensor_id.to_owned(),
            record.sensor_label.to_owned(),
            sensor_kind_name(record.kind).to_owned(),
            record
                .value
                .map(|value| value.to_string())
                .unwrap_or_default(),
            record.raw_value.unwrap_or_default().to_owned(),
            record.unit.as_str().to_owned(),
            record.formatted.clone(),
            status_name(record),
        ];
        let line = fields
            .iter()
            .map(|value| escape_delimited(value, delimiter))
            .collect::<Vec<_>>()
            .join(&separator);
        writeln!(out, "{line}")?;
    }
    Ok(())
}

pub(crate) fn json_value(snapshot: &Snapshot) -> Value {
    let records = records(
        snapshot,
        Filters {
            class: None,
            device: None,
            kind: None,
            status: None,
        },
    );
    json!({
        "schema_version": snapshot.schema_version,
        "captured_at_unix_ms": snapshot.captured_at_unix_ms,
        "sensors": record_values(&records),
        "warnings": &snapshot.warnings,
    })
}

fn write_json(mut out: impl Write, records: &[Record<'_>]) -> io::Result<()> {
    serde_json::to_writer_pretty(&mut out, &record_values(records)).map_err(io::Error::other)?;
    writeln!(out)
}

fn record_values(records: &[Record<'_>]) -> Vec<Value> {
    records
        .iter()
        .map(|record| {
            json!({
                "device_id": record.device_id,
                "device": record.device_name,
                "class": record.class.as_str(),
                "sensor_id": record.sensor_id,
                "sensor": record.sensor_label,
                "kind": record.kind.as_str(),
                "value": record.value,
                "raw_value": record.raw_value,
                "unit": record.unit.as_str(),
                "formatted": record.formatted.as_str(),
                "status": status_key(record),
                "freshness": record.freshness.as_str(),
                "last_updated_unix_ms": record.last_updated_unix_ms,
            })
        })
        .collect()
}

fn status_name(record: &Record<'_>) -> String {
    match record.freshness {
        ReadingFreshness::Stale => record.last_updated_unix_ms.map_or_else(
            || "Stale".to_owned(),
            |timestamp| {
                format!(
                    "Stale — last updated {}",
                    format_reading_age_compact(timestamp)
                )
            },
        ),
        ReadingFreshness::Unavailable => "Unavailable".to_owned(),
        ReadingFreshness::Offline => "Offline".to_owned(),
        ReadingFreshness::Current if record.limits_unconfigured => {
            "Limits not configured".to_owned()
        }
        ReadingFreshness::Current => record.status.display_name().to_owned(),
    }
}

fn status_key(record: &Record<'_>) -> &'static str {
    match record.freshness {
        ReadingFreshness::Stale => "stale",
        ReadingFreshness::Unavailable => "unavailable",
        ReadingFreshness::Offline => "offline",
        ReadingFreshness::Current if record.limits_unconfigured => "limits_not_configured",
        ReadingFreshness::Current => record.status.as_str(),
    }
}

fn truncate(value: &str, maximum: usize) -> String {
    let mut chars = value.chars();
    let prefix: String = chars.by_ref().take(maximum).collect();
    if chars.next().is_none() {
        prefix
    } else if maximum > 1 {
        prefix.chars().take(maximum - 1).chain(['…']).collect()
    } else {
        "…".to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stale_status_filter_matches_freshness() {
        let mut sensor = Sensor::new(
            "temp:0",
            "Temperature",
            SensorKind::Temperature,
            Unit::Celsius,
            Some(42.0),
            "/test",
            hwall_core::Identification::KernelLabel,
        );
        sensor.freshness = ReadingFreshness::Stale;
        assert!(SensorStatusFilter::Stale.matches(&sensor));
        assert!(!SensorStatusFilter::Ok.matches(&sensor));
    }

    #[test]
    fn offline_status_filter_matches_freshness() {
        let mut sensor = Sensor::new(
            "cpu:0:effective_clock:logical:1",
            "CPU 1 effective clock",
            SensorKind::EffectiveClock,
            Unit::Hertz,
            Some(42_000_000.0),
            "/test",
            hwall_core::Identification::Inferred,
        );
        sensor.freshness = ReadingFreshness::Offline;
        assert!(SensorStatusFilter::Offline.matches(&sensor));
        assert!(!SensorStatusFilter::Unavailable.matches(&sensor));
    }

    #[test]
    fn delimiter_escaping_quotes_special_fields() {
        assert_eq!(escape_delimited("plain", ','), "plain");
        assert_eq!(escape_delimited("a,b", ','), "\"a,b\"");
        assert_eq!(escape_delimited("a\"b", ','), "\"a\"\"b\"");
    }
}
