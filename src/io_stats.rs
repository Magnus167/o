use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

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

        // Linux /proc/diskstats: sectors read at idx 5, sectors written at idx 9 (512-byte units)
        let sectors_read = parts[5].parse::<u64>().unwrap_or(0);
        let sectors_written = parts[9].parse::<u64>().unwrap_or(0);
        let read_bytes = sectors_read.saturating_mul(512);
        let write_bytes = sectors_written.saturating_mul(512);
        map.insert(name.to_string(), (read_bytes, write_bytes));
    }
    map
}

fn base_of(name: &str) -> String {
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

pub fn sample_disk_io_detail(sample_ms: u64) -> Vec<(String, f64, f64)> {
    let d1 = read_diskstats();
    let t1 = Instant::now();
    std::thread::sleep(Duration::from_millis(sample_ms));
    let d2 = read_diskstats();
    let dt = t1.elapsed().as_secs_f64();

    let mut base_r2: HashMap<String, u64> = HashMap::new();
    let mut part_r2: HashMap<String, u64> = HashMap::new();
    let mut base_w2: HashMap<String, u64> = HashMap::new();
    let mut part_w2: HashMap<String, u64> = HashMap::new();
    let mut bases: HashSet<String> = HashSet::new();

    for (name, (r2, w2)) in d2.iter() {
        let base = base_of(name);
        let is_partition = base != *name;
        bases.insert(base.clone());
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

    for (name, (r1, w1)) in d1.iter() {
        let base = base_of(name);
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

    let all_bases: HashSet<_> = bases.union(&bases1).cloned().collect();
    let mut devs: Vec<(String, f64, f64)> = Vec::new();
    for base in all_bases {
        let br2 = base_r2.get(&base).copied().unwrap_or(0);
        let br1 = base_r1.get(&base).copied().unwrap_or(0);
        let pr2 = part_r2.get(&base).copied().unwrap_or(0);
        let pr1 = part_r1.get(&base).copied().unwrap_or(0);
        let bw2 = base_w2.get(&base).copied().unwrap_or(0);
        let bw1 = base_w1.get(&base).copied().unwrap_or(0);
        let pw2 = part_w2.get(&base).copied().unwrap_or(0);
        let pw1 = part_w1.get(&base).copied().unwrap_or(0);

        let dr = br2.saturating_sub(br1).max(pr2.saturating_sub(pr1));
        let dw = bw2.saturating_sub(bw1).max(pw2.saturating_sub(pw1));

        devs.push((base, dr as f64 / dt, dw as f64 / dt));
    }

    devs.sort_by(|a, b| {
        (b.1 + b.2)
            .partial_cmp(&(a.1 + a.2))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    devs
}
