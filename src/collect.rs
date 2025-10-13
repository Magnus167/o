use serde::Serialize;
use sysinfo::System;

use crate::io::{base_deltas, net_deltas, read_diskstats, read_netstats};

#[doc = "Snapshot of system summary used by renderers and JSON output."]
#[derive(Serialize, Debug, Clone)]
pub struct Summary {
    pub cpu_percent: f32,
    pub mem_used: u64,
    pub mem_total: u64,
    pub swap_used: u64,
    pub swap_total: u64,
    pub uptime_secs: u64,
    pub load_avg: (f64, f64, f64),
    pub disk_read_bps: f64,
    pub disk_write_bps: f64,
    pub net_rx_bps: f64,
    pub net_tx_bps: f64,
}

#[doc = "Format a byte count using binary units (KB/MB/GB/PB), with one decimal place."]
pub fn fmt_bytes(b: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KB", "MB", "GB", "TB", "PB"];
    let mut v = b as f64;
    let mut i = 0usize;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    format!("{:.1}{}", v, UNITS[i])
}

#[doc = "Format a bytes-per-second rate using binary units per second, with one decimal place."]
pub fn fmt_bps(bps: f64) -> String {
    const UNITS: [&str; 6] = ["B/s", "KB/s", "MB/s", "GB/s", "TB/s", "PB/s"];
    let mut v = bps;
    let mut i = 0usize;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    format!("{:.1}{}", v, UNITS[i])
}

#[doc = "Collect a single summary sample.\n\nBlocks for approximately `sample_ms` to compute CPU usage and disk throughput over that window."]
pub fn collect_summary(sample_ms: u64) -> Summary {
    use std::time::{Duration, Instant};

    let mut sys = System::new_all();
    sys.refresh_all();

    // Disk and network snapshots 1
    let d1 = read_diskstats();
    let n1 = read_netstats();

    // Sample window for CPU, disk, and network rates
    let t1 = Instant::now();
    std::thread::sleep(Duration::from_millis(sample_ms.max(1)));
    sys.refresh_cpu();
    let d2 = read_diskstats();
    let n2 = read_netstats();
    let elapsed = t1.elapsed().as_secs_f64().max(1e-9);

    let cpu_percent = sys.global_cpu_info().cpu_usage();
    let mem_total = sys.total_memory();
    let mem_used = sys.used_memory();
    let swap_total = sys.total_swap();
    let swap_used = sys.used_swap();
    let uptime_secs = System::uptime();
    let la = System::load_average();

    // Compute per-base-device deltas, then totalize
    let deltas = base_deltas(&d1, &d2);
    let mut rdelta: u64 = 0;
    let mut wdelta: u64 = 0;
    for (_base, (dr, dw)) in deltas {
        rdelta = rdelta.saturating_add(dr);
        wdelta = wdelta.saturating_add(dw);
    }

    let disk_read_bps = rdelta as f64 / elapsed;
    let disk_write_bps = wdelta as f64 / elapsed;

    // Compute per-interface network deltas, then totalize
    let net_deltas_map = net_deltas(&n1, &n2);
    let mut rx_delta: u64 = 0;
    let mut tx_delta: u64 = 0;
    for (_interface, (dr, dt)) in net_deltas_map {
        rx_delta = rx_delta.saturating_add(dr);
        tx_delta = tx_delta.saturating_add(dt);
    }

    let net_rx_bps = rx_delta as f64 / elapsed;
    let net_tx_bps = tx_delta as f64 / elapsed;

    Summary {
        cpu_percent,
        mem_used,
        mem_total,
        swap_used,
        swap_total,
        uptime_secs,
        load_avg: (la.one, la.five, la.fifteen),
        disk_read_bps,
        disk_write_bps,
        net_rx_bps,
        net_tx_bps,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fmt_bytes_scales() {
        assert_eq!(fmt_bytes(0), "0.0B");
        assert_eq!(fmt_bytes(1023), "1023.0B");
        assert_eq!(fmt_bytes(1024), "1.0KB");
        assert_eq!(fmt_bytes(1024 * 1024), "1.0MB");
    }

    #[test]
    fn fmt_bps_scales() {
        assert_eq!(fmt_bps(0.0), "0.0B/s");
        assert_eq!(fmt_bps(1024.0), "1.0KB/s");
        assert_eq!(fmt_bps(1024.0 * 1024.0), "1.0MB/s");
    }
}
