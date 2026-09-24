//! PDF report rendering (spec § 14).
//!
//! Produces an A4 report with the header/metadata block, a findings table and
//! the five integrity fields. Text is laid out with simple word-wrapping
//! (Helvetica metrics approximated) and flows across pages when findings
//! exceed one page.

use std::path::Path;

use lopdf::content::{Content, Operation};
use lopdf::{dictionary, Document, Object, Stream};
use tpt_app_media_qc_core::error::Result;
use tpt_app_media_qc_model::report::Report;

const PAGE_WIDTH: f64 = 595.0;
const PAGE_HEIGHT: f64 = 842.0;
const MARGIN: f64 = 48.0;
const LINE_HEIGHT: f64 = 13.0;
const BODY_SIZE: f64 = 9.0;
const SMALL_SIZE: f64 = 8.0;

/// Rendering options for the PDF exporter.
#[derive(Clone, Copy, Debug, Default)]
pub struct WritePdfOptions {}

/// Render a readable PDF report.
pub fn render_pdf(path: &Path, report: &Report, _options: WritePdfOptions) -> Result<()> {
    let mut doc = Document::with_version("1.5");
    let pages_id = doc.new_object_id();

    let regular = doc.add_object(dictionary! {
        "Type" => "Font",
        "Subtype" => "Type1",
        "BaseFont" => "Helvetica",
    });
    let bold = doc.add_object(dictionary! {
        "Type" => "Font",
        "Subtype" => "Type1",
        "BaseFont" => "Helvetica-Bold",
    });
    let resources_id = doc.add_object(dictionary! {
        "Font" => dictionary! {
            "F1" => regular,
            "F2" => bold,
        },
    });

    let mut layout = Layout::new();
    build_document(&mut layout, report);
    let pages = layout.finish();

    let mut kids = Vec::new();
    for page in pages {
        let content_id = doc.add_object(Stream::new(dictionary! {}, page.encode().unwrap()));
        let page_id = doc.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "Contents" => content_id,
            "Resources" => resources_id,
            "MediaBox" => vec![0.into(), 0.into(), PAGE_WIDTH.into(), PAGE_HEIGHT.into()],
        });
        kids.push(Object::Reference(page_id));
    }
    if kids.is_empty() {
        // A report always emits at least the header page.
        unreachable!("layout always produces a page")
    }
    let count = kids.len() as u32;

    doc.objects.insert(
        pages_id,
        Object::Dictionary(dictionary! {
            "Type" => "Pages",
            "Kids" => kids,
            "Count" => count,
        }),
    );
    let catalog_id = doc.add_object(dictionary! {
        "Type" => "Catalog",
        "Pages" => pages_id,
    });
    doc.trailer.set("Root", catalog_id);
    doc.compress();
    doc.save(path)?;
    Ok(())
}

enum Font {
    Regular,
    Bold,
}

/// Text line with absolute placement, ready to emit into a content stream.
struct Placed {
    x: f64,
    y_top: f64,
    font: Font,
    size: f64,
    text: String,
}

/// Accumulates placed text and splits it across A4 pages.
struct Layout {
    lines: Vec<Placed>,
    pages: Vec<Content>,
    cursor: f64,
}

impl Layout {
    fn new() -> Self {
        Layout {
            lines: Vec::new(),
            pages: Vec::new(),
            cursor: MARGIN,
        }
    }

    fn flush_if_needed(&mut self, lines: usize) {
        let blocked = lines as f64 * LINE_HEIGHT;
        if self.cursor + blocked > PAGE_HEIGHT - MARGIN {
            let page = self.take_current();
            self.pages.push(page);
        }
    }

    fn take_current(&mut self) -> Content {
        let lines = std::mem::take(&mut self.lines);
        let mut ops = Vec::new();
        for l in lines {
            let y = PAGE_HEIGHT - l.y_top;
            let font = match l.font {
                Font::Regular => "F1",
                Font::Bold => "F2",
            };
            ops.push(Operation::new("BT", vec![]));
            ops.push(Operation::new(
                "Tf",
                vec![
                    Object::Name(font.as_bytes().to_vec()),
                    Object::Real(l.size as f32),
                ],
            ));
            ops.push(Operation::new(
                "Tm",
                vec![
                    Object::Real(1.0),
                    Object::Real(0.0),
                    Object::Real(0.0),
                    Object::Real(1.0),
                    Object::Real(l.x as f32),
                    Object::Real(y as f32),
                ],
            ));
            ops.push(Operation::new(
                "Tj",
                vec![Object::string_literal(l.text.as_str())],
            ));
            ops.push(Operation::new("ET", vec![]));
        }
        Content { operations: ops }
    }

