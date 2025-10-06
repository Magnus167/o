use std::time::Duration;
use sysinfo::System;

#[derive(serde::Serialize)]
struct Summary {
    cpu_percent: f32,
    mem_used: u64,
    mem_total: u64,
    swap_used: u64,
    swap_total: u64,
    uptime_secs: u64,
    load_avg: (f64, f64, f64),
    disk_read_bps: f64,
    disk_write_bps: f64,
}

fn fmt_bytes(b: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KB", "MB", "GB", "TB", "PB"];
    let mut v = b as f64;
    let mut i = 0usize;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    format!("{:.1}{}", v, UNITS[i])
}

fn fmt_bps(bps: f64) -> String {
    const UNITS: [&str; 6] = ["B/s", "KB/s", "MB/s", "GB/s", "TB/s", "PB/s"];
    let mut v = bps;
    let mut i = 0usize;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    format!("{:.1}{}", v, UNITS[i])
}

fn read_diskstats() -> std::collections::HashMap<String, (u64, u64)> {
    use std::collections::HashMap;
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
        // base devices; exclude partitions (simple heuristic)
        let is_base = name.starts_with("sd")
            || name.starts_with("nvme")
            || name.starts_with("vd")
            || name.starts_with("hd")
            || name.starts_with("mmcblk");
        let is_partition = name.chars().last().map(|c| c.is_ascii_digit()).unwrap_or(false)
            || name.contains('p');
        if !is_base || is_partition {
            continue;
        }
        // Linux /proc/diskstats: sectors read at idx 5, sectors written at idx 9
        let sectors_read = parts[5].parse::<u64>().unwrap_or(0);
        let sectors_written = parts[9].parse::<u64>().unwrap_or(0);
        // assume 512-byte sectors as a conservative default
        let read_bytes = sectors_read.saturating_mul(512);
        let write_bytes = sectors_written.saturating_mul(512);
        map.insert(name.to_string(), (read_bytes, write_bytes));
    }
    map
}

