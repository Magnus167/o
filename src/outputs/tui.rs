#![cfg(feature = "tui")]
use std::io::stdout;

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

use crate::collect::{collect_summary, fmt_bps};
use crate::io_stats::sample_disk_io_detail;

pub fn run(sample_ms: u64, io_detail_sample_ms: u64) -> std::io::Result<()> {
    enable_raw_mode().map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    let mut out = stdout();
    execute!(out, EnterAlternateScreen).map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    let backend = CrosstermBackend::new(out);
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;

    let mut show_help = false;
    let mut show_io_detail = false;
    let mut refresh_ms = sample_ms.max(1);

    let res = (|| -> std::io::Result<()> {
        loop {
            // Blocking sampling; provides stable rates for CPU/disk over refresh_ms
            let s = collect_summary(refresh_ms);

            terminal.draw(|f| {
                draw_ui(
                    f,
                    &s,
                    show_help,
                    show_io_detail,
                    refresh_ms,
                    io_detail_sample_ms,
                )
            })?;

            // Handle input (non-blocking between ticks here, as collect_summary already slept)
            while event::poll(std::time::Duration::from_millis(0)).map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))? {
                if let Event::Key(key) = event::read().map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))? {
                    match key.code {
                        KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
                        KeyCode::Char('?') => {
                            show_help = !show_help;
                        }
                        KeyCode::Char('i') => {
                            show_io_detail = !show_io_detail;
                        }
                        KeyCode::Char('+') => {
                            // faster refresh (smaller window), clamp to 100ms min
                            let dec = std::cmp::max(50, refresh_ms / 5);
                            refresh_ms = refresh_ms.saturating_sub(dec).max(100);
                        }
                        KeyCode::Char('-') | KeyCode::Char('_') => {
                            // slower refresh (larger window)
                            let inc = std::cmp::max(50, refresh_ms / 5);
                            refresh_ms = refresh_ms.saturating_add(inc);
                        }
                        _ => {}
                    }
                }
            }
        }
    })();

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
    io_detail_sample_ms: u64,
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
        Span::styled("o", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
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
        .gauge_style(Style::default().fg(severity_color(cpu)).add_modifier(Modifier::BOLD))
        .ratio(cpu)
        .label(format!("{:.1}%", s.cpu_percent));
    f.render_widget(cpu_g, chunks[1]);

    let mem_g = Gauge::default()
        .block(Block::default().title("Mem").borders(Borders::ALL))
        .gauge_style(Style::default().fg(severity_color(mem)).add_modifier(Modifier::BOLD))
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
        .gauge_style(Style::default().fg(severity_color(swap)).add_modifier(Modifier::BOLD))
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
                Style::default().fg(Color::Green).add_modifier(Modifier::BOLD),
            ),
            Span::raw(" read / "),
            Span::styled(
                fmt_bps(s.disk_write_bps),
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
            ),
            Span::raw(" write"),
        ]))
        .block(Block::default().borders(Borders::ALL).title("Throughput"))
        .alignment(Alignment::Left);
        f.render_widget(disk, split[0]);

        // Gather IO devices (blocking sample)
        let mut lines: Vec<Line> = Vec::new();
        lines.push(Line::from(Span::styled(
            "Base Device            Read/s         Write/s",
            Style::default().add_modifier(Modifier::BOLD),
        )));
        for (name, rb, wb) in sample_disk_io_detail(io_detail_sample_ms) {
            lines.push(Line::from(vec![
                Span::raw(format!("{:<20}  ", name)),
                Span::styled(
                    format!("{:>10}", fmt_bps(rb)),
                    Style::default().fg(Color::Green),
                ),
                Span::raw("    "),
                Span::styled(
                    format!("{:>10}", fmt_bps(wb)),
                    Style::default().fg(Color::Yellow),
                ),
            ]));
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
                Style::default().fg(Color::Green).add_modifier(Modifier::BOLD),
            ),
            Span::raw(" read / "),
            Span::styled(
                fmt_bps(s.disk_write_bps),
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
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
        Span::raw("?: help  "),
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
            Line::from("?: Toggle this help"),
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
