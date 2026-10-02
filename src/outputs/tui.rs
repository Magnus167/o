use std::{
    io::stdout,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc, Arc,
    },
    thread,
    time::Duration,
};

use crossterm::{
    event::{self, Event, KeyCode, KeyEvent, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Gauge, Paragraph, Row, Table},
    Frame, Terminal,
};

use crate::collect::{
    collect_summary_with_top, fmt_bps, fmt_bytes, top_processes, ProcInfo, ProcSort, Summary,
};
use crate::io::{sample_disk_io_detail, sample_net_io_detail};

/// Non-blocking TUI runner with background samplers for system summary and (optional) I/O detail.
/// - Keystrokes are handled immediately (poll with small timeout)
/// - Sampling sleeps happen off the UI thread
/// - Process panel holds up to `top` rows (10 when `top` is 0); `p` toggles `sort`
pub fn run(
    sample_ms: u64,
    io_detail_sample_ms: u64,
    top: usize,
    sort: ProcSort,
) -> std::io::Result<()> {
    enable_raw_mode().map_err(std::io::Error::other)?;
    let mut out = stdout();
    execute!(out, EnterAlternateScreen).map_err(std::io::Error::other)?;
    let backend = CrosstermBackend::new(out);
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;

    // UI state
    let mut show_help = false;
    let mut show_io_detail = false;

    // Shared refresh interval for sampler thread
    let refresh_ms = Arc::new(AtomicU64::new(sample_ms.max(1)));
    let refresh_min = sysinfo::MINIMUM_CPU_UPDATE_INTERVAL.as_millis() as u64;

    let proc_cap = if top > 0 { top } else { 10 };
    let sort_mem = Arc::new(AtomicBool::new(sort == ProcSort::Mem));

    // Sampler thread (produces Summary on a channel)
    let (sum_tx, sum_rx) = mpsc::channel::<Summary>();
    let sampler_stop = Arc::new(AtomicBool::new(false));
    {
        let refresh_ms = Arc::clone(&refresh_ms);
        let sampler_stop = Arc::clone(&sampler_stop);
        let sort_mem = Arc::clone(&sort_mem);
        thread::spawn(move || {
            // Initial snapshot to avoid blank UI
            let initial = collect_summary_with_top(
                refresh_ms.load(Ordering::Relaxed),
                proc_cap,
                sort_of(&sort_mem),
            );
            let _ = sum_tx.send(initial);

            while !sampler_stop.load(Ordering::Relaxed) {
                let win = refresh_ms.load(Ordering::Relaxed);
                let s = collect_summary_with_top(win, proc_cap, sort_of(&sort_mem));
                if sum_tx.send(s).is_err() {
                    break;
                }
            }
        });
    }

    // Optional I/O detail worker (started/stopped on demand)
    let mut io_rx: Option<mpsc::Receiver<Vec<(String, f64, f64)>>> = None;
    let mut io_stop: Option<Arc<AtomicBool>> = None;
    let mut io_thread_alive = false;

    // Last received data
    let mut last_summary: Option<Summary> = None;
    let mut last_io_detail: Option<Vec<(String, f64, f64)>> = None;

    // Main UI loop
    let res = (|| -> std::io::Result<()> {
        loop {
            // Drain any available summaries
            for s in sum_rx.try_iter() {
                last_summary = Some(s);
            }

            // Drain any available I/O detail
            if let Some(rx) = &io_rx {
                for io in rx.try_iter() {
                    last_io_detail = Some(io);
                }
            }

            // Draw if we have a summary
            if let Some(ref s) = last_summary {
                terminal.draw(|f| {
                    draw_ui(
                        f,
                        s,
                        show_help,
                        show_io_detail,
                        refresh_ms.load(Ordering::Relaxed),
                        last_io_detail.as_deref(),
                        sort_of(&sort_mem),
                    )
                })?;
            }

            // Poll input with a short timeout to keep interaction snappy
            if event::poll(Duration::from_millis(50)).map_err(std::io::Error::other)? {
                let mut handle_key = |key: KeyEvent| -> bool {
                    if key.kind != KeyEventKind::Press {
                        return false;
                    }
                    match key.code {
                        KeyCode::Char('q') | KeyCode::Esc => true,
                        // Map help to 'h'; keep '?' as an alias
                        KeyCode::Char('h') | KeyCode::Char('?') => {
                            show_help = !show_help;
                            false
                        }
                        KeyCode::Char('i') => {
                            show_io_detail = !show_io_detail;
                            if show_io_detail {
                                // Start I/O worker
                                let (tx, rx) = mpsc::channel::<Vec<(String, f64, f64)>>();
                                let stop = Arc::new(AtomicBool::new(false));
                                let stop_clone = Arc::clone(&stop);
                                let sample_ms = io_detail_sample_ms.max(1);
                                thread::spawn(move || {
                                    while !stop_clone.load(Ordering::Relaxed) {
                                        let mut data = sample_disk_io_detail(sample_ms);
                                        let net_data = sample_net_io_detail(sample_ms);
                                        // Combine disk and network data
                                        data.extend(net_data);
                                        if tx.send(data).is_err() {
                                            break;
                                        }
                                    }
                                });
                                io_rx = Some(rx);
                                io_stop = Some(stop);
                                io_thread_alive = true;
                                // Drop last I/O data so the UI shows fresh values for the newly enabled panel
                                last_io_detail = None;
                            } else {
                                // Stop I/O worker
                                if let Some(stop) = io_stop.take() {
                                    stop.store(true, Ordering::Relaxed);
                                }
                                io_rx = None;
                                io_thread_alive = false;
                            }
                            false
                        }
                        KeyCode::Char('p') => {
                            sort_mem.fetch_xor(true, Ordering::Relaxed);
                            false
                        }
                        KeyCode::Char('+') => {
                            // faster refresh (smaller window), clamped to the collector's CPU floor
                            let cur = refresh_ms.load(Ordering::Relaxed);
                            let dec = std::cmp::max(50, cur / 5);
                            let next = cur.saturating_sub(dec).max(refresh_min);
                            refresh_ms.store(next, Ordering::Relaxed);
                            false
                        }
                        KeyCode::Char('-') | KeyCode::Char('_') => {
                            // slower refresh (larger window)
                            let cur = refresh_ms.load(Ordering::Relaxed);
                            let inc = std::cmp::max(50, cur / 5);
                            let next = cur.saturating_add(inc);
                            refresh_ms.store(next, Ordering::Relaxed);
                            false
                        }
                        _ => false,
                    }
                };

                let mut should_exit = false;

                if let Event::Key(key) = event::read().map_err(std::io::Error::other)? {
                    should_exit = handle_key(key);
                }

                while !should_exit
                    && event::poll(Duration::from_millis(0)).map_err(std::io::Error::other)?
                {
                    if let Event::Key(key) = event::read().map_err(std::io::Error::other)? {
                        should_exit = handle_key(key);
                    }
                }

                if should_exit {
                    break;
                }
            }
        }
        Ok(())
    })();

    // Stop workers before restoring terminal
    sampler_stop.store(true, Ordering::Relaxed);
    if io_thread_alive {
        if let Some(stop) = io_stop.take() {
            stop.store(true, Ordering::Relaxed);
        }
    }

    // Restore terminal
    disable_raw_mode().ok();
    execute!(terminal.backend_mut(), LeaveAlternateScreen).ok();
    terminal.show_cursor().ok();

    res
}

