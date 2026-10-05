# tpt-app-media-qc-plugin

Plugin host for [TPT Media QC](../../README.md) (spec §27): discovers
installed plugins in an operator-controlled directory, validates their
manifests, and runs their rules out of process with a timeout, a response size
limit and a minimal environment. Writing plugins is covered by the
[`plugin SDK`](../tpt-app-media-qc-plugin-sdk) and
[`docs/plugin-sdk.md`](../../docs/plugin-sdk.md).
