use serde::Serialize;
use sysinfo::System;

use crate::io_stats::read_diskstats;

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
}

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

pub fn collect_summary(sample_ms: u64) -> Summary {
    use std::collections::{HashMap, HashSet};
    use std::time::{Duration, Instant};

    let mut sys = System::new_all();
    // Initial refresh
    sys.refresh_all();

    // Snapshot diskstats and then sample again after a delay to compute rates
    let d1 = read_diskstats();

    let t1 = Instant::now();
    std::thread::sleep(Duration::from_millis(sample_ms.max(1)));
    sys.refresh_cpu();
    let d2 = read_diskstats();
    let elapsed = t1.elapsed().as_secs_f64();

    let cpu_percent = sys.global_cpu_info().cpu_usage();
    let mem_total = sys.total_memory();
    let mem_used = sys.used_memory();
    let swap_total = sys.total_swap();
    let swap_used = sys.used_swap();
    let uptime_secs = System::uptime();
    let la = System::load_average();

    // Group partitions under base devices and compute deltas
    let base_of = |n: &str| -> String {
        if n.starts_with("nvme") {
            if let Some(pos) = n.rfind('p') {
                return n[..pos].to_string();
            }
        }
        if n.starts_with("mmcblk") {
            if let Some(pos) = n.rfind('p') {
                return n[..pos].to_string();
            }
        }
        if (n.starts_with("sd")
            || n.starts_with("vd")
            || n.starts_with("hd")
            || n.starts_with("md"))
            && n.chars()
                .last()
                .map(|c| c.is_ascii_digit())
                .unwrap_or(false)
        {
            let mut end = n.len();
            for (i, ch) in n.char_indices().rev() {
                if ch.is_ascii_digit() {
                    end = i;
                } else {
                    break;
                }
            }
            return n[..end].to_string();
        }
        n.to_string()
    };

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
    let mut rdelta = 0u64;
    let mut wdelta = 0u64;
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

        rdelta = rdelta.saturating_add(dr_base.max(dr_part));
        wdelta = wdelta.saturating_add(dw_base.max(dw_part));
    }

    let disk_read_bps = rdelta as f64 / elapsed.max(1e-9);
    let disk_write_bps = wdelta as f64 / elapsed.max(1e-9);

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
    }
}
