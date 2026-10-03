use clap::ValueEnum;
use serde::Serialize;
use sysinfo::{ProcessRefreshKind, System, ThreadKind};

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
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub processes: Vec<ProcInfo>,
}

/// One row of the top-processes view. Deliberately limited to PID and name for privacy.
#[derive(Serialize, Debug, Clone)]
pub struct ProcInfo {
    pub pid: u32,
    pub name: String,
    pub cpu_percent: f32,
    pub mem_bytes: u64,
}

/// Sort key for the top-processes view.
#[derive(Copy, Clone, Debug, Eq, PartialEq, ValueEnum)]
pub enum ProcSort {
    Cpu,
    Mem,
}

/// Sort descending by `sort` (ties by ascending pid) and keep the first `n`.
pub fn top_processes(mut procs: Vec<ProcInfo>, n: usize, sort: ProcSort) -> Vec<ProcInfo> {
    procs.sort_by(|a, b| {
        let key = match sort {
            ProcSort::Cpu => b.cpu_percent.total_cmp(&a.cpu_percent),
            ProcSort::Mem => b.mem_bytes.cmp(&a.mem_bytes),
        };
        key.then(a.pid.cmp(&b.pid))
    });
    procs.truncate(n);
    procs
}

/// Process names are attacker-controlled (any user can name a binary); replacing control
/// characters keeps them from forging table rows or emitting terminal escape sequences.
fn printable(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_control() { '?' } else { c })
        .collect()
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

#[doc = "Collect a single summary sample.\n\nBlocks for approximately `sample_ms` (at least `sysinfo::MINIMUM_CPU_UPDATE_INTERVAL`) to compute CPU usage and disk throughput over that window."]
pub fn collect_summary(sample_ms: u64) -> Summary {
    collect_summary_with_top(sample_ms, 0, ProcSort::Cpu)
}

/// Like [`collect_summary`], plus the top `top` processes by `sort` measured over the same
/// window. `top == 0` skips process collection entirely.
pub fn collect_summary_with_top(sample_ms: u64, top: usize, sort: ProcSort) -> Summary {
    use std::time::{Duration, Instant};

    let proc_kind = ProcessRefreshKind::new().with_cpu().with_memory();
    let mut sys = System::new();
    sys.refresh_memory();
    sys.refresh_cpu();
    if top > 0 {
        sys.refresh_processes_specifics(proc_kind);
    }

    // Disk and network snapshots 1
    let d1 = read_diskstats();
    let n1 = read_netstats();

    // Sample window for CPU, disk, and network rates
    let t1 = Instant::now();
    // sysinfo ignores CPU refreshes closer together than this, which would report ~0% CPU.
    std::thread::sleep(
        Duration::from_millis(sample_ms.max(1)).max(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL),
    );
    sys.refresh_cpu();
    if top > 0 {
        sys.refresh_processes_specifics(proc_kind);
    }
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

    // Linux lists each userland thread as its own entry; they share the parent's memory and
    // would crowd out real processes.
    let procs = sys
        .processes()
        .values()
        .filter(|p| p.thread_kind() != Some(ThreadKind::Userland))
        .map(|p| ProcInfo {
            pid: p.pid().as_u32(),
            name: printable(p.name()),
            cpu_percent: p.cpu_usage(),
            mem_bytes: p.memory(),
        })
        .collect();

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
        processes: top_processes(procs, top, sort),
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
    fn top_processes_sorts_desc_breaks_ties_by_pid_and_truncates() {
        let p = |pid, cpu_percent, mem_bytes| ProcInfo {
            pid,
            name: format!("p{pid}"),
            cpu_percent,
            mem_bytes,
        };
        let procs = vec![p(4, 1.0, 50), p(3, 9.0, 10), p(2, 1.0, 50), p(1, 5.0, 99)];
        let pids = |v: Vec<ProcInfo>| v.into_iter().map(|p| p.pid).collect::<Vec<_>>();

        assert_eq!(
            pids(top_processes(procs.clone(), 3, ProcSort::Cpu)),
            [3, 1, 2]
        );
        assert_eq!(
            pids(top_processes(procs.clone(), 10, ProcSort::Mem)),
            [1, 2, 4, 3]
        );
        assert!(top_processes(procs, 0, ProcSort::Cpu).is_empty());
    }

    #[test]
    fn tiny_sample_still_measures_process_cpu() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;
        use std::time::Instant;

        let stop = Arc::new(AtomicBool::new(false));
        let spin = {
            let stop = stop.clone();
            std::thread::spawn(move || while !stop.load(Ordering::Relaxed) {})
        };
        let start = Instant::now();
        let s = collect_summary_with_top(1, usize::MAX, ProcSort::Cpu);
        let took = start.elapsed();
        stop.store(true, Ordering::Relaxed);
        spin.join().unwrap();

        assert!(took >= sysinfo::MINIMUM_CPU_UPDATE_INTERVAL, "{took:?}");
        let me = s
            .processes
            .iter()
            .find(|p| p.pid == std::process::id())
            .expect("own process listed");
        assert!(me.cpu_percent > 1.0, "{}", me.cpu_percent);
    }

    #[test]
    fn printable_replaces_control_chars() {
        assert_eq!(printable("zz\n  1 FAKE"), "zz?  1 FAKE");
        assert_eq!(printable("\u{1b}[2J\u{9b}x"), "?[2J?x");
        assert_eq!(printable("café"), "café");
    }

    #[test]
    fn fmt_bps_scales() {
        assert_eq!(fmt_bps(0.0), "0.0B/s");
        assert_eq!(fmt_bps(1024.0), "1.0KB/s");
        assert_eq!(fmt_bps(1024.0 * 1024.0), "1.0MB/s");
    }
}
