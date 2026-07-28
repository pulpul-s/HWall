//! Shared value formatting and exhaustive diagnostic rendering.

mod diagnostic;
mod format;

pub use format::{
    escape_delimited, format_property_value, format_sample_age, format_sample_age_compact,
    format_value, hardware_property_label, humanize_key, is_low_level_hardware_property,
    property_to_string, sensor_kind_name, storage_health_property_label,
};

use crate::{Snapshot, SnapshotStatistics};

pub fn diagnostic(snapshot: &Snapshot, statistics: Option<&SnapshotStatistics>) -> String {
    diagnostic::render(snapshot, statistics)
}
