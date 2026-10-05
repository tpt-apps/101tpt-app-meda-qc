//! Command-line argument definitions.

use std::path::PathBuf;

use clap::{Parser, Subcommand};
use tpt_app_media_qc_report::ReportTemplate;

fn parse_template(s: &str) -> Result<ReportTemplate, String> {
    ReportTemplate::parse(s).ok_or_else(|| {
        format!(
            "unknown report template '{s}' (expected one of: {})",
            ReportTemplate::ALL.map(|t| t.as_str()).join(", ")
        )
    })
}

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

        /// Report layout for the HTML, PDF and CSV outputs: detailed
        /// (default), summary, executive or audit. JSON is always complete.
        #[arg(long, value_name = "NAME", default_value = "detailed", value_parser = parse_template)]
        report_template: ReportTemplate,

        /// Suppress per-finding output (summary + verdict only).
        #[arg(long)]
        quiet: bool,
    },

    /// QC several files (or a directory) against a profile.
    Batch {
        /// One or more files or directories to scan recursively.
        files: Vec<PathBuf>,

        /// One or more input files or directories (alternative to positional paths).
        #[arg(long = "input", value_name = "PATH", num_args = 1..)]
        input: Vec<PathBuf>,

        /// YAML profile file.
        #[arg(long, short = 'p')]
        profile: Option<PathBuf>,

        /// Metadata-only (quick) scan.
        #[arg(long)]
        quick: bool,

        /// Directory to write reports into (one JSON per asset).
        #[arg(long, visible_alias = "output")]
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

    /// Compare two media files: metadata, streams, duration, frame rate,
    /// resolution, codec, audio layout and loudness (spec §15).
    ///
    /// Exit code 0 when the files match within tolerance, 1 when only minor
    /// differences exist, 2 when any major difference exists.
    Compare {
        /// The reference file (for example `master_v1.mov`).
        left: PathBuf,

        /// The file compared against the reference.
        right: PathBuf,

        /// Metadata-only comparison; skip decode-based measurements.
        #[arg(long)]
        quick: bool,

        /// Write the comparison as JSON to this path.
        #[arg(long)]
        json: Option<PathBuf>,

        /// Duration tolerance in milliseconds.
        #[arg(long, default_value_t = 40)]
        duration_tolerance_ms: u64,

        /// Loudness tolerance in LU.
        #[arg(long, default_value_t = 0.5)]
        loudness_tolerance_lu: f64,
    },

    /// Correct loudness and DC offset of a standalone WAV/AIFF/FLAC file
    /// into a new WAV (never touches the original).
    ///
    /// Applies one linear gain to reach the target loudness, but only when the
    /// result stays under the true-peak ceiling (there is no limiter), and
    /// removes per-channel DC offset. The written file is re-measured so the
    /// before/after values are real. Targets default to the profile's
    /// `audio.loudness` and `audio.true_peak` rules.
    Correct {
        /// Audio file (WAV, AIFF/AIFC or FLAC).
        file: PathBuf,

        /// YAML profile supplying the loudness target and true-peak ceiling.
        #[arg(long, short = 'p')]
        profile: Option<PathBuf>,

        /// Output WAV path. Default: `<name>.corrected.wav` next to the input.
        #[arg(long, short = 'o')]
        out: Option<PathBuf>,

        /// Integrated loudness target in LUFS (overrides the profile).
        #[arg(long, allow_negative_numbers = true)]
        target_lufs: Option<f64>,

        /// True-peak ceiling in dBTP (overrides the profile; default -1).
        #[arg(long, allow_negative_numbers = true)]
        true_peak_db: Option<f64>,

        /// Do not remove DC offset.
        #[arg(long)]
        no_dc: bool,

        /// Output sample size: 16 or 24 bit PCM.
        #[arg(long, default_value = "24", value_parser = ["16", "24"])]
        bits: String,

        /// Show the plan without writing a file.
        #[arg(long)]
        dry_run: bool,
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
            report_template,
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
        assert_eq!(report_template, ReportTemplate::Detailed);
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
            "--report-template",
            "audit",
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
            report_template,
            quiet,
        } = cli.command
        else {
            panic!("expected check");
        };
        assert_eq!(file, PathBuf::from("x.mov"));
        assert_eq!(profile.as_deref(), Some(std::path::Path::new("p.yaml")));
        assert!(quick && quiet);
        assert_eq!(report_template, ReportTemplate::Audit);
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
    fn parses_batch_documented_input_and_output_flags() {
        let cli = Cli::try_parse_from([
            "tpt-media-qc",
            "batch",
            "--input",
            "incoming",
            "second-input",
            "--output",
            "reports",
        ])
        .unwrap();
        let Command::Batch {
            files,
            input,
            profile,
            quick,
            out,
            fail_fast,
        } = cli.command
        else {
            panic!("expected batch");
        };
        assert!(files.is_empty());
        assert_eq!(
            input,
            vec![PathBuf::from("incoming"), PathBuf::from("second-input")]
        );
        assert!(profile.is_none() && !quick && !fail_fast);
        assert_eq!(out, Some(PathBuf::from("reports")));
    }

    #[test]
    fn parses_batch_positional_paths_and_legacy_out_flag() {
        let cli = Cli::try_parse_from([
            "tpt-media-qc",
            "batch",
            "one.mp4",
            "two.mov",
            "--out",
            "reports",
        ])
        .unwrap();
        let Command::Batch {
            files, input, out, ..
        } = cli.command
        else {
            panic!("expected batch");
        };
        assert_eq!(
            files,
            vec![PathBuf::from("one.mp4"), PathBuf::from("two.mov")]
        );
        assert!(input.is_empty());
        assert_eq!(out, Some(PathBuf::from("reports")));
    }

    #[test]
    fn missing_subcommand_is_an_error() {
        assert!(Cli::try_parse_from(["tpt-media-qc"]).is_err());
    }

    #[test]
    fn rejects_unknown_report_template() {
        let err = Cli::try_parse_from([
            "tpt-media-qc",
            "check",
            "x.mp4",
            "--report-template",
            "fancy",
        ])
        .unwrap_err()
        .to_string();
        assert!(err.contains("unknown report template"), "{err}");
    }
}
