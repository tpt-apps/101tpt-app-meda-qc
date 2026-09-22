# Integration tests

End-to-end tests spanning real binaries and the engine boundary (spec §24).
These are the CLI-level tests that prove the pieces fit together:

- CLI `check` / `batch` / `info` / `list-rules` against real files with
  `ffprobe` present (skipped when `ffprobe` is unavailable — the metadata
  probe is an environment dependency, not a test dependency).
- Engine + report round-trips: inspect → QC → report → parse the report back
  and verify the five integrity fields (spec §14.1).
- Batch failure isolation (spec §21): a corrupt asset in a batch must not
  terminate the batch; the batch completes and reports the per-asset error.
- Profile loading from the bundled [`profiles/`](../../profiles/) tree and
  CLI exit-code mapping (spec §16).
- Watch-folder routing (to be added with the watch-folder feature, spec §13).

An integration test that needs fixtures lives in
[`../fixtures/`](../fixtures/) with its expectation in
[`../golden/`](../golden/).