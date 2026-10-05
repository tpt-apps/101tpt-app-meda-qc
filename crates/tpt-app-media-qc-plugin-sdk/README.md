# tpt-app-media-qc-plugin-sdk

SDK for writing third-party rules for [TPT Media QC](../../README.md)
(spec §27). A plugin is a separate program that reads one JSON request on
standard input and writes one JSON response on standard output. This crate
provides the protocol types and a `serve` helper; plugins can also be written
in any other language by following the same JSON shapes.

See [`docs/plugin-sdk.md`](../../docs/plugin-sdk.md) for the manifest format,
installation, security model and a worked example.
