//! TPT Media QC command-line interface ([spec § 16], § 21).
//!
//! The library target exposes the argument model and the run loop so test,
//! benchmark and fuzz harnesses (spec § 24) can drive the CLI in-process.

pub mod app;
pub mod cli;
pub mod exit;
pub mod probe;
pub mod watch;

pub use app::run;
pub use cli::Cli;