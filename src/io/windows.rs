use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::time::{Duration, Instant};

use sysinfo::{DiskExt, Disks, DisksExt};

fn drive_label_from_mount(mount: &OsStr) -> Option<String> {
    let mount = mount.to_str()?.trim_end_matches(['\\', '/']);
    if mount.is_empty() {
        None
    } else {
        Some(mount.to_uppercase())
    }
}

fn normalize_disk_name(disk: &sysinfo::Disk) -> String {
    if let Some(mount_label) = drive_label_from_mount(disk.mount_point().as_os_str()) {
        return mount_label;
    }

    disk.name().to_string_lossy().to_uppercase()
}

/// Read Windows disk stats using `sysinfo::Disks` and return per-device totals of
/// `(read_bytes, write_bytes)`.
pub fn read_diskstats() -> HashMap<String, (u64, u64)> {
    let mut map = HashMap::new();

    let mut disks = Disks::new_with_refreshed_list();
    disks.refresh();

    for disk in disks.iter() {
        // Skip entries that don't expose IO counters.
        let read = disk.total_read_bytes();
        let write = disk.total_written_bytes();

        if read == 0 && write == 0 {
            continue;
        }

        let name = normalize_disk_name(disk);
        if name.is_empty() {
            continue;
        }

        map.insert(name, (read, write));
    }

    map
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
