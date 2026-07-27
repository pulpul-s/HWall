use super::ownership::resolve_device;
use super::util::{
    canonical, command_exists, list_dirs, normalize_pci_address, read_f64, read_trimmed,
    run_command, symlink_basename,
};
use crate::model::{
    Device, DeviceClass, Identification, Sensor, SensorKind, SnapshotBuilder, Unit,
};
use std::fs;
use std::path::Path;

pub(super) fn collect(
    builder: &mut SnapshotBuilder,
    allow_helper_commands: bool,
    include_sensitive: bool,
) {
    collect_drm(builder);
    if allow_helper_commands {
        collect_nvidia_smi(builder, include_sensitive);
    }
}

fn collect_drm(builder: &mut SnapshotBuilder) {
    for card in list_dirs("/sys/class/drm") {
        let Some(card_name) = card.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if !card_name.starts_with("card") || !card_name[4..].chars().all(|ch| ch.is_ascii_digit()) {
            continue;
        }
        let device_path = canonical(card.join("device")).unwrap_or_else(|| card.join("device"));
        let driver = symlink_basename(device_path.join("driver"));
        let device_id = resolve_device(&device_path, driver.as_deref()).id;
        let mut device = Device::new(
            device_id.clone(),
            DeviceClass::Gpu,
            format!("GPU {card_name}"),
        );
        device.bus_address = Some(card_name.to_owned());
        device.driver = driver.clone();
        device.properties.insert(
            "drm_node".to_owned(),
            format!("/dev/dri/{card_name}").into(),
        );
        if let Some(driver) = &driver {
            device
                .properties
                .insert("drm_driver".to_owned(), driver.clone().into());
        }

        add_percent_sensor(
            &mut device,
            &device_path,
            "gpu_busy_percent",
            "GPU utilization",
            Identification::KernelLabel,
        );
        add_percent_sensor(
            &mut device,
            &device_path,
            "mem_busy_percent",
            "Memory utilization",
            Identification::KernelLabel,
        );
        add_bytes_sensor(
            &mut device,
            &device_path,
            "mem_info_vram_total",
            "VRAM total",
        );
        add_bytes_sensor(&mut device, &device_path, "mem_info_vram_used", "VRAM used");
        add_bytes_sensor(&mut device, &device_path, "mem_info_gtt_total", "GTT total");
        add_bytes_sensor(&mut device, &device_path, "mem_info_gtt_used", "GTT used");
        add_frequency_from_dpm(&mut device, &device_path, "pp_dpm_sclk", "Graphics clock");
        add_frequency_from_dpm(&mut device, &device_path, "pp_dpm_mclk", "Memory clock");
        add_frequency_mhz(
            &mut device,
            &device_path,
            "gt_cur_freq_mhz",
            "Graphics clock",
        );
        add_frequency_mhz(
            &mut device,
            &device_path,
            "rps_cur_freq_mhz",
            "Graphics clock",
        );
        if let Some(level) = read_trimmed(device_path.join("power_dpm_force_performance_level")) {
            device
                .properties
                .insert("performance_level".to_owned(), level.into());
        }
        if let Some(state) = read_trimmed(device_path.join("power_state")) {
            device
                .properties
                .insert("power_state".to_owned(), state.into());
        }
        add_amd_gpu_metrics_header(&mut device, &device_path);
        builder.add_device(device);
    }
}

fn add_amd_gpu_metrics_header(device: &mut Device, root: &Path) {
    let path = root.join("gpu_metrics");
    let Ok(bytes) = fs::read(&path) else {
        return;
    };
    if bytes.len() < 4 {
        return;
    }
    let structure_size = u16::from_le_bytes([bytes[0], bytes[1]]) as u64;
    device.properties.insert(
        "gpu_metrics_structure_size".to_owned(),
        structure_size.into(),
    );
    device.properties.insert(
        "gpu_metrics_format_revision".to_owned(),
        (bytes[2] as u64).into(),
    );
    device.properties.insert(
        "gpu_metrics_content_revision".to_owned(),
        (bytes[3] as u64).into(),
    );
    device.properties.insert(
        "gpu_metrics_bytes_read".to_owned(),
        (bytes.len() as u64).into(),
    );
}

fn add_percent_sensor(
    device: &mut Device,
    root: &Path,
    file: &str,
    label: &str,
    identification: Identification,
) {
    let path = root.join(file);
    let Some(value) = read_f64(&path) else {
        return;
    };
    device.sensors.push(Sensor::new(
        format!("{}:gpu:{file}", device.id),
        label,
        SensorKind::Utilization,
        Unit::Percent,
        Some(value),
        path.to_string_lossy(),
        identification,
    ));
}

fn add_bytes_sensor(device: &mut Device, root: &Path, file: &str, label: &str) {
    let path = root.join(file);
    let Some(value) = read_f64(&path) else {
        return;
    };
    device.sensors.push(Sensor::new(
        format!("{}:gpu:{file}", device.id),
        label,
        SensorKind::Capacity,
        Unit::Byte,
        Some(value),
        path.to_string_lossy(),
        Identification::KernelLabel,
    ));
}

