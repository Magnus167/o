//! Disk and network I/O helpers: read /proc/diskstats and /proc/net/dev, normalize device names, and compute deltas.
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

/// Read Linux /proc/diskstats and return a map of device name to (read_bytes, write_bytes).
/// Bytes are computed from sectors (512-byte units).
pub fn read_diskstats() -> HashMap<String, (u64, u64)> {
    let mut map = HashMap::new();
    let Ok(content) = std::fs::read_to_string("/proc/diskstats") else {
        return map;
    };
    for line in content.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 14 {
            continue;
        }
        let name = parts[2];

        if name.starts_with("loop") || name.starts_with("zram") || name.starts_with("ram") {
            continue;
        }

        // sectors read at idx 5, sectors written at idx 9
        let sectors_read = parts[5].parse::<u64>().unwrap_or(0);
        let sectors_written = parts[9].parse::<u64>().unwrap_or(0);
        let read_bytes = sectors_read.saturating_mul(512);
        let write_bytes = sectors_written.saturating_mul(512);
        map.insert(name.to_string(), (read_bytes, write_bytes));
    }
    map
}

/// Normalize a partition name to its base device (e.g., nvme0n1p1 -> nvme0n1, sda1 -> sda).
pub fn base_device_name(name: &str) -> String {
    if name.starts_with("nvme") || name.starts_with("mmcblk") {
        if let Some(pos) = name.rfind('p') {
            return name[..pos].to_string();
        }
    }
    if (name.starts_with("sd")
        || name.starts_with("vd")
        || name.starts_with("hd")
        || name.starts_with("md"))
        && name
            .chars()
            .last()
            .map(|c| c.is_ascii_digit())
            .unwrap_or(false)
    {
        let mut end = name.len();
        for (i, ch) in name.char_indices().rev() {
            if ch.is_ascii_digit() {
                end = i;
            } else {
                break;
            }
        }
        return name[..end].to_string();
    }
    name.to_string()
}

/// Compute per-base-device deltas between two snapshots of diskstats (in bytes).
/// For each base device, the delta is the max of:
/// - delta of the base device itself, and
/// - aggregated deltas of all its partitions.
///
/// Returns a map: base_device -> (delta_read_bytes, delta_write_bytes).
pub fn base_deltas(
    prev: &HashMap<String, (u64, u64)>,
    next: &HashMap<String, (u64, u64)>,
) -> HashMap<String, (u64, u64)> {
    let mut base_r2: HashMap<String, u64> = HashMap::new();
    let mut part_r2: HashMap<String, u64> = HashMap::new();
    let mut base_w2: HashMap<String, u64> = HashMap::new();
    let mut part_w2: HashMap<String, u64> = HashMap::new();
    let mut bases2: HashSet<String> = HashSet::new();

    for (name, (r2, w2)) in next.iter() {
        let base = base_device_name(name);
        let is_partition = base != *name;
        bases2.insert(base.clone());
        if is_partition {
            *part_r2.entry(base.clone()).or_default() =
                part_r2.get(&base).copied().unwrap_or(0).saturating_add(*r2);
            *part_w2.entry(base.clone()).or_default() =
                part_w2.get(&base).copied().unwrap_or(0).saturating_add(*w2);
        } else {
            base_r2.insert(base.clone(), *r2);
            base_w2.insert(base, *w2);
        }
    }

    let mut base_r1: HashMap<String, u64> = HashMap::new();
    let mut part_r1: HashMap<String, u64> = HashMap::new();
    let mut base_w1: HashMap<String, u64> = HashMap::new();
    let mut part_w1: HashMap<String, u64> = HashMap::new();
    let mut bases1: HashSet<String> = HashSet::new();

    for (name, (r1, w1)) in prev.iter() {
        let base = base_device_name(name);
        let is_partition = base != *name;
        bases1.insert(base.clone());
        if is_partition {
            *part_r1.entry(base.clone()).or_default() =
                part_r1.get(&base).copied().unwrap_or(0).saturating_add(*r1);
            *part_w1.entry(base.clone()).or_default() =
                part_w1.get(&base).copied().unwrap_or(0).saturating_add(*w1);
        } else {
            base_r1.insert(base.clone(), *r1);
            base_w1.insert(base, *w1);
        }
    }

    let mut out: HashMap<String, (u64, u64)> = HashMap::new();
    let all_bases: HashSet<_> = bases2.union(&bases1).cloned().collect();
    for base in all_bases {
        let br2 = base_r2.get(&base).copied().unwrap_or(0);
        let br1 = base_r1.get(&base).copied().unwrap_or(0);
        let pr2 = part_r2.get(&base).copied().unwrap_or(0);
        let pr1 = part_r1.get(&base).copied().unwrap_or(0);
        let bw2 = base_w2.get(&base).copied().unwrap_or(0);
        let bw1 = base_w1.get(&base).copied().unwrap_or(0);
        let pw2 = part_w2.get(&base).copied().unwrap_or(0);
        let pw1 = part_w1.get(&base).copied().unwrap_or(0);

        let dr_base = br2.saturating_sub(br1);
        let dr_part = pr2.saturating_sub(pr1);
        let dw_base = bw2.saturating_sub(bw1);
        let dw_part = pw2.saturating_sub(pw1);

        let read = dr_base.max(dr_part);
        let write = dw_base.max(dw_part);
        out.insert(base, (read, write));
    }
    out
}

