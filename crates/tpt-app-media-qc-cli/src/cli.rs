//! Command-line argument definitions.

use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(
    name = "tpt-media-qc",
    version,
    about = "Offline-first professional media quality-control and validation tool",
    long_about = "TPT Media QC performs rule-based quality control of media files against\nversioned, deterministic profiles. Exit codes are stable per spec §16."
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// QC a single media file against a profile.
    Check {
        /// Media file to inspect.
        file: PathBuf,

        /// YAML profile file. Defaults to the bundled `generic` profile.
        #[arg(long, short = 'p')]
        profile: Option<PathBuf>,

        /// Metadata-only (quick) scan; skip decode-based measurements.
        #[arg(long)]
        quick: bool,

        /// Write the JSON report to this path.
        #[arg(long)]
        json: Option<PathBuf>,

        /// Write the HTML report to this path.
        #[arg(long)]
        html: Option<PathBuf>,

        /// Write a PDF report to this path.
        #[arg(long)]
        pdf: Option<PathBuf>,

        /// Write the CSV findings to this path.
        #[arg(long)]
        csv: Option<PathBuf>,

        /// Suppress per-finding output (summary + verdict only).
        #[arg(long)]
        quiet: bool,
    },

    /// QC several files (or a directory) against a profile.
    Batch {
        /// One or more files or directories to scan recursively.
        files: Vec<PathBuf>,

        /// YAML profile file.
        #[arg(long, short = 'p')]
        profile: Option<PathBuf>,

        /// Metadata-only (quick) scan.
        #[arg(long)]
        quick: bool,

        /// Directory to write reports into (one JSON per asset).
        #[arg(long)]
        out: Option<PathBuf>,

        /// Stop at the first failing asset (still reports it).
        #[arg(long)]
        fail_fast: bool,
    },

    /// Print container and stream metadata for a file.
    Info {
        /// Media file to inspect.
        file: PathBuf,

        /// YAML profile file (used only for inspection depth decisions).
        #[arg(long, short = 'p')]
        profile: Option<PathBuf>,

        /// Metadata-only scan (no decode pass).
        #[arg(long)]
        quick: bool,
    },

    /// List the rules the given profile enables (or the full catalogue).
    ListRules {
        /// YAML profile file.
        #[arg(long, short = 'p')]
        profile: Option<PathBuf>,
    },

    /// Watch an input folder and route QC results to pass/warn/fail folders
    /// (spec §13, fully local).
    Watch {
        /// Input folder to monitor recursively.
        #[arg(long)]
        input: PathBuf,

        /// YAML profile file. Defaults to the bundled `generic` profile.
        #[arg(long, short = 'p')]
        profile: Option<PathBuf>,

        /// Destination for assets whose QC verdict is PASS (spec §13).
        #[arg(long)]
        pass: PathBuf,

        /// Destination for assets whose QC verdict is WARN.
        #[arg(long)]
        warn: PathBuf,

        /// Destination for assets whose QC verdict is FAIL.
        #[arg(long)]
        fail: PathBuf,

        /// Directory for per-asset JSON reports (optional).
        #[arg(long)]
        report: Option<PathBuf>,

        /// Metadata-only (quick) processing for each asset.
        #[arg(long)]
        quick: bool,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn parses_check_defaults() {
        let cli = Cli::try_parse_from(["tpt-media-qc", "check", "x.mp4"]).unwrap();
        let Command::Check {
            file,
            profile,
            quick,
            json,
            html,
            pdf,
            csv,
            quiet,
        } = cli.command
        else {
            panic!("expected check");
        };
        assert_eq!(file, PathBuf::from("x.mp4"));
        assert!(profile.is_none());
        assert!(!quick);
        assert!(json.is_none() && html.is_none() && pdf.is_none() && csv.is_none());
        assert!(!quiet);
    }

    #[test]
    fn parses_check_flags() {
        let cli = Cli::try_parse_from([
            "tpt-media-qc",
            "check",
            "x.mov",
            "--quick",
            "--profile",
            "p.yaml",
            "--json",
            "r.json",
            "--html",
            "r.html",
            "--pdf",
            "r.pdf",
            "--csv",
            "r.csv",
            "--quiet",
        ])
        .unwrap();
        let Command::Check {
            file,
            profile,
            quick,
            json,
            html,
            pdf,
            csv,
            quiet,
        } = cli.command
        else {
            panic!("expected check");
        };
        assert_eq!(file, PathBuf::from("x.mov"));
        assert_eq!(profile.as_deref(), Some(std::path::Path::new("p.yaml")));
        assert!(quick && quiet);
        assert_eq!(json.as_deref(), Some(std::path::Path::new("r.json")));
        assert_eq!(html.as_deref(), Some(std::path::Path::new("r.html")));
        assert_eq!(pdf.as_deref(), Some(std::path::Path::new("r.pdf")));
        assert_eq!(csv.as_deref(), Some(std::path::Path::new("r.csv")));
    }

    #[test]
    fn parses_watch_flags() {
        let cli = Cli::try_parse_from([
            "tpt-media-qc",
            "watch",
            "--input",
            "Incoming",
            "--profile",
            "p.yaml",
            "--pass",
            "Approved",
            "--warn",
            "Review",
            "--fail",
            "Rejected",
            "--report",
            "Reports",
            "--quick",
        ])
        .unwrap();
        let Command::Watch {
            input,
            profile,
            pass,
            warn,
            fail,
            report,
            quick,
        } = cli.command
        else {
            panic!("expected watch");
        };
        assert_eq!(input, PathBuf::from("Incoming"));
        assert_eq!(profile.as_deref(), Some(std::path::Path::new("p.yaml")));
        assert_eq!(pass, PathBuf::from("Approved"));
        assert_eq!(warn, PathBuf::from("Review"));
        assert_eq!(fail, PathBuf::from("Rejected"));
        assert_eq!(report, Some(PathBuf::from("Reports")));
        assert!(quick);
    }

    #[test]
    fn missing_subcommand_is_an_error() {
        assert!(Cli::try_parse_from(["tpt-media-qc"]).is_err());
    }
}