    /// Place a single line; advances the cursor by one line height.
    fn text(&mut self, x: f64, font: Font, size: f64, s: &str) {
        self.flush_if_needed(1);
        let y_top = self.cursor + size;
        let text = sanitize(s);
        self.lines.push(Placed {
            x,
            y_top,
            font,
            size,
            text,
        });
        self.cursor += LINE_HEIGHT;
    }

    /// Gutter between the section header and the table body.
    fn gap(&mut self, n: f64) {
        self.cursor += n;
    }

    fn finish(mut self) -> Vec<Content> {
        if !self.lines.is_empty() {
            let page = self.take_current();
            self.pages.push(page);
        }
        if self.pages.is_empty() {
            // Guaranteed by build_document, but keep the return type honest.
            unreachable!("pdf layout produced no pages")
        }
        self.pages
    }
}

/// Column layout for the findings table.
struct Col {
    x: f64,
    width: f64,
}

impl Col {
    fn new(x: f64, width: f64) -> Self {
        Col { x, width }
    }
}

fn build_document(layout: &mut Layout, report: &Report) {
    let counts = report.status_counts();

    layout.text(MARGIN, Font::Bold, 15.0, "TPT Media QC — QC Report");
    layout.text(
        MARGIN,
        Font::Regular,
        BODY_SIZE,
        &format!("Asset: {}", report.asset_path),
    );
    layout.text(
        MARGIN,
        Font::Bold,
        11.0,
        &format!(
            "Verdict: {}   (pass {}, warn {}, fail {}, inconclusive {})",
            report.verdict.as_str().to_uppercase(),
            counts.pass,
            counts.warn,
            counts.fail,
            counts.inconclusive,
        ),
    );
    layout.gap(4.0);

    layout.text(MARGIN, Font::Bold, BODY_SIZE, "Integrity");
    layout.text(
        MARGIN,
        Font::Regular,
        SMALL_SIZE,
        &format!("Analysis ID:  {}", report.analysis_id.0),
    );
    layout.text(
        MARGIN,
        Font::Regular,
        SMALL_SIZE,
        &format!("Asset SHA-256: {}", report.integrity.asset_sha256),
    );
    layout.text(
        MARGIN,
        Font::Regular,
        SMALL_SIZE,
        &format!("Profile SHA-256: {}", report.integrity.profile_sha256),
    );
    layout.text(
        MARGIN,
        Font::Regular,
        SMALL_SIZE,
        &format!(
            "App {} {} (ruleset {}) — created {}",
            report.app.name,
            report.app.version,
            report.app.ruleset_version,
            report.created_at.to_rfc3339()
        ),
    );
    layout.gap(2.0);

    layout.text(
        MARGIN,
        Font::Regular,
        SMALL_SIZE,
        &format!(
            "Profile: {} v{} — host {} ({}) — {} bytes",
            report.profile.name,
            report.profile.version,
            report.host.os,
            report.host.arch,
            report.asset_size_bytes
        ),
    );

    layout.gap(6.0);
    layout.text(MARGIN, Font::Bold, 12.0, "Findings");
    layout.gap(2.0);

    if report.findings.is_empty() {
        layout.text(MARGIN, Font::Regular, BODY_SIZE, "No findings.");
        return;
    }

    let cols = columns();
    table_header(layout, &cols);
    for f in &report.findings {
        let stream = f.stream_idx.map(|s| s.to_string()).unwrap_or_default();
        let time = f
            .time_range
            .map(|r| format!("{}–{} ms", r.start_ms, r.end_ms))
            .unwrap_or_default();
        let measured = f
            .measured
            .as_ref()
            .map(|v| v.to_string())
            .unwrap_or_default();
        let expected = f
            .expected
            .as_ref()
            .map(|v| v.to_string())
            .unwrap_or_default();

        let cells: Vec<Vec<String>> = vec![
            wrapped(f.rule_id.as_str(), &cols[0]),
            vec![f.status.as_str().to_string()],
            vec![f.severity.as_str().to_string()],
            vec![stream],
            vec![time],
            wrapped(&measured, &cols[5]),
            wrapped(&expected, &cols[6]),
            wrapped(&f.message, &cols[7]),
        ];
        let rows_used = cells.iter().map(|c| c.len()).max().unwrap_or(1);
        layout.flush_if_needed(rows_used);
        for row in 0..rows_used {
            for (i, cell) in cells.iter().enumerate() {
                let text = cell.get(row).map(String::as_str).unwrap_or("");
                if text.is_empty() {
                    continue;
                }
                layout.text(cols[i].x, Font::Regular, BODY_SIZE, text);
            }
        }
    }
}

fn table_header(layout: &mut Layout, cols: &[Col]) {
    layout.flush_if_needed(1);
    for (i, header) in [
        "Rule", "Status", "Sev", "S", "Time", "Measured", "Expected", "Message",
    ]
    .iter()
    .enumerate()
    {
        layout.text(cols[i].x, Font::Bold, BODY_SIZE, header);
    }
}

