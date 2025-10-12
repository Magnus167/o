use clap::{Parser, ValueEnum};

/// o - customizable command line system monitor
///
/// Modes:
///   Default (watch): live-refresh textual dashboard.
///   --once, -1, -o: print one textual snapshot and exit.
///   --json, -j: print one JSON snapshot and exit.
///   --json -w: stream JSON snapshots (newline-delimited).
///
/// Examples:
///   o
///   o -o
///   o -j
///   o -j -w
///   o --io-detail --io-sample-ms 3000
#[derive(Parser, Debug, Clone)]
#[command(
    name = "o",
    version,
    author,
    about = "A customizable command line system monitor.",
    long_about = r#"o - customizable command line system monitor

Modes:
  Default: TUI (when built with the `tui` feature). Falls back to plain when unavailable.
  --plain           Force minimal textual renderer.
  --once, -1, -o    Print one textual snapshot and exit (plain mode only).
  --json, -j        Print one JSON snapshot and exit.
  --json -w         Stream JSON snapshots (newline-delimited).

Plain:
  Defaults to a single snapshot; pass --watch for continuous updates.

Color:
  Respects NO_COLOR; override with --color {auto,always,never}.

TUI:
  Keys: q quit, h help, i toggle I/O detail, + faster, - slower."#,
    after_help = r#"Examples:
  o                               # default TUI (when built with feature)
  o --plain                       # plain watch
  o --plain -o                    # one plain snapshot
  o -j                            # one JSON snapshot
  o -j -w                         # JSON stream
  o --io-detail --io-sample-ms 3000
  cargo run --features tui --     # TUI default at runtime"#,
    disable_help_subcommand = true
)]
pub struct Args {
    /// Force watch mode (textual or JSON depending on --json)
    #[arg(long, short = 'w')]
    pub watch: bool,

    /// Single snapshot and exit (textual when not using --json)
    #[arg(long, short = 'o', alias = "1")]
    pub once: bool,

    /// JSON output (pretty for single-shot, NDJSON for --watch)
    #[arg(long, short = 'j')]
    pub json: bool,

    /// Include aggregated per-base-device disk I/O lines in textual mode
    #[arg(long = "io-detail", short = 'I')]
    pub io_detail: bool,

    /// Sample window for CPU and disk rates (ms)
    #[arg(long = "sample-ms", default_value_t = 1000)]
    pub sample_ms: u64,

    /// Sampling window for per-device I/O detail (ms)
    #[arg(long = "io-sample-ms", default_value_t = 2000)]
    pub io_sample_ms: u64,

    /// Force plain text renderer (disables TUI even if available)
    #[arg(long = "plain")]
    pub plain: bool,

    /// Control colorization of plain/text output
    #[arg(long = "color", value_enum, default_value_t = ColorWhen::Auto)]
    pub color: ColorWhen,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, ValueEnum)]
pub enum ColorWhen {
    Auto,
    Always,
    Never,
}

impl Args {
    /// Determine whether to colorize our own output (independent from clap's help color).
    pub fn color_enabled(&self) -> bool {
        match self.color {
            ColorWhen::Always => true,
            ColorWhen::Never => false,
            ColorWhen::Auto => std::env::var_os("NO_COLOR").is_none(),
        }
    }
}

pub fn parse() -> Args {
    Args::parse()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_sensible() {
        let a = Args::parse_from(["o"]);
        assert!(!a.json);
        assert!(!a.watch);
        assert!(!a.once);
        assert!(!a.plain);
        assert_eq!(a.sample_ms, 1000);
        assert_eq!(a.io_sample_ms, 2000);
        assert_eq!(a.color, ColorWhen::Auto);
    }

    #[test]
    fn parse_short_flags() {
        let a = Args::parse_from(["o", "-j", "-w", "-o"]);
        assert!(a.json);
        assert!(a.watch);
        assert!(a.once);
    }

    #[test]
    fn color_policy() {
        let mut a = Args::parse_from(["o"]);
        a.color = ColorWhen::Always;
        assert!(a.color_enabled());
        a.color = ColorWhen::Never;
        assert!(!a.color_enabled());
        a.color = ColorWhen::Auto;
        std::env::remove_var("NO_COLOR");
        assert!(a.color_enabled());
        std::env::set_var("NO_COLOR", "1");
        assert!(!a.color_enabled());
        std::env::remove_var("NO_COLOR");
    }
}
