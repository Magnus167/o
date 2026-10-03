//! Disk and network I/O helpers for Windows: read cumulative per-physical-drive and per-interface byte counters, and compute deltas.
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use windows::core::HSTRING;
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows::Win32::System::Ioctl::{DISK_PERFORMANCE, IOCTL_DISK_PERFORMANCE};
use windows::Win32::System::IO::DeviceIoControl;

/// Cumulative (read_bytes, write_bytes) of `\\.\PhysicalDrive{index}`, or `None` if it cannot be opened or queried.
fn physical_drive_bytes(index: u32) -> Option<(u64, u64)> {
    let path = HSTRING::from(format!(r"\\.\PhysicalDrive{index}"));
    // IOCTL_DISK_PERFORMANCE is FILE_ANY_ACCESS, so a zero-access handle is enough and needs no admin rights.
    let handle = unsafe {
        CreateFileW(
            &path,
            0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            OPEN_EXISTING,
            FILE_FLAGS_AND_ATTRIBUTES(0),
            HANDLE::default(),
        )
    }
    .ok()?;

    let mut perf = DISK_PERFORMANCE::default();
    let size = std::mem::size_of::<DISK_PERFORMANCE>() as u32;
    let mut returned = 0u32;
    let queried = unsafe {
        DeviceIoControl(
            handle,
            IOCTL_DISK_PERFORMANCE,
            None,
            0,
            Some((&mut perf as *mut DISK_PERFORMANCE).cast()),
            size,
            Some(&mut returned),
            None,
        )
    };
    let _ = unsafe { CloseHandle(handle) };

    if queried.is_err() || returned != size {
        return None;
    }
    Some((
        u64::try_from(perf.BytesRead).ok()?,
        u64::try_from(perf.BytesWritten).ok()?,
    ))
}

/// Read cumulative (read_bytes, write_bytes) per physical drive, keyed `PhysicalDriveN`.
pub fn read_diskstats() -> HashMap<String, (u64, u64)> {
    // Drive numbers can have gaps (e.g. after a hot-unplug), so probe a fixed range instead of
    // stopping at the first missing drive; drives numbered 64 and above are not reported.
    (0..64)
        .filter_map(|n| Some((format!("PhysicalDrive{n}"), physical_drive_bytes(n)?)))
        .collect()
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

/// Read cumulative (rx_bytes, tx_bytes) per connected network adapter, keyed by its alias (e.g. `Ethernet`).
pub fn read_netstats() -> HashMap<String, (u64, u64)> {
    sysinfo::Networks::new_with_refreshed_list()
        .iter()
        .map(|(name, data)| {
            (
                name.clone(),
                (data.total_received(), data.total_transmitted()),
            )
        })
        .collect()
}

/// Compute network interface deltas between two snapshots.
/// Returns a map: interface -> (delta_rx_bytes, delta_tx_bytes).
pub fn net_deltas(
    prev: &HashMap<String, (u64, u64)>,
    next: &HashMap<String, (u64, u64)>,
) -> HashMap<String, (u64, u64)> {
    let mut keys: HashSet<String> = HashSet::new();
    keys.extend(prev.keys().cloned());
    keys.extend(next.keys().cloned());

    let mut out = HashMap::new();
    for key in keys {
        let (pr, pt) = prev.get(&key).copied().unwrap_or((0, 0));
        let (nr, nt) = next.get(&key).copied().unwrap_or((0, 0));

        let dr = nr.saturating_sub(pr);
        let dt = nt.saturating_sub(pt);

        out.insert(key, (dr, dt));
    }
    out
}

/// Sample per-interface rx/tx rates over a small window.
/// Returns a sorted list of (interface, rx_bps, tx_bps) by total throughput descending.
pub fn sample_net_io_detail(sample_ms: u64) -> Vec<(String, f64, f64)> {
    let n1 = read_netstats();
    let t1 = Instant::now();
    std::thread::sleep(Duration::from_millis(sample_ms.max(1)));
    let n2 = read_netstats();
    let dt = t1.elapsed().as_secs_f64().max(1e-9);

    let mut interfaces: Vec<(String, f64, f64)> = net_deltas(&n1, &n2)
        .into_iter()
        .map(|(name, (r, t))| (name, r as f64 / dt, t as f64 / dt))
        .collect();

    interfaces.sort_by(|a, b| {
        (b.1 + b.2)
            .partial_cmp(&(a.1 + a.2))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    interfaces
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_base_device_name_unchanged() {
        assert_eq!(base_device_name("PhysicalDrive0"), "PhysicalDrive0");
    }

    #[test]
    fn test_base_deltas_calculation() {
        let mut prev = HashMap::new();
        prev.insert("C:".to_string(), (1000, 500));
        prev.insert("D:".to_string(), (2000, 1000));

        let mut next = HashMap::new();
        next.insert("C:".to_string(), (1100, 600)); // +100 read, +100 write
        next.insert("D:".to_string(), (2150, 1050)); // +150 read, +50 write

        let deltas = base_deltas(&prev, &next);

        assert_eq!(deltas.get("C:"), Some(&(100, 100)));
        assert_eq!(deltas.get("D:"), Some(&(150, 50)));
    }

    #[test]
    fn test_net_deltas_calculation() {
        let mut prev = HashMap::new();
        prev.insert("Ethernet".to_string(), (1000, 500));
        prev.insert("Wi-Fi".to_string(), (2000, 1000));

        let mut next = HashMap::new();
        next.insert("Ethernet".to_string(), (1200, 700)); // +200 rx, +200 tx
        next.insert("Wi-Fi".to_string(), (2300, 1100)); // +300 rx, +100 tx

        let deltas = net_deltas(&prev, &next);
        assert_eq!(deltas.get("Ethernet"), Some(&(200, 200)));
        assert_eq!(deltas.get("Wi-Fi"), Some(&(300, 100)));
    }

    // Reads the host's live counters; run on a Windows machine with `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn test_live_counters_never_decrease() {
        let (disks, nets) = (read_diskstats(), read_netstats());
        assert!(
            !disks.is_empty(),
            "no physical drive answered IOCTL_DISK_PERFORMANCE (Windows Server needs `diskperf -y`)"
        );
        for (before, after) in [(disks, read_diskstats()), (nets, read_netstats())] {
            for (name, (r1, w1)) in &before {
                if let Some((r2, w2)) = after.get(name) {
                    assert!(r2 >= r1 && w2 >= w1, "{name} counters went backwards");
                }
            }
        }
    }
}