fn columns() -> Vec<Col> {
    let mut x = MARGIN;
    let mut cols = Vec::new();
    let widths = [
        120.0, // Rule
        55.0,  // Status
        40.0,  // Severity
        28.0,  // Stream
        70.0,  // Time
        90.0,  // Measured
        90.0,  // Expected
               // Message takes the remaining width.
    ];
    for w in widths {
        cols.push(Col::new(x, w));
        x += w;
    }
    cols.push(Col::new(x, PAGE_WIDTH - MARGIN - x));
    cols
}

/// Wrap text to a column, capping the number of lines.
fn wrapped(s: &str, col: &Col) -> Vec<String> {
    let mut out = Vec::new();
    let max_chars = ((col.width - 6.0) / (0.55 * BODY_SIZE)).floor().max(4.0) as usize;
    for line in wrap_text(s, max_chars) {
        out.push(line);
        if out.len() >= 4 {
            break;
        }
    }
    if out.is_empty() {
        out.push(String::new());
    }
    out
}

/// Simple greedy word-wrapping at an approximate character budget.
fn wrap_text(s: &str, max_chars: usize) -> Vec<String> {
    let max_chars = max_chars.max(1);
    let mut out = Vec::new();
    let mut line = String::new();
    for word in s.split_whitespace() {
        if word.len() > max_chars {
            if !line.is_empty() {
                out.push(std::mem::take(&mut line));
            }
            let mut rest = word;
            while rest.len() > max_chars {
                let (head, tail) = rest.split_at(max_chars);
                out.push(head.to_string());
                rest = tail;
            }
            line = rest.to_string();
            continue;
        }
        if line.is_empty() {
            line.push_str(word);
        } else if line.len() + 1 + word.len() <= max_chars {
            line.push(' ');
            line.push_str(word);
        } else {
            out.push(std::mem::take(&mut line));
            line.push_str(word);
        }
    }
    if !line.is_empty() {
        out.push(line);
    }
    out
}

/// Make text safe for PDFDocEncoding: escape PDF string characters and map
/// anything outside Latin-1 to `?`.
fn sanitize(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        let c = ch as u32;
        if c == 0x28 || c == 0x29 || c == 0x5C {
            out.push('\\');
            out.push(char::from_u32(c).unwrap());
        } else if c < 0x20 {
            out.push('?');
        } else if c <= 0xFF {
            out.push(char::from_u32(c).unwrap());
        } else {
            out.push('?');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use tpt_app_media_qc_model::asset::{Asset, AssetFingerprint};
    use tpt_app_media_qc_model::report::AnalysisId;

    #[test]
    fn wrapping_words_respect_budget() {
        let lines = wrap_text("aa bb cc dd", 5);
        assert!(lines.iter().all(|l| l.len() <= 5));
        assert!(!lines.is_empty());
    }

    #[test]
    fn wraps_long_words() {
        let lines = wrap_text(&"x".repeat(60), 20);
        assert_eq!(lines.len(), 3);
        assert!(lines.iter().all(|l| l.len() <= 20));
    }

    #[test]
    fn sanitize_escapes_and_maps_non_latin1() {
        assert_eq!(sanitize("a(b)\\c — é"), "a\\(b\\)\\\\c ? é");
    }

    #[test]
    fn pdf_renders_valid_file() {
        let dir = tempfile::tempdir().unwrap();
        let path: PathBuf = dir.path().join("report.pdf");
        let asset = Asset {
            id: Default::default(),
            path: "x.mp4".into(),
            fingerprint: AssetFingerprint {
                sha256: "ab".repeat(32),
                size_bytes: 1,
            },
            size_bytes: 1,
            modified_time: None,
            duration: None,
            streams: vec![],
        };
        let profile = tpt_app_media_qc_profile::model::Profile::default();
        let engine = tpt_app_media_qc_pipeline::QcEngine::new(
            std::sync::Arc::new(profile.clone()),
            tpt_app_media_qc_pipeline::arc(tpt_app_media_qc_pipeline::NoopInspector),
        );
        let run = engine.check_metadata_only(&asset).unwrap();
        let report = crate::build_report(&run, AnalysisId::default(), &profile);

        render_pdf(&path, &report, WritePdfOptions {}).unwrap();
        let bytes = fs::read(&path).unwrap();
        assert!(bytes.starts_with(b"%PDF-"), "must start with PDF header");
        assert!(
            bytes.windows(5).any(|w| w == b"%%EOF"),
            "must end with EOF marker"
        );

        let reloaded = Document::load(&path).unwrap();
        assert!(
            !reloaded.get_pages().is_empty(),
            "must contain at least one page"
        );
    }
}
