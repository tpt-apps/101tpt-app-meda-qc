# Plugin SDK and third-party rules

Third-party rules extend the rule catalogue without rebuilding the product
(spec §7, §27). A plugin is a **separate program**: for each rule use, the
host starts it, writes one JSON request on its standard input, and reads one
JSON response from its standard output. That keeps a faulty plugin from
taking the host down, lets plugins be written in any language, and needs no
dynamic loading or `unsafe` code.

## Using a plugin

1. Install it as a folder inside a plugin directory you control:

   ```
   plugins/
     acme/
       plugin.json
       bin/acme-qc.exe
   ```

2. Point the tool at the directory with `--plugin-dir plugins` or the
   `TPT_MEDIA_QC_PLUGIN_DIR` environment variable (the desktop app uses the
   variable). The directory is only read when the profile uses plugin rules.
3. Name the rule in a profile:

   ```yaml
   rules:
     plugins:
       - rule: acme.min_video_bitrate
         severity: error              # default error; the profile decides severity
         config: { min_bps: 5000000 } # passed to the plugin unchanged
   ```

`tpt-media-qc list-rules -p profile.yaml` lists installed plugins and the
plugin rules a profile uses. A profile naming a rule that is not installed
fails with exit code 5 instead of silently skipping it.

## Security model

Installing a plugin is the trust decision. What the host guarantees:

* **A profile cannot choose what runs.** Profiles name rule ids only. Which
  program provides a rule comes from the manifest in *your* plugin directory,
  so a profile received from someone else cannot start an arbitrary program.
* **Contained execution.** Each run has a timeout (manifest `timeout_ms`,
  default 10 s, at most 300 s; the process is killed on expiry), a 4 MiB
  response limit, at most 1000 findings, the plugin directory as working
  directory, and a cleared environment. Only `PATH` (plus `SystemRoot` and
  `windir` on Windows) and `TPT_MEDIA_QC_PLUGIN_PROTOCOL` are passed, so
  credentials and tokens in the host's environment are not leaked.
* **Failures are findings.** A crash, non-zero exit, timeout, oversized or
  malformed response, or protocol mismatch becomes an `Inconclusive` finding
  that names the plugin and the reason. It never aborts the run or the batch.
* **The host owns identity.** A plugin cannot change its rule id or severity
  (severity comes from the profile) and control characters are stripped from
  its messages.
* **Measurements, not media.** Plugins receive the asset description and the
  inspection (spec §3.5), never media bytes.
* **No caching.** Plugin results are never cached, because a plugin can change
  without the profile changing.

What it does **not** do: it does not sandbox the plugin. A plugin can use the
network and any file the user can read, so install only plugins you trust.

## `plugin.json`

```json
{
  "schema": 1,
  "id": "acme",
  "name": "ACME delivery checks",
  "version": "1.0.0",
  "protocol": 1,
  "command": "bin/acme-qc.exe",
  "args": [],
  "timeout_ms": 10000,
  "rules": [
    { "id": "acme.min_video_bitrate", "description": "...", "streams": ["video"] },
    { "id": "acme.slow_check", "decode": "stream" }
  ]
}
```

* `id`: lowercase letters, digits, `_`, `-`. May not be `container`, `video`,
  `audio`, `subtitle`, `voice` or `custom`. Every rule id must be
  `<id>.<name>`.
* `command`: either a path inside the plugin folder (no `..`, no absolute
  path, must resolve to a file inside the folder) or a bare program name such
  as `python` found on `PATH`. `args` are passed as plain arguments, never
  through a shell.
* `streams` (optional): stream kinds (`video`, `audio`, `subtitle`) the rule
  needs. A file without one gets a pass without starting the plugin.
* `decode` (optional, `none` or `stream`): whether the rule needs decode-pass
  measurements. Rules always run; with `stream`, a quick (metadata-only) scan
  hands the plugin an inspection without them, so it should report
  `inconclusive`.
* Invalid manifests are skipped with a warning; two plugins providing the same
  rule is an error.

## Protocol (version 1)

Request, one JSON document on stdin:

```json
{
  "protocol": 1,
  "rule": "acme.min_video_bitrate",
  "config": { "min_bps": 5000000 },
  "asset": { "...": "Asset, as in the JSON report" },
  "inspection": { "...": "Inspection: container, video, audio, subtitle, voice" }
}
```

Response, one JSON document on stdout, exit status 0:

```json
{
  "protocol": 1,
  "findings": [
    {
      "status": "fail",
      "message": "video bitrate 3400000 b/s is below 5000000 b/s",
      "measured": { "UInt": 3400000 },
      "expected": { "UInt": 5000000 },
      "stream": 0,
      "time_range": { "start_ms": 1000, "end_ms": 4000 }
    }
  ]
}
```

* `status`: `pass`, `warn`, `fail` or `inconclusive`. An empty `findings`
  list is a pass. Report `inconclusive` when a measurement you need is missing;
  never guess (spec §3.4).
* `measured` / `expected` use the report's `Value` encoding (`UInt`, `Int`,
  `Float`, `Text`, `Bool`, `Ratio`, `DurationMs`, `Bytes`, `Rational`).
  `stream` and `time_range` are optional and point the viewer at the evidence.
* Anything written to stderr is shown (first 2 KiB) only when the plugin fails.

## Writing a plugin in Rust

Depend on `tpt-app-media-qc-plugin-sdk`:

```rust
use tpt_app_media_qc_plugin_sdk::{serve, PluginFinding, PluginRequest};

fn main() {
    std::process::exit(serve(|request: &PluginRequest| {
        let min = request.config["min_bps"].as_u64().unwrap_or(0);
        request.asset.streams.iter()
            .filter(|s| s.bitrate.is_some_and(|b| b < min))
            .map(|s| PluginFinding::fail(format!("stream {} is below {min} b/s", s.index)))
            .collect()
    }));
}
```

A complete reference plugin with two rules is
[`qc-example-plugin`](../crates/tpt-app-media-qc-test/src/bin/qc-example-plugin.rs);
the integration tests in
[`tests/plugins.rs`](../crates/tpt-app-media-qc-test/tests/plugins.rs) run it,
and a deliberately misbehaving plugin, through the real process boundary.
Plugins in other languages follow the JSON shapes above.

## Out of scope

Facility management, distributed local workers and LAN processing are
Facility-tier features that the spec says not to build until customers need
them (spec §29, §32). They are not implemented.