/// Sample per-base-device read/write rates over a small window.
/// Returns a sorted list of (base_device, read_bps, write_bps) by total descending.
pub fn sample_disk_io_detail(sample_ms: u64) -> Vec<(String, f64, f64)> {
    let d1 = read_diskstats();
    let t1 = Instant::now();
    std::thread::sleep(Duration::from_millis(sample_ms.max(1)));
    let d2 = read_diskstats();
    let dt = t1.elapsed().as_secs_f64().max(1e-9);

    let deltas = base_deltas(&d1, &d2);
    let mut devs: Vec<(String, f64, f64)> = deltas
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

/// Read Linux /proc/net/dev and return a map of interface name to (rx_bytes, tx_bytes).
pub fn read_netstats() -> HashMap<String, (u64, u64)> {
    let mut map = HashMap::new();
    let Ok(content) = std::fs::read_to_string("/proc/net/dev") else {
        return map;
    };

    for line in content.lines().skip(2) {
        // Skip header lines
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 10 {
            continue;
        }

        let interface = parts[0].trim_end_matches(':');

        // Skip loopback and other virtual interfaces
        if interface == "lo" || interface.starts_with("docker") || interface.starts_with("veth") {
            continue;
        }

        // RX bytes at index 1, TX bytes at index 9
        let rx_bytes = parts[1].parse::<u64>().unwrap_or(0);
        let tx_bytes = parts[9].parse::<u64>().unwrap_or(0);

        map.insert(interface.to_string(), (rx_bytes, tx_bytes));
    }
    map
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
    use std::collections::HashMap;

    #[test]
    fn base_device_name_handles_common_patterns() {
        assert_eq!(base_device_name("nvme0n1p1"), "nvme0n1");
        assert_eq!(base_device_name("mmcblk0p2"), "mmcblk0");
        assert_eq!(base_device_name("sda1"), "sda");
        assert_eq!(base_device_name("vda3"), "vda");
        assert_eq!(base_device_name("md0"), "md");
        assert_eq!(base_device_name("sdb"), "sdb");
        assert_eq!(base_device_name("weird"), "weird");
    }

    #[test]
    fn base_deltas_aggregates_partitions_and_takes_max() {
        // prev snapshot: base=100/100, parts sum to 90/80
        let mut prev = HashMap::new();
        prev.insert("sda".to_string(), (100, 100));
        prev.insert("sda1".to_string(), (40, 30));
        prev.insert("sda2".to_string(), (50, 50));

        // next snapshot: base increased by 30/40, parts sum increased by 20/60
        let mut next = HashMap::new();
        next.insert("sda".to_string(), (130, 140)); // +30/+40
        next.insert("sda1".to_string(), (50, 60)); // +10/+30
        next.insert("sda2".to_string(), (60, 60)); // +10/+10

        let deltas = base_deltas(&prev, &next);
        let (r, w) = deltas.get("sda").copied().unwrap();
        // read: max(30, 20) = 30; write: max(40, 40) = 40
        assert_eq!((r, w), (30, 40));
    }

    #[test]
    fn net_deltas_calculates_correctly() {
        let mut prev = HashMap::new();
        prev.insert("eth0".to_string(), (1000, 500));
        prev.insert("wlan0".to_string(), (2000, 1000));

        let mut next = HashMap::new();
        next.insert("eth0".to_string(), (1200, 700)); // +200 rx, +200 tx
        next.insert("wlan0".to_string(), (2300, 1100)); // +300 rx, +100 tx

        let deltas = net_deltas(&prev, &next);
        assert_eq!(deltas.get("eth0"), Some(&(200, 200)));
        assert_eq!(deltas.get("wlan0"), Some(&(300, 100)));
    }
}