fn draw_ui(
    f: &mut Frame<'_>,
    s: &crate::collect::Summary,
    show_help: bool,
    show_io_detail: bool,
    refresh_ms: u64,
    io_detail: Option<&[(String, f64, f64)]>,
    sort: ProcSort,
) {
    let size = f.size();

    // Root layout
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // title
            Constraint::Length(3), // CPU
            Constraint::Length(3), // Mem
            Constraint::Length(3), // Swap
            Constraint::Min(3),    // Disk and optional IO detail
            Constraint::Length(1), // footer
        ])
        .split(size);

    // Title
    let title = Paragraph::new(Line::from(vec![
        Span::styled(
            "o",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" - system monitor"),
    ]))
    .alignment(Alignment::Left);
    f.render_widget(title, chunks[0]);

    // Gauges
    let cpu = clamp_ratio(s.cpu_percent as f64 / 100.0);
    let mem = if s.mem_total > 0 {
        clamp_ratio(s.mem_used as f64 / s.mem_total as f64)
    } else {
        0.0
    };
    let swap = if s.swap_total > 0 {
        clamp_ratio(s.swap_used as f64 / s.swap_total as f64)
    } else {
        0.0
    };

    let cpu_g = Gauge::default()
        .block(Block::default().title("CPU").borders(Borders::ALL))
        .gauge_style(
            Style::default()
                .fg(severity_color(cpu))
                .add_modifier(Modifier::BOLD),
        )
        .ratio(cpu)
        .label(format!("{:.1}%", s.cpu_percent));
    f.render_widget(cpu_g, chunks[1]);

    let mem_g = Gauge::default()
        .block(Block::default().title("Mem").borders(Borders::ALL))
        .gauge_style(
            Style::default()
                .fg(severity_color(mem))
                .add_modifier(Modifier::BOLD),
        )
        .ratio(mem)
        .label(format!(
            "{:.1}%  ({} / {})",
            mem * 100.0,
            crate::collect::fmt_bytes(s.mem_used),
            crate::collect::fmt_bytes(s.mem_total)
        ));
    f.render_widget(mem_g, chunks[2]);

    let swap_g = Gauge::default()
        .block(Block::default().title("Swap").borders(Borders::ALL))
        .gauge_style(
            Style::default()
                .fg(severity_color(swap))
                .add_modifier(Modifier::BOLD),
        )
        .ratio(swap)
        .label(format!(
            "{:.1}%  ({} / {})",
            swap * 100.0,
            crate::collect::fmt_bytes(s.swap_used),
            crate::collect::fmt_bytes(s.swap_total)
        ));
    f.render_widget(swap_g, chunks[3]);

    // Processes take the rows the throughput / I/O panels leave over; the I/O detail list keeps
    // priority because the user asked for it explicitly.
    let io_rows = if show_io_detail {
        (io_detail.map_or(1, |d| d.len()) + 3).max(4)
    } else {
        4
    };
    let proc_rows = proc_panel_height(chunks[4].height, io_rows, s.processes.len());
    let lower = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(proc_rows)])
        .split(chunks[4]);
    let io_area = lower[0];
    if proc_rows > 0 {
        draw_processes(f, lower[1], &s.processes, sort);
    }

    // Disk + Network + IO detail area
    if show_io_detail {
        // Split horizontally: left throughput summary, right IO devices
        let split = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(io_area);

        let throughput = Paragraph::new(vec![
            Line::from(vec![
                Span::raw("Disk: "),
                Span::styled(
                    fmt_bps(s.disk_read_bps),
                    Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(" read / "),
                Span::styled(
                    fmt_bps(s.disk_write_bps),
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(" write"),
            ]),
            Line::from(vec![
                Span::raw("Net:  "),
                Span::styled(
                    fmt_bps(s.net_rx_bps),
                    Style::default()
                        .fg(Color::Blue)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(" rx   / "),
                Span::styled(
                    fmt_bps(s.net_tx_bps),
                    Style::default()
                        .fg(Color::Magenta)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(" tx"),
            ]),
        ])
        .block(Block::default().borders(Borders::ALL).title("Throughput"))
        .alignment(Alignment::Left);
        f.render_widget(throughput, split[0]);

        // I/O detail data (non-blocking; render last received)
        let mut lines: Vec<Line> = Vec::new();
        lines.push(Line::from(Span::styled(
            "Device / Interface     Read/RX        Write/TX",
            Style::default().add_modifier(Modifier::BOLD),
        )));
        if let Some(devs) = io_detail {
            for (name, rb, wb) in devs {
                lines.push(Line::from(vec![
                    Span::raw(format!("{:<20}  ", name)),
                    Span::styled(
                        format!("{:>10}", fmt_bps(*rb)),
                        Style::default().fg(Color::Green),
                    ),
                    Span::raw("    "),
                    Span::styled(
                        format!("{:>10}", fmt_bps(*wb)),
                        Style::default().fg(Color::Yellow),
                    ),
                ]));
            }
        } else {
            lines.push(Line::from(Span::raw("Collecting I/O detail...")));
        }
        let detail = Paragraph::new(lines)
            .block(Block::default().borders(Borders::ALL).title("I/O Detail"))
            .alignment(Alignment::Left);
        f.render_widget(detail, split[1]);
    } else {
        let throughput = Paragraph::new(vec![
            Line::from(vec![
                Span::raw("Disk: "),
                Span::styled(
                    fmt_bps(s.disk_read_bps),
                    Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(" read / "),
                Span::styled(
                    fmt_bps(s.disk_write_bps),
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(" write"),
            ]),
            Line::from(vec![
                Span::raw("Net:  "),
                Span::styled(
                    fmt_bps(s.net_rx_bps),
                    Style::default()
                        .fg(Color::Blue)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(" rx   / "),
                Span::styled(
                    fmt_bps(s.net_tx_bps),
                    Style::default()
                        .fg(Color::Magenta)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(" tx"),
            ]),
        ])
        .block(Block::default().borders(Borders::ALL).title("Throughput"))
        .alignment(Alignment::Left);
        f.render_widget(throughput, io_area);
    }

    // Footer / hints
    let footer = Paragraph::new(Line::from(vec![
        Span::raw("q: quit  "),
        Span::raw("h: help  "),
        Span::raw("i: toggle IO detail  "),
        Span::raw("+: faster  "),
        Span::raw("-: slower  "),
        Span::raw("| refresh: "),
        Span::styled(
            format!("{} ms", refresh_ms),
            Style::default().fg(Color::Cyan),
        ),
    ]))
    .alignment(Alignment::Left);
    f.render_widget(footer, chunks[5]);

    // Help overlay
    if show_help {
        let area = centered_rect(70, 60, size);
        let help_lines = vec![
            Line::from(Span::styled(
                "Help",
                Style::default().add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from("q: Quit"),
            Line::from("h: Toggle this help"),
            Line::from("i: Toggle I/O detail panel"),
            Line::from("p: Toggle process sort (CPU / memory)"),
            Line::from("+: Speed up refresh (smaller sampling window)"),
            Line::from("-: Slow down refresh (larger sampling window)"),
            Line::from(""),
            Line::from("JSON/plain modes remain available via CLI flags."),
        ];
        let help = Paragraph::new(help_lines)
            .block(Block::default().borders(Borders::ALL).title("Controls"))
            .alignment(Alignment::Left);
        // Clear the area behind overlay
        f.render_widget(Clear, area);
        f.render_widget(help, area);
    }
}

/// Panel height: one row per process plus header and borders, without eating into `reserved`
/// rows. 0 hides the panel when not even one process row fits.
fn proc_panel_height(avail: u16, reserved: usize, procs: usize) -> u16 {
    let free = usize::from(avail).saturating_sub(reserved);
    if procs == 0 || free < 4 {
        0
    } else {
        // Bounded by `free`, which came from a u16.
        (procs + 3).min(free) as u16
    }
}

fn draw_processes(f: &mut Frame<'_>, area: Rect, procs: &[ProcInfo], sort: ProcSort) {
    let header = Row::new([
        format!("{:>7}", "PID"),
        "NAME".to_string(),
        format!("{:>6}", "CPU%"),
        format!("{:>9}", "MEM"),
    ])
    .style(Style::default().add_modifier(Modifier::BOLD));
    // Re-sort locally so a `p` press reorders at once instead of after the next sample.
    let rows = top_processes(procs.to_vec(), procs.len(), sort)
        .into_iter()
        .map(|p| {
            Row::new([
                format!("{:>7}", p.pid),
                p.name,
                format!("{:>6.1}", p.cpu_percent),
                format!("{:>9}", fmt_bytes(p.mem_bytes)),
            ])
        });
    let by = match sort {
        ProcSort::Cpu => "CPU",
        ProcSort::Mem => "memory",
    };
    let widths = [
        Constraint::Length(7),
        Constraint::Min(4),
        Constraint::Length(6),
        Constraint::Length(9),
    ];
    let table = Table::new(rows, widths).header(header).block(
        Block::default()
            .borders(Borders::ALL)
            .title(format!("Top processes by {by} (p: toggle)")),
    );
    f.render_widget(table, area);
}

fn sort_of(mem: &AtomicBool) -> ProcSort {
    if mem.load(Ordering::Relaxed) {
        ProcSort::Mem
    } else {
        ProcSort::Cpu
    }
}

fn clamp_ratio(v: f64) -> f64 {
    if v.is_finite() {
        v.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

fn severity_color(ratio: f64) -> Color {
    let p = ratio * 100.0;
    if p >= 80.0 {
        Color::Red
    } else if p >= 50.0 {
        Color::Yellow
    } else {
        Color::Green
    }
}

fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let vert = Layout::default()
        .direction(Direction::Vertical)
        .constraints(
            [
                Constraint::Percentage((100 - percent_y) / 2),
                Constraint::Percentage(percent_y),
                Constraint::Percentage((100 - percent_y) / 2),
            ]
            .as_ref(),
        )
        .split(r);

    let hor = Layout::default()
        .direction(Direction::Horizontal)
        .constraints(
            [
                Constraint::Percentage((100 - percent_x) / 2),
                Constraint::Percentage(percent_x),
                Constraint::Percentage((100 - percent_x) / 2),
            ]
            .as_ref(),
        )
        .split(vert[1]);

    hor[1]
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::layout::Rect;

    #[test]
    fn clamp_ratio_bounds_and_nans() {
        assert_eq!(super::clamp_ratio(-0.5), 0.0);
        assert_eq!(super::clamp_ratio(0.0), 0.0);
        assert!((super::clamp_ratio(0.5) - 0.5).abs() < 1e-9);
        assert_eq!(super::clamp_ratio(1.5), 1.0);
        assert_eq!(super::clamp_ratio(f64::NAN), 0.0);
        assert_eq!(super::clamp_ratio(f64::INFINITY), 0.0); // not finite -> 0.0
    }

    #[test]
    fn severity_color_thresholds() {
        // Note: function expects ratio in [0,1]
        assert!(matches!(super::severity_color(0.0), Color::Green));
        assert!(matches!(super::severity_color(0.49), Color::Green));
        assert!(matches!(super::severity_color(0.50), Color::Yellow));
        assert!(matches!(super::severity_color(0.79), Color::Yellow));
        assert!(matches!(super::severity_color(0.80), Color::Red));
        assert!(matches!(super::severity_color(1.0), Color::Red));
    }

    #[test]
    fn processes_panel_shows_when_room_and_hides_when_small() {
        use ratatui::backend::TestBackend;
        let s = Summary {
            cpu_percent: 0.0,
            mem_used: 0,
            mem_total: 0,
            swap_used: 0,
            swap_total: 0,
            uptime_secs: 0,
            load_avg: (0.0, 0.0, 0.0),
            disk_read_bps: 0.0,
            disk_write_bps: 0.0,
            net_rx_bps: 0.0,
            net_tx_bps: 0.0,
            processes: vec![
                ProcInfo {
                    pid: 1,
                    name: "small-cpu-big-mem".into(),
                    cpu_percent: 1.0,
                    mem_bytes: 9 << 20,
                },
                ProcInfo {
                    pid: 2,
                    name: "big-cpu".into(),
                    cpu_percent: 50.0,
                    mem_bytes: 1 << 20,
                },
            ],
        };
        let render = |h: u16, sort: ProcSort| {
            let mut t = Terminal::new(TestBackend::new(80, h)).unwrap();
            t.draw(|f| draw_ui(f, &s, false, false, 1000, None, sort))
                .unwrap();
            let buf = t.backend().buffer();
            buf.content().iter().map(|c| c.symbol()).collect::<String>()
        };

        let pos = |hay: &str, needle: &str| hay.find(needle).expect(needle);
        let by_cpu = render(30, ProcSort::Cpu);
        assert!(by_cpu.contains("Top processes by CPU") && by_cpu.contains("Disk:"));
        assert!(pos(&by_cpu, "big-cpu") < pos(&by_cpu, "small-cpu-big-mem"));
        let by_mem = render(30, ProcSort::Mem);
        assert!(pos(&by_mem, "small-cpu-big-mem") < pos(&by_mem, "big-cpu"));

        let small = render(15, ProcSort::Cpu);
        assert!(!small.contains("Top processes") && small.contains("Disk:"));
    }

    #[test]
    fn centered_rect_geometry() {
        let outer = Rect::new(0, 0, 100, 50);
        let inner = super::centered_rect(50, 50, outer);
        // Expect roughly centered half-size rect
        assert_eq!(inner.width, 50);
        assert_eq!(inner.height, 25);
        // X/Y positions should place it centered
        assert!(inner.x > 0 && inner.x < 100);
        assert!(inner.y > 0 && inner.y < 50);
    }
}
