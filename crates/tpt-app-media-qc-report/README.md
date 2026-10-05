# tpt-app-media-qc-report

Deterministic, auditable report generation for
[TPT Media QC](../../README.md) (spec §14).

The crate turns a pipeline `QcRun` into an immutable `Report` (JSON) plus
CSV, HTML and PDF renderings. Every report embeds the five integrity fields
(asset SHA-256, profile SHA-256, application version, ruleset version,
analysis ID), and output writes are staged in RAII-managed temporary files
before atomic replacement.

## Exports

- `build_report` / `write_json_report` — immutable JSON report (always complete).
- `write_csv` — one row per finding (`WriteCsvOptions`).
- `render_html` — readable single-page HTML (`WriteHtmlOptions`).
- `render_pdf` — paginated PDF via `lopdf` (`WritePdfOptions`).
- `ReportTemplate` (`detailed` / `summary` / `executive` / `audit`) and
  `rule_rollup` — layout selection for HTML/PDF/CSV; JSON is always complete.

## Example

```rust
let template = tpt_app_media_qc_report::ReportTemplate::parse("summary");
assert!(template.is_some());
```

See [report format](../../docs/report-format.md) for the JSON schema and
template semantics.

## Spec references

- §14 (reports), §14.1 (integrity fields).

## License

Dual-licensed MIT / Apache-2.0. See `LICENSE-MIT` and `LICENSE-APACHE` at the workspace root.
