use std::time::{Duration, Instant};
use sysinfo::System;
mod io_stats;
use crate::io_stats::{read_diskstats, sample_disk_io_detail};

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

fn collect_summary(sample_ms: u64) -> Summary {
    let mut sys = System::new_all();
    // Initial refresh
    sys.refresh_all();
    let d1 = read_diskstats();
    // Refresh CPU twice and sample diskstats with a short delay to get meaningful rates
    let t1 = Instant::now();
    std::thread::sleep(Duration::from_millis(sample_ms));
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

    // Aggregate disk read/write deltas to get bytes/sec, grouping partitions under base devices.
    let dt = elapsed; // seconds measured between samples
    use std::collections::HashMap;
    let mut base_r2: HashMap<String, u64> = HashMap::new();
    let mut part_r2: HashMap<String, u64> = HashMap::new();
    let mut base_w2: HashMap<String, u64> = HashMap::new();
    let mut part_w2: HashMap<String, u64> = HashMap::new();

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

    let mut bases = std::collections::HashSet::new();
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

    let mut bases1 = std::collections::HashSet::new();
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

    let all_bases: std::collections::HashSet<_> = bases.union(&bases1).cloned().collect();
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

fn print_plain(s: &Summary, io_detail: bool, io_detail_sample_ms: u64) {
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

    let label_w = rows
        .iter()
        .map(|(l, _, _)| l.len())
        .max()
        .unwrap_or(0)
        .max(4);
    let info_w = rows
        .iter()
        .map(|(_, _, info)| info.len())
        .max()
        .unwrap_or(0)
        .max(7);
    let pct_strings: Vec<String> = rows
        .iter()
        .map(|(_, pct, _)| format!("{:.1}%", pct))
        .collect();
    let pct_w = pct_strings
        .iter()
        .map(|s| s.len())
        .max()
        .unwrap_or(0)
        .max(5);

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
    let disk_info = format!(
        "{} / {}",
        fmt_bps(s.disk_read_bps),
        fmt_bps(s.disk_write_bps)
    );
    let disk_val = format!("{:>width$}", disk_info, width = info_w);
    println!("{} {} {} {}", disk_label, disk_val, blanks_bar, blanks_pct);

    if io_detail {
        let devs = sample_disk_io_detail(io_detail_sample_ms);
        for (name, rb, wb) in devs {
            let label_s = format!("{:<width$}", "", width = label_w);
            let label_pad = label_s.dimmed();
            let info = format!("{}: {} / {}", name, fmt_bps(rb), fmt_bps(wb));
            let info_val = format!("{:>width$}", info, width = info_w);
            println!("{} {} {} {}", label_pad, info_val, blanks_bar, blanks_pct);
        }
    }

    let up_label_s = format!("{:<width$}", "Up:", width = label_w);
    let up_label = up_label_s.dimmed();
    let up_val = format!("{:>width$}", format!("{}s", s.uptime_secs), width = info_w);
    println!("{} {} {} {}", up_label, up_val, blanks_bar, blanks_pct);

    let load_label_s = format!("{:<width$}", "Load:", width = label_w);
    let load_label = load_label_s.dimmed();
    let load_val = format!(
        "{:>width$}",
        format!(
            "{:.2} {:.2} {:.2}",
            s.load_avg.0, s.load_avg.1, s.load_avg.2
        ),
        width = info_w
    );
    println!("{} {} {} {}", load_label, load_val, blanks_bar, blanks_pct);
}

fn print_help() {
    let help = r#"o - minimal system summary

Usage:
  o [--watch|-w] [--once|-1|-o] [--json|-j] [--io-detail|-I]
    [--sample-ms N] [--io-sample-ms N] [--help|-h]

Modes:
  Default (watch): live-refresh textual dashboard.
  --once, -1, -o: print one textual snapshot and exit.
  --json, -j: print one JSON snapshot and exit.
  --json -w: stream JSON snapshots (newline-delimited).

Options:
  --sample-ms N       Sample window for CPU and disk rates (ms). Default: 1000.
  --io-detail, -I     Include per-device disk I/O lines in textual mode.
  --io-sample-ms N    Sampling window for per-device I/O detail (ms). Default: 2000.
  --watch, -w         Force watch mode (textual or JSON depending on --json).
  --once, -1, -o      Single snapshot and exit (textual when not using --json).
  --help, -h          Show this help and exit.

Examples:
  o
  o -o
  o -j
  o -j -w
  o --io-detail --io-sample-ms 3000
"#;
    print!("{}", help);
}

fn main() {
    let mut json = false;
    let mut sample_ms: u64 = 1000;
    let mut io_detail: bool = false;
    let mut io_detail_sample_ms: u64 = 2000;
    let mut once: bool = false;
    let mut watch: bool = false;
    let mut help: bool = false;

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--help" | "-h" => help = true,
            "--json" | "-j" => json = true,
            "--io-detail" | "-I" => io_detail = true,
            "--io-sample-ms" => {
                if let Some(v) = args.next() {
                    if let Ok(ms) = v.parse::<u64>() {
                        io_detail_sample_ms = ms.max(1);
                    }
                }
            }
            "--once" | "-1" | "-o" => once = true,
            "--watch" | "-w" => watch = true,
            "--sample-ms" => {
                if let Some(v) = args.next() {
                    if let Ok(ms) = v.parse::<u64>() {
                        sample_ms = ms.max(1);
                    }
                }
            }
            _ => {}
        }
    }

    if help {
        print_help();
        return;
    }

    if json {
        if watch {
            loop {
                let s = collect_summary(sample_ms);
                println!("{}", serde_json::to_string(&s).unwrap());
                use std::io::Write;
                std::io::stdout().flush().ok();
            }
        } else {
            let s = collect_summary(sample_ms);
            println!("{}", serde_json::to_string_pretty(&s).unwrap());
        }
    } else {
        let do_watch = if watch {
            true
        } else if once {
            false
        } else {
            true
        };

        if do_watch {
            loop {
                // Clear viewport and scrollback; move cursor home
                print!("\x1b[2J\x1b[H\x1b[3J");
                {
                    use std::io::Write;
                    std::io::stdout().flush().ok();
                }
                let s = collect_summary(sample_ms);
                print_plain(&s, io_detail, io_detail_sample_ms);
                {
                    use std::io::Write;
                    std::io::stdout().flush().ok();
                }
            }
        } else {
            let s = collect_summary(sample_ms);
            print_plain(&s, io_detail, io_detail_sample_ms);
        }
    }
}
