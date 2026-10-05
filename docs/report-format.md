# Report Format

Reports are generated from an **immutable QC result set** (spec §14) and are
the product's audit artifact: they must prove *why* an asset passed or failed
(spec §28, Level 5 — Explainable QC).

The report model lives in
[`tpt-app-media-qc-model::report`](../crates/tpt-app-media-qc-model/src/report.rs);
assembly and rendering live in
[`tpt-app-media-qc-report`](../crates/tpt-app-media-qc-report/).

## 1. Integrity fields (spec §14.1)

Every report carries these five fields, making it auditable and reproducible:

| Field | Source |
|-------|--------|
| `Asset SHA-256` | content-based fingerprint of the asset |
| `Profile SHA-256` | hash of the canonical profile representation |
| `Application version` | from `Cargo.toml` |
| `Ruleset version` | built-in rule catalogue version (`core::config::RULESET_VERSION`) |
| `Analysis ID` | UUID identifying this specific analysis |

Reproducibility: the same asset fingerprint + profile + application/ruleset
version reproduce the same result set (spec §19). Passing a fixed `AnalysisId`
to `build_report` reproduces byte-for-byte identical reports.

## 2. JSON

The canonical report is JSON — the `Report` struct serialized directly. The CLI
writes it with `--json`; `batch --out` writes one JSON report per asset. This
is also the format machines consume via the local API (spec §17).

Example:

```json
{
  "analysis_id": "...",
  "created_at": "...",
  "app": { "name": "TPT Media QC", "version": "0.1.0", "ruleset_version": "0.1.0" },
  "integrity": {
    "asset_sha256": "ab...",
    "profile_sha256": "cd...",
    "application_version": "0.1.0",
    "ruleset_version": "0.1.0",
    "analysis_id": "..."
  },
  "profile": { "name": "generic", "version": 1, "sha256": "cd..." },
  "host": { "os": "windows", "arch": "x86_64", "cpu_count": 8 },
  "asset_path": "episode-01.mov",
  "asset_size_bytes": 123456,
  "asset_duration_ms": 1223400,
  "asset_resolution": "1920x1080",
  "findings": [
    {
      "rule_id": "container.readable",
      "status": "pass",
      "severity": "info",
      "message": "...",
      "measured": null,
      "expected": null,
      "stream_id": null,
      "time_range": null,
      "frame_range": null,
      "evidence": [],
      "confidence": null
    }
  ],
  "verdict": "pass",
  "operator_notes": ""
}
```

## 3. HTML

A self-contained human-readable report for email/inspection. Rendered by the
report crate's `render_html`; with `WriteHtmlOptions { embed_json: true }` the
full JSON report is embedded in the page so a browser viewer can be built
later. The CLI writes it with `--html`.

### Templates

`render_html`, `render_pdf` and `write_csv` take a `ReportTemplate` that
selects which parts of the report a human-facing rendering shows. The report
itself, its verdict and its integrity fields never change, and JSON is always
complete.

| Template | Contents |
|----------|----------|
| `detailed` (default) | Header, integrity fields and every finding |
| `summary` | Header and only findings that need attention (not `pass`) |
| `executive` | Verdict, counts and a per-rule "Issues by rule" roll-up (worst status, count, message of the worst finding); no per-finding table |
| `audit` | `detailed` plus the roll-up and the full integrity and host block (asset and full profile hashes, application and ruleset versions, OS/architecture/CPU count) |

CSV is row data, so `summary` and `executive` drop passing rows and
`detailed` and `audit` keep every row. The CLI selects a template with
`check --report-template <name>`; the desktop inspector has a template picker
next to the export buttons.

## 4. CSV

Findings flattened to rows (`rule_id`, `status`, `severity`, message, stream,
time range, measured, expected) for spreadsheet work. The CLI writes it with
`--csv`.

## 5. PDF

Renders the same immutable `Report` (spec §14) as an A4 document: header with
the five integrity fields, a findings table with word-wrapped cells, and
automatic pagination. Text is laid out with approximated Helvetica metrics and
non-Latin-1 characters are replaced; the CLI writes it with `--pdf`. Fonts are
the base-14 standard fonts, so no font files need to be bundled.

## 6. Report contents (spec §14)

All formats carry: application version, profile name/version, profile hash,
input fingerprint, analysis timestamp, host information, rule versions, all
findings, the final verdict, evidence, and operator notes. `operator_notes`
is a free-text field on the `Report` model, editable at review time.

## 7. CLI usage

```sh
tpt-media-qc check --profile client-a.yaml --json r.json --html r.html --csv r.csv episode-01.mov
tpt-media-qc check --report-template executive --pdf summary.pdf episode-01.mov
tpt-media-qc batch --profile client-a.yaml --input ./incoming --output ./reports
```

Machine-readable CLI output stays stable per spec §16 (exit codes `0`–`6`).