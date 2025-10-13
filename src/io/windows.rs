use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

/// Windows disk I/O monitoring using WMI Win32_PerfRawData_PerfDisk_LogicalDisk
/// This provides cumulative read/write byte counters for each logical disk.

fn query_wmi_disk_stats() -> HashMap<String, (u64, u64)> {
    // For now, return empty map as a fallback until we implement WMI
    // This prevents the application from crashing while we work on the full implementation
    HashMap::new()
}

/// Read Windows disk stats and return per-device totals of (read_bytes, write_bytes).
/// Currently returns empty as WMI implementation is not yet complete.
pub fn read_diskstats() -> HashMap<String, (u64, u64)> {
    query_wmi_disk_stats()
}

/// On Windows there is no partition suffix to strip; return the identifier unchanged.
pub fn base_device_name(name: &str) -> String {
    name.to_string()
}

/// Plain delta calculation between two snapshots.
pub fn base_deltas(
    prev: &HashMap<String, (u64, u64)>,
    next: &HashMap<String, (u64, u64)>,
) -> HashMap<String, (u64, u64)> {
    let mut keys: HashSet<String> = HashSet::new();
    keys.extend(prev.keys().cloned());
    keys.extend(next.keys().cloned());

    let mut out = HashMap::new();
    for key in keys {
        let (pr, pw) = prev.get(&key).copied().unwrap_or((0, 0));
        let (nr, nw) = next.get(&key).copied().unwrap_or((0, 0));

        let dr = nr.saturating_sub(pr);
        let dw = nw.saturating_sub(pw);

        out.insert(key, (dr, dw));
    }
    out
}

/// Sample per-device read/write rates over a small window.
/// Returns a sorted list of `(device, read_bps, write_bps)` by total throughput descending.
pub fn sample_disk_io_detail(sample_ms: u64) -> Vec<(String, f64, f64)> {
    let d1 = read_diskstats();
    let t1 = Instant::now();
    std::thread::sleep(Duration::from_millis(sample_ms.max(1)));
    let d2 = read_diskstats();
    let dt = t1.elapsed().as_secs_f64().max(1e-9);

    let mut devs: Vec<(String, f64, f64)> = base_deltas(&d1, &d2)
        .into_iter()
        .map(|(name, (r, w))| (name, r as f64 / dt, w as f64 / dt))
        .collect();

    devs.sort_by(|a, b| {
        (b.1 + b.2)
            .partial_cmp(&(a.1 + a.2))
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    devs
}
