use o::cli::parse;
use o::collect::{collect_summary, Summary};
use std::io::Write;

fn main() {
    let args = parse();

    // JSON modes (single or stream)
    if args.json {
        if args.watch {
            loop {
                let s = collect_summary(args.sample_ms);
                println!("{}", serde_json::to_string(&s).unwrap());
                std::io::stdout().flush().ok();
            }
        } else {
            let s = collect_summary(args.sample_ms);
            println!("{}", serde_json::to_string_pretty(&s).unwrap());
        }
        return;
    }

    // Default to TUI when available (unless --plain), otherwise fall back to plain
    #[cfg(feature = "tui")]
    {
        use std::io::IsTerminal;
        // A pipe or redirect has no terminal to draw on, so fall back to plain output.
        if !args.plain && std::io::stdout().is_terminal() {
            if let Err(e) = o::outputs::tui::run(args.sample_ms, args.io_sample_ms) {
                eprintln!("tui error: {e}");
                std::process::exit(1);
            }
            return;
        }
    }

    // Plain text modes
    // In plain mode, default to a single snapshot unless --watch is explicitly set.
    let do_watch = if args.once { false } else { args.watch };

    if do_watch {
        plain_watch(args);
    } else {
        let s = collect_summary(args.sample_ms);
        o::outputs::plain::print_plain(&s, args.io_detail, args.io_sample_ms, args.color_enabled());
    }
}

fn plain_watch(args: o::cli::Args) {
    use crossterm::{
        cursor::MoveTo,
        execute,
        terminal::{Clear, ClearType},
    };
    let mut stdout = std::io::stdout();

    // Main watch loop on the primary screen buffer
    loop {
        // Clear the viewport and move cursor to home
        execute!(&mut stdout, Clear(ClearType::All), MoveTo(0, 0)).ok();

        let s: Summary = collect_summary(args.sample_ms);
        o::outputs::plain::print_plain(&s, args.io_detail, args.io_sample_ms, args.color_enabled());

        // Flush to ensure the frame is visible
        stdout.flush().ok();
    }
}
