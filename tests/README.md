# Test suites

This directory holds the product test infrastructure (spec §24).

```
tests/
├── fixtures/     # controlled media fixtures (golden inputs)
├── integration/  # end-to-end CLI / engine tests
├── golden/       # expected-result manifests for fixtures (spec §24.2)
└── performance/  # benchmark harness (spec §20)
```

- **Unit tests** live beside the code they test (spec §24.1): every QC rule
  ships valid-input, invalid-input, boundary, malformed-input and
  expected-result tests.
- **Golden media tests** (spec §24.2) pair a fixture in `fixtures/` with an
  expected result in `golden/` — implemented and enforced: the runner in
  `tpt-app-media-qc-test` verifies every manifest on `cargo test --workspace`
  and asserts coverage of every production rule.
- **Property tests** (spec §24.3) cover timecode conversion, frame indexing,
  duration calculations, threshold logic, profile parsing and result
  aggregation (planned).
- **Fuzzing** (spec §24.4) targets containers, metadata, the profile parser,
  the result parser, CLI arguments, report generation and media-boundary
  handling.
- **Regression tests** (spec §24.5): every production bug produces a
  permanent regression fixture.

Fixtures are checked in only if legally clean (own content or permissively
licensed) and small. Placeholder/empty files are ignored by the runner.