fn add_frequency_mhz(device: &mut Device, root: &Path, file: &str, label: &str) {
    let path = root.join(file);
    let Some(value) = read_f64(&path) else {
        return;
    };
    device.sensors.push(Sensor::new(
        format!("{}:gpu:{file}", device.id),
        label,
        SensorKind::Frequency,
        Unit::Hertz,
        Some(value * 1_000_000.0),
        path.to_string_lossy(),
        Identification::KernelLabel,
    ));
}

fn add_frequency_from_dpm(device: &mut Device, root: &Path, file: &str, label: &str) {
    let path = root.join(file);
    let Some(text) = read_trimmed(&path) else {
        return;
    };
    let active = text
        .lines()
        .find(|line| line.contains('*'))
        .or_else(|| text.lines().last());
    let Some(line) = active else {
        return;
    };
    let frequency = line.split_whitespace().find_map(|part| {
        let lower = part.to_ascii_lowercase();
        if let Some(value) = lower.strip_suffix("mhz") {
            value.parse::<f64>().ok().map(|value| value * 1_000_000.0)
        } else if let Some(value) = lower.strip_suffix("ghz") {
            value
                .parse::<f64>()
                .ok()
                .map(|value| value * 1_000_000_000.0)
        } else {
            None
        }
    });
    let Some(frequency) = frequency else {
        return;
    };
    device.sensors.push(Sensor::new(
        format!("{}:gpu:{file}", device.id),
        label,
        SensorKind::Frequency,
        Unit::Hertz,
        Some(frequency),
        path.to_string_lossy(),
        Identification::KnownDriverMapping,
    ));
}

fn collect_nvidia_smi(builder: &mut SnapshotBuilder, include_sensitive: bool) {
    if !command_exists("nvidia-smi") {
        return;
    }
    let fields = [
        "pci.bus_id",
        "name",
        "uuid",
        "driver_version",
        "temperature.gpu",
        "fan.speed",
        "utilization.gpu",
        "utilization.memory",
        "memory.total",
        "memory.used",
        "power.draw",
        "clocks.current.graphics",
        "clocks.current.memory",
    ]
    .join(",");
    let query = format!("--query-gpu={fields}");
    let Ok(output) = run_command(
        "nvidia-smi",
        [query.as_str(), "--format=csv,noheader,nounits"],
    ) else {
        return;
    };
    if !output.status.success() {
        return;
    }
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let columns = line.split(',').map(str::trim).collect::<Vec<_>>();
        if columns.len() != 13 {
            continue;
        }
        let address = normalize_pci_address(columns[0]);
        let mut device = Device::new(format!("pci:{address}"), DeviceClass::Gpu, columns[1]);
        device.vendor = Some("NVIDIA".to_owned());
        device.model = Some(columns[1].to_owned());
        device.driver = Some("nvidia".to_owned());
        device.bus_address = Some(address);
        if include_sensitive {
            device
                .properties
                .insert("gpu_uuid".to_owned(), columns[2].into());
        }
        device
            .properties
            .insert("driver_version".to_owned(), columns[3].into());
        let measurements = [
            (
                "temperature",
                "GPU temperature",
                4,
                SensorKind::Temperature,
                Unit::Celsius,
                1.0,
            ),
            ("fan_percent", "Fan", 5, SensorKind::Fan, Unit::Percent, 1.0),
            (
                "gpu_utilization",
                "GPU utilization",
                6,
                SensorKind::Utilization,
                Unit::Percent,
                1.0,
            ),
            (
                "memory_utilization",
                "Memory utilization",
                7,
                SensorKind::Utilization,
                Unit::Percent,
                1.0,
            ),
            (
                "memory_total",
                "VRAM total",
                8,
                SensorKind::Capacity,
                Unit::Byte,
                1024.0 * 1024.0,
            ),
            (
                "memory_used",
                "VRAM used",
                9,
                SensorKind::Capacity,
                Unit::Byte,
                1024.0 * 1024.0,
            ),
            (
                "power",
                "Board power",
                10,
                SensorKind::Power,
                Unit::Watt,
                1.0,
            ),
            (
                "graphics_clock",
                "Graphics clock",
                11,
                SensorKind::Frequency,
                Unit::Hertz,
                1_000_000.0,
            ),
            (
                "memory_clock",
                "Memory clock",
                12,
                SensorKind::Frequency,
                Unit::Hertz,
                1_000_000.0,
            ),
        ];
        for (id, label, column, kind, unit, multiplier) in measurements {
            push_parsed(
                &mut device,
                id,
                label,
                columns[column],
                kind,
                unit,
                multiplier,
            );
        }
        builder.add_device(device);
    }
}

fn push_parsed(
    device: &mut Device,
    id: &str,
    label: &str,
    raw: &str,
    kind: SensorKind,
    unit: Unit,
    multiplier: f64,
) {
    let Ok(value) = raw.parse::<f64>() else {
        return;
    };
    device.sensors.push(Sensor::new(
        format!("{}:nvidia:{id}", device.id),
        label,
        kind,
        unit,
        Some(value * multiplier),
        "nvidia-smi",
        Identification::VendorApi,
    ));
}
