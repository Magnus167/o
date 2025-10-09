#![cfg(feature = "tui")]
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
    event::{self, Event, KeyCode},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Gauge, Paragraph},
    Frame, Terminal,
};

use crate::collect::{collect_summary, fmt_bps, Summary};
use crate::io_stats::sample_disk_io_detail;

/// Non-blocking TUI runner with background samplers for system summary and (optional) I/O detail.
/// - Keystrokes are handled immediately (poll with small timeout)
/// - Sampling sleeps happen off the UI thread
pub fn run(sample_ms: u64, io_detail_sample_ms: u64) -> std::io::Result<()> {
    enable_raw_mode().map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    let mut out = stdout();
    execute!(out, EnterAlternateScreen)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    let backend = CrosstermBackend::new(out);
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;

    // UI state
    let mut show_help = false;
    let mut show_io_detail = false;

    // Shared refresh interval for sampler thread
    let refresh_ms = Arc::new(AtomicU64::new(sample_ms.max(1)));
    let refresh_min = 100u64;

    // Sampler thread (produces Summary on a channel)
    let (sum_tx, sum_rx) = mpsc::channel::<Summary>();
    let sampler_stop = Arc::new(AtomicBool::new(false));
    {
        let refresh_ms = Arc::clone(&refresh_ms);
        let sampler_stop = Arc::clone(&sampler_stop);
        thread::spawn(move || {
            // Initial snapshot to avoid blank UI
            let initial = collect_summary(refresh_ms.load(Ordering::Relaxed));
            let _ = sum_tx.send(initial);

            while !sampler_stop.load(Ordering::Relaxed) {
                let win = refresh_ms.load(Ordering::Relaxed);
                let s = collect_summary(win);
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
                    )
                })?;
            }

            // Poll input with a short timeout to keep interaction snappy
            if event::poll(Duration::from_millis(50))
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?
            {
                if let Event::Key(key) =
                    event::read().map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?
                {
                    match key.code {
                        KeyCode::Char('q') | KeyCode::Esc => break,
                        // Map help to 'h'; keep '?' as an alias
                        KeyCode::Char('h') | KeyCode::Char('?') => {
                            show_help = !show_help;
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
                                        let data = sample_disk_io_detail(sample_ms);
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
                        }
                        KeyCode::Char('+') => {
                            // faster refresh (smaller window), clamp to 100ms min
                            let cur = refresh_ms.load(Ordering::Relaxed);
                            let dec = std::cmp::max(50, cur / 5);
                            let next = cur.saturating_sub(dec).max(refresh_min);
                            refresh_ms.store(next, Ordering::Relaxed);
                        }
                        KeyCode::Char('-') | KeyCode::Char('_') => {
                            // slower refresh (larger window)
                            let cur = refresh_ms.load(Ordering::Relaxed);
                            let inc = std::cmp::max(50, cur / 5);
                            let next = cur.saturating_add(inc);
                            refresh_ms.store(next, Ordering::Relaxed);
                        }
                        _ => {}
                    }
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

    // Disk + IO detail area
    if show_io_detail {
        // Split horizontally: left disk summary, right IO devices
        let split = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(chunks[4]);

        let disk = Paragraph::new(Line::from(vec![
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
        ]))
        .block(Block::default().borders(Borders::ALL).title("Throughput"))
        .alignment(Alignment::Left);
        f.render_widget(disk, split[0]);

        // I/O detail data (non-blocking; render last received)
        let mut lines: Vec<Line> = Vec::new();
        lines.push(Line::from(Span::styled(
            "Base Device            Read/s         Write/s",
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
        let disk = Paragraph::new(Line::from(vec![
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
        ]))
        .block(Block::default().borders(Borders::ALL).title("Throughput"))
        .alignment(Alignment::Left);
        f.render_widget(disk, chunks[4]);
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
