use crate::collect::{fmt_bps, fmt_bytes, Summary};
use crate::io::sample_disk_io_detail;

use owo_colors::{OwoColorize, Style};

/// Render a plain-text snapshot. Returns a String for easy testing and reuse.
pub fn render_plain(
    s: &Summary,
    io_detail: bool,
    io_detail_sample_ms: u64,
    color_enabled: bool,
) -> String {
    // Style helpers that respect color_enabled
    let apply_style = |text: &str, style: Style| -> String {
        if color_enabled {
            format!("{}", text.style(style))
        } else {
            text.to_string()
        }
    };
    let dim = |text: &str| -> String {
        if color_enabled {
            format!("{}", text.dimmed())
        } else {
            text.to_string()
        }
    };
    let bold = |text: &str| -> String {
        if color_enabled {
            format!("{}", text.bold())
        } else {
            text.to_string()
        }
    };

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

    let mut out = String::new();
    out.push_str(&format!("{}\n", bold("o - system summary")));

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
        let label_fmt = dim(&label_s);

        let info_s = format!("{:>width$}", info, width = info_w);
        let info_fmt = apply_style(&info_s, style);

        let bar_s = bar(*pct, bar_w);
        let bar_fmt = apply_style(&bar_s, style);

        let pct_s = &pct_cols[i];
        let pct_fmt = apply_style(pct_s, style);

        out.push_str(&format!(
            "{} {} {} {}\n",
            label_fmt, info_fmt, bar_fmt, pct_fmt
        ));
    }

    // Aligned non-bar rows (labels, info, and empty bar/pct columns to preserve grid)
    let blanks_bar = " ".repeat(bar_w);
    let blanks_pct = " ".repeat(pct_w + 2); // account for parentheses width

    // Disk speed (Read/s / Write/s)
    let disk_label_s = format!("{:<width$}", "Disk:", width = label_w);
    let disk_label = dim(&disk_label_s);
    let disk_info = format!(
        "{} / {}",
        fmt_bps(s.disk_read_bps),
        fmt_bps(s.disk_write_bps)
    );
    let disk_val = format!("{:>width$}", disk_info, width = info_w);
    out.push_str(&format!(
        "{} {} {} {}\n",
        disk_label, disk_val, blanks_bar, blanks_pct
    ));

    if io_detail {
        let devs = sample_disk_io_detail(io_detail_sample_ms);
        for (name, rb, wb) in devs {
            let label_s = format!("{:<width$}", "", width = label_w);
            let label_pad = dim(&label_s);
            let info = format!("{}: {} / {}", name, fmt_bps(rb), fmt_bps(wb));
            let info_val = format!("{:>width$}", info, width = info_w);
            out.push_str(&format!(
                "{} {} {} {}\n",
                label_pad, info_val, blanks_bar, blanks_pct
            ));
        }
    }

    let up_label_s = format!("{:<width$}", "Up:", width = label_w);
    let up_label = dim(&up_label_s);
    let up_val = format!("{:>width$}", format!("{}s", s.uptime_secs), width = info_w);
    out.push_str(&format!(
        "{} {} {} {}\n",
        up_label, up_val, blanks_bar, blanks_pct
    ));

    let load_label_s = format!("{:<width$}", "Load:", width = label_w);
    let load_label = dim(&load_label_s);
    let load_val = format!(
        "{:>width$}",
        format!(
            "{:.2} {:.2} {:.2}",
            s.load_avg.0, s.load_avg.1, s.load_avg.2
        ),
        width = info_w
    );
    out.push_str(&format!(
        "{} {} {} {}",
        load_label, load_val, blanks_bar, blanks_pct
    ));

    out
}

/// Print a plain-text snapshot directly to stdout.
pub fn print_plain(s: &Summary, io_detail: bool, io_detail_sample_ms: u64, color_enabled: bool) {
    let rendered = render_plain(s, io_detail, io_detail_sample_ms, color_enabled);
    println!("{}", rendered);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_plain_includes_labels() {
        let s = Summary {
            cpu_percent: 12.3,
            mem_used: 1024 * 1024,
            mem_total: 4 * 1024 * 1024,
            swap_used: 0,
            swap_total: 0,
            uptime_secs: 42,
            load_avg: (0.12, 0.34, 0.56),
            disk_read_bps: 2048.0,
            disk_write_bps: 512.0,
        };
        std::env::set_var("COLUMNS", "80");
        let out = render_plain(&s, false, 0, false);
        assert!(out.contains("o - system summary"));
        assert!(out.contains("CPU:"));
        assert!(out.contains("Mem:"));
        assert!(out.contains("Swap:"));
        assert!(out.contains("Disk:"));
        assert!(out.contains("Up:"));
        assert!(out.contains("Load:"));
    }
}
