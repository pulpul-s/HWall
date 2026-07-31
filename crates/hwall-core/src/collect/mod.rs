mod block;
mod cpu;
mod dmi;
pub(crate) mod energy;
mod gpu;
mod hwmon;
mod memory;
mod network;
mod nvme;
mod ownership;
mod pci;
mod perf_event;
mod power;
mod reconcile;
mod storage_health;
mod system;
mod thermal;
mod thunderbolt;
mod usb;
mod util;

use crate::model::{CollectorId, Snapshot, SnapshotBuilder};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CollectionProfile {
    /// Inventory, telemetry, labels, and optional read-only helper tools.
    Full,
    /// Dynamic values suitable for overlaying onto a previous full snapshot.
    Fast,
}

#[derive(Debug, Clone)]
pub struct CollectOptions {
    pub profile: CollectionProfile,
    /// Permit read-only helper commands such as sensors, nvidia-smi, and ethtool.
    pub allow_helper_commands: bool,
    /// Include potentially identifying values such as serial numbers and MAC addresses.
    pub include_sensitive: bool,
    /// Run slower health tools such as smartctl and nvme-cli.
    pub include_storage_health: bool,
}

impl Default for CollectOptions {
    fn default() -> Self {
        Self {
            profile: CollectionProfile::Full,
            allow_helper_commands: true,
            include_sensitive: false,
            include_storage_health: false,
        }
    }
}

pub(crate) use storage_health::StorageHealthTarget;

pub fn supports_storage_health(device: &crate::model::Device) -> bool {
    storage_health::target_from_device(device).is_some()
}

pub(crate) fn storage_health_target(device: &crate::model::Device) -> Option<StorageHealthTarget> {
    storage_health::target_from_device(device)
}

pub(crate) fn collect_storage_health_targets(
    targets: &[StorageHealthTarget],
    include_sensitive: bool,
    elevated: bool,
) -> Snapshot {
    let mut builder = SnapshotBuilder::default();
    storage_health::collect_targets(&mut builder, targets, include_sensitive, elevated);
    let mut snapshot = builder.finish();
    reconcile_snapshot(&mut snapshot);
    snapshot
}

pub(crate) fn reconcile_snapshot(snapshot: &mut Snapshot) {
    reconcile::apply(snapshot);
}

pub fn collect_snapshot(options: &CollectOptions) -> Snapshot {
    let mut builder = SnapshotBuilder::default();

    match options.profile {
        CollectionProfile::Full => collect_full(&mut builder, options),
        CollectionProfile::Fast => collect_fast(&mut builder, options),
    }

    let mut snapshot = builder.finish();
    reconcile_snapshot(&mut snapshot);
    snapshot
}

fn collect_full(builder: &mut SnapshotBuilder, options: &CollectOptions) {
    system::collect(builder);
    dmi::collect(
        builder,
        options.allow_helper_commands,
        options.include_sensitive,
    );
    cpu::collect(builder);
    builder.mark_collector_succeeded(CollectorId::Cpu);
    pci::collect(builder);
    usb::collect(builder, options.include_sensitive);
    block::collect(builder, options.include_sensitive);
    builder.mark_collector_succeeded(CollectorId::Block);
    nvme::collect(builder, options.include_sensitive);
    network::collect(builder, options);
    builder.mark_collector_succeeded(CollectorId::Network);
    power::collect(builder, options.include_sensitive);
    builder.mark_collector_succeeded(CollectorId::Power);
    memory::collect_usage(builder);
    builder.mark_collector_succeeded(CollectorId::Memory);
    memory::collect(builder);
    thermal::collect(builder);
    builder.mark_collector_succeeded(CollectorId::Thermal);
    hwmon::collect(builder, options.allow_helper_commands);
    builder.mark_collector_succeeded(CollectorId::Hwmon);
    gpu::collect(
        builder,
        options.allow_helper_commands,
        options.include_sensitive,
    );
    thunderbolt::collect(builder, options.include_sensitive);

    if options.include_storage_health {
        storage_health::collect(
            builder,
            options.allow_helper_commands,
            options.include_sensitive,
        );
    }
}

fn collect_fast(builder: &mut SnapshotBuilder, options: &CollectOptions) {
    cpu::collect_dynamic(builder);
    builder.mark_collector_succeeded(CollectorId::Cpu);
    memory::collect_usage(builder);
    builder.mark_collector_succeeded(CollectorId::Memory);
    network::collect_dynamic(builder);
    builder.mark_collector_succeeded(CollectorId::Network);
    block::collect_dynamic(builder);
    builder.mark_collector_succeeded(CollectorId::Block);
    power::collect(builder, options.include_sensitive);
    builder.mark_collector_succeeded(CollectorId::Power);
    thermal::collect(builder);
    builder.mark_collector_succeeded(CollectorId::Thermal);
    hwmon::collect(builder, options.allow_helper_commands);
    builder.mark_collector_succeeded(CollectorId::Hwmon);
    gpu::collect(
        builder,
        options.allow_helper_commands,
        options.include_sensitive,
    );
}
