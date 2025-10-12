//! o — a customizable command line system monitor.
//!
//! This library exposes a small, documented API surface so the binary can stay thin
//! and consumers can reuse core functionality (collection, formatting, and rendering).
//!
//! Layers:
//! - cli: argument parsing and color policy
//! - collect: system snapshot (CPU, memory, load, IO totals) and simple format helpers
//! - io: disk statistics sampling and base-device aggregation helpers
//! - outputs: renderers (plain text; optional TUI behind the `tui` feature)
//!
//! Examples
//!
//! Collect and print a single plain snapshot without colors:
//! ```no_run
//! use o::{collect_summary, outputs::plain::render_plain};
//!
//! let s = collect_summary(500);
//! let out = render_plain(&s, false, 2000, false);
//! println!("{}", out);
//! ```
//!
//! JSON single-shot (pretty) can be produced by serializing [`collect::Summary`]:
//! ```no_run
//! use o::collect_summary;
//! let s = collect_summary(1000);
//! println!("{}", serde_json::to_string_pretty(&s).unwrap());
//! ```
pub mod cli;
pub mod collect;
pub mod io;
pub mod outputs;

pub use collect::{collect_summary, fmt_bps, fmt_bytes, Summary};
