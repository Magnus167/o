//! Compatibility shim for legacy imports.
//! This module re-exports disk and network I/O helpers from the `io` module.
pub use crate::io::{
    base_deltas, base_device_name, read_diskstats, sample_disk_io_detail,
    net_deltas, read_netstats, sample_net_io_detail,
};