fn collect_summary() -> Summary {
    let mut sys = System::new_all();
    // Initial refresh
    sys.refresh_all();
    let d1 = read_diskstats();
    // Refresh CPU twice and sample diskstats with a short delay to get meaningful rates
    std::thread::sleep(Duration::from_millis(500));
    sys.refresh_cpu();
    let d2 = read_diskstats();

    let cpu_percent = sys.global_cpu_info().cpu_usage();
    let mem_total = sys.total_memory();
    let mem_used = sys.used_memory();
    let swap_total = sys.total_swap();
    let swap_used = sys.used_swap();
    let uptime_secs = System::uptime();
    let la = System::load_average();

    // Aggregate disk read/write deltas to get bytes/sec
    let dt = 0.5f64; // seconds (matches sleep above)
    let mut rdelta = 0u64;
    let mut wdelta = 0u64;
    for (name, (r2, w2)) in d2.iter() {
        if let Some((r1, w1)) = d1.get(name) {
            rdelta = rdelta.saturating_add(r2.saturating_sub(*r1));
            wdelta = wdelta.saturating_add(w2.saturating_sub(*w1));
        }
    }
    let disk_read_bps = rdelta as f64 / dt;
    let disk_write_bps = wdelta as f64 / dt;

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

fn main() {
    let json = std::env::args().any(|a| a == "--json" || a == "-j");
    let s = collect_summary();

    if json {
        println!("{}", serde_json::to_string_pretty(&s).unwrap());
    } else {
        use owo_colors::{OwoColorize, Style};

        let style_for = |p: f32| {
            if p >= 80.0 {
                Style::new().red().bold()
            } else if p >= 50.0 {
                Style::new().yellow().bold()
            } else {
                Style::new().green().bold()
            }
        };

        let bar = |percent: f32, width: usize| {
            let width = width.max(1);
            let filled = ((percent.clamp(0.0, 100.0) / 100.0) * width as f32).round() as usize;
            let filled = filled.min(width);
            let mut s = String::new();
            s.push_str(&"█".repeat(filled));
            s.push_str(&"░".repeat(width - filled));
            s
        };

        // bar width computed dynamically below

        println!("{}", "o - minimal system summary".bold());

        // Build rows with unstyled strings to compute consistent column widths
        let cpu_info = format!("{:.1}%", s.cpu_percent);

        let mem_percent = if s.mem_total > 0 {
            (s.mem_used as f64 / s.mem_total as f64 * 100.0) as f32
        } else {
            0.0
        };
        let mem_info = format!("{} / {}", fmt_bytes(s.mem_used), fmt_bytes(s.mem_total));

        let swap_percent = if s.swap_total > 0 {
            (s.swap_used as f64 / s.swap_total as f64 * 100.0) as f32
        } else {
            0.0
        };
        let swap_info = format!("{} / {}", fmt_bytes(s.swap_used), fmt_bytes(s.swap_total));

        let rows: [(&str, f32, String); 3] = [
            ("CPU:", s.cpu_percent, cpu_info),
            ("Mem:", mem_percent, mem_info),
            ("Swap:", swap_percent, swap_info),
        ];

        let label_w = rows.iter().map(|(l, _, _)| l.len()).max().unwrap_or(0).max(4);
        let info_w = rows.iter().map(|(_, _, info)| info.len()).max().unwrap_or(0).max(7);
        let pct_strings: Vec<String> = rows.iter().map(|(_, pct, _)| format!("{:.1}%", pct)).collect();
        let pct_w = pct_strings.iter().map(|s| s.len()).max().unwrap_or(0).max(5);

        // Compute dynamic bar width to fit a fixed total width box
        let term_width: usize = std::env::var("COLUMNS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(80);
        let sep = 1usize; // single space between columns
        let base = label_w + sep + info_w + sep + (pct_w + 2) + sep; // +2 for parentheses
        let bar_w = term_width.saturating_sub(base).max(10);

        // Precompute percentage column strings with parentheses to ensure fixed width
        let pct_cols: Vec<String> = pct_strings
            .iter()
            .map(|s| format!("({:>width$})", s, width = pct_w))
            .collect();

        for (i, (label, pct, info)) in rows.iter().enumerate() {
            let style = style_for(*pct);

            let label_s = format!("{:<width$}", label, width = label_w);
            let label_fmt = label_s.dimmed();

            let info_s = format!("{:>width$}", info, width = info_w);
            let info_fmt = info_s.style(style);

            let bar_s = bar(*pct, bar_w);
            let bar_fmt = bar_s.style(style);

            let pct_s = &pct_cols[i];
            let pct_fmt = pct_s.style(style);

            println!("{} {} {} {}", label_fmt, info_fmt, bar_fmt, pct_fmt);
        }

        // Aligned non-bar rows (labels, info, and empty bar/pct columns to preserve grid)
        let blanks_bar = " ".repeat(bar_w);
        let blanks_pct = " ".repeat(pct_w + 2); // account for parentheses width

        // Disk speed (Read/s / Write/s)
        let disk_label_s = format!("{:<width$}", "Disk:", width = label_w);
        let disk_label = disk_label_s.dimmed();
        let disk_info = format!("{} / {}", fmt_bps(s.disk_read_bps), fmt_bps(s.disk_write_bps));
        let disk_val = format!("{:>width$}", disk_info, width = info_w);
        println!("{} {} {} {}", disk_label, disk_val, blanks_bar, blanks_pct);

        let up_label_s = format!("{:<width$}", "Up:", width = label_w);
        let up_label = up_label_s.dimmed();
        let up_val = format!("{:>width$}", format!("{}s", s.uptime_secs), width = info_w);
        println!("{} {} {} {}", up_label, up_val, blanks_bar, blanks_pct);

        let load_label_s = format!("{:<width$}", "Load:", width = label_w);
        let load_label = load_label_s.dimmed();
        let load_val = format!("{:>width$}", format!("{:.2} {:.2} {:.2}", s.load_avg.0, s.load_avg.1, s.load_avg.2), width = info_w);
        println!("{} {} {} {}", load_label, load_val, blanks_bar, blanks_pct);
    }
}
