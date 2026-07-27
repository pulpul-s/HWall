use clap::{Args as ClapArgs, ValueEnum};
use hwall_core::render::{escape_delimited, format_value, sensor_kind_name};
use hwall_core::{
    collect_snapshot, CollectOptions, DeviceClass, MonitorCollector, SensorKind, SensorStatus,
    Snapshot, Unit,
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
    Unavailable,
}

impl From<SensorStatusFilter> for SensorStatus {
    fn from(value: SensorStatusFilter) -> Self {
        match value {
            SensorStatusFilter::Ok => Self::Ok,
            SensorStatusFilter::Alarm => Self::Alarm,
            SensorStatusFilter::Fault => Self::Fault,
            SensorStatusFilter::Unavailable => Self::Unavailable,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Filters<'a> {
    class: Option<DeviceClass>,
    device: Option<&'a str>,
    kind: Option<SensorKind>,
    status: Option<SensorStatus>,
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
        status: args.status.map(Into::into),
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
                    status != sensor.status
                        || (status == SensorStatus::Alarm && limits_unconfigured)
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
            status_name(record).to_owned(),
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

fn write_json(mut out: impl Write, records: &[Record<'_>]) -> io::Result<()> {
    let values: Vec<Value> = records
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
            })
        })
        .collect();
    serde_json::to_writer_pretty(&mut out, &values).map_err(io::Error::other)?;
    writeln!(out)
}

fn status_name(record: &Record<'_>) -> &'static str {
    if record.limits_unconfigured {
        "Limits not configured"
    } else {
        record.status.display_name()
    }
}

fn status_key(record: &Record<'_>) -> &'static str {
    if record.limits_unconfigured {
        "limits_not_configured"
    } else {
        record.status.as_str()
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
    fn delimiter_escaping_quotes_special_fields() {
        assert_eq!(escape_delimited("plain", ','), "plain");
        assert_eq!(escape_delimited("a,b", ','), "\"a,b\"");
        assert_eq!(escape_delimited("a\"b", ','), "\"a\"\"b\"");
    }
}
