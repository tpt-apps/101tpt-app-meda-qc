//! Report generation and exports ([spec § 14]).
//!
//! The report crate turns a pipeline [`QcRun`] into an immutable [`Report`]
//! (JSON) plus CSV and HTML renderings. Reports are reproducible: every report
//! embeds the five integrity fields required by [spec § 14.1].

mod builder;
mod csv;
mod html;
mod json;
mod pdf;

pub use builder::{build_report, report_host_info, write_json_report};
pub use csv::{write_csv, WriteCsvOptions};
pub use html::{render_html, WriteHtmlOptions};
pub use pdf::{render_pdf, WritePdfOptions};