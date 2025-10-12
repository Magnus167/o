//! Compatibility shim for legacy imports.
//! This module re-exports disk I/O helpers from the `io` module.
pub use crate::io::{base_deltas, base_device_name, read_diskstats, sample_disk_io_detail};
