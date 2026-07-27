//! HWall's reusable, presentation-independent Linux hardware model and collectors.
//!
//! The CLI and GTK GUI consume the same [`Snapshot`] values.

pub mod collect;
pub mod model;
pub mod monitor;
pub mod render;
pub mod statistics;
mod telemetry;

pub use collect::{collect_snapshot, supports_storage_health, CollectOptions, CollectionProfile};
pub use model::*;
pub use monitor::{
    MonitorCollector, MonitorPoll, MonitorRequestResult, MonitorUpdate, MonitorWorker,
};
pub use statistics::{RunningStatistics, SnapshotStatistics};
