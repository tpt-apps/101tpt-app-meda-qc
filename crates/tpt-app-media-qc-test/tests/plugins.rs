//! Plugin host integration tests (spec § 27): real child processes, real
//! JSON over stdin/stdout, through the QC engine.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tpt_app_media_qc_model::severity::{Severity, VerdictDecision};
use tpt_app_media_qc_pipeline::{arc, InspectionLevel, QcEngine, QcRun};
use tpt_app_media_qc_plugin::{PluginError, PluginRegistry};
use tpt_app_media_qc_profile::Profile;
use tpt_app_media_qc_test::{FixtureInspector, MediaFixture};

const EXAMPLE: &str = env!("CARGO_BIN_EXE_qc-example-plugin");
const BAD: &str = env!("CARGO_BIN_EXE_qc-test-plugin");

/// Install `exe` as plugin `id` under `root`, providing `rules`
/// (`(rule id, extra manifest fields)`).
fn install(root: &Path, id: &str, exe: &str, rules: &[(&str, &str)], timeout_ms: u64) -> PathBuf {
    let dir = root.join(id);
    std::fs::create_dir_all(dir.join("bin")).unwrap();
    let program = format!("bin/plugin{}", std::env::consts::EXE_SUFFIX);
    std::fs::copy(exe, dir.join(&program)).unwrap();
    let rules_json: Vec<String> = rules
        .iter()
        .map(|(rule, extra)| format!(r#"{{"id":"{rule}"{extra}}}"#))
        .collect();
    std::fs::write(
        dir.join("plugin.json"),
        format!(
            r#"{{"schema":1,"id":"{id}","name":"{id} plugin","version":"1.2.3","protocol":1,
                "command":"{program}","timeout_ms":{timeout_ms},"rules":[{}]}}"#,
            rules_json.join(",")
        ),
    )
    .unwrap();
    dir
}

fn profile(plugins_yaml: &str) -> Profile {
    Profile::from_yaml(&format!(
        "name: plugin-test\nrules:\n  plugins:\n{plugins_yaml}"
    ))
    .expect("profile parses")
}

fn run(registry: &PluginRegistry, profile: &Profile) -> QcRun {
    let fixture = MediaFixture::load_named("clean-master").expect("fixture");
    let engine = QcEngine::with_extra_rules(
        Arc::new(profile.clone()),
        arc(FixtureInspector::new(&fixture)),
        registry.build_rules(profile).expect("rules build"),
    );
    engine
        .check(&fixture.asset, InspectionLevel::MetadataOnly)
        .expect("run")
}

fn only_finding(run: &QcRun, rule: &str) -> tpt_app_media_qc_model::finding::QcFinding {
    let found: Vec<_> = run
        .findings
        .iter()
        .filter(|f| f.rule_id.as_str() == rule)
        .collect();
    assert_eq!(found.len(), 1, "{rule}: {:?}", run.findings);
    found[0].clone()
}

#[test]
fn a_plugin_rule_passes_and_fails_through_the_engine() {
    let dir = tempfile::tempdir().unwrap();
    install(
        dir.path(),
        "example",
        EXAMPLE,
        &[
            ("example.min_video_bitrate", r#","streams":["video"]"#),
            ("example.max_streams", ""),
        ],
        10_000,
    );
    let registry = PluginRegistry::discover(dir.path()).unwrap();
    assert!(registry.warnings.is_empty(), "{:?}", registry.warnings);
    assert_eq!(registry.plugins()[0].rules.len(), 2);

    // The clean master's video is well above 1 Mb/s and has few streams.
    let ok = run(
        &registry,
        &profile(
            "    - {rule: example.min_video_bitrate, config: {min_bps: 1000000}}\n    - {rule: example.max_streams, config: {max: 16}}\n",
        ),
    );
    assert_eq!(ok.verdict, VerdictDecision::Pass, "{:?}", ok.findings);

    let bad = run(
        &registry,
        &profile(
            "    - {rule: example.min_video_bitrate, severity: warning, config: {min_bps: 900000000}}\n    - {rule: example.max_streams, config: {max: 1}}\n",
        ),
    );
    let f = only_finding(&bad, "example.min_video_bitrate");
    assert_eq!(f.status, VerdictDecision::Fail);
    assert_eq!(
        f.severity,
        Severity::Warning,
        "severity comes from the profile"
    );
    assert!(f.stream_idx.is_some());
    assert!(f.measured.is_some() && f.expected.is_some());
    let g = only_finding(&bad, "example.max_streams");
    assert_eq!(g.severity, Severity::Error, "default severity is error");
    assert_eq!(bad.verdict, VerdictDecision::Fail);
}

#[test]
fn every_plugin_failure_mode_becomes_an_inconclusive_finding() {
    let dir = tempfile::tempdir().unwrap();
    install(
        dir.path(),
        "bad",
        BAD,
        &[
            ("bad.crash", ""),
            ("bad.garbage", ""),
            ("bad.huge", ""),
            ("bad.hang", ""),
            ("bad.protocol", ""),
        ],
        1_000,
    );
    let registry = PluginRegistry::discover(dir.path()).unwrap();
    let p = profile(
        "    - {rule: bad.crash}\n    - {rule: bad.garbage}\n    - {rule: bad.huge}\n    - {rule: bad.hang}\n    - {rule: bad.protocol}\n",
    );
    let started = std::time::Instant::now();
    let result = run(&registry, &p);
    assert!(
        started.elapsed() < std::time::Duration::from_secs(20),
        "a hanging plugin must be stopped by its timeout"
    );

    for (rule, needle) in [
        ("bad.crash", "something went badly wrong"),
        ("bad.garbage", "invalid response"),
        ("bad.huge", "larger than"),
        ("bad.hang", "timed out"),
        ("bad.protocol", "protocol 99"),
    ] {
        let f = only_finding(&result, rule);
        assert_eq!(f.status, VerdictDecision::Inconclusive, "{rule}");
        assert!(f.message.contains(needle), "{rule}: {}", f.message);
    }
}

#[test]
fn the_host_owns_rule_id_severity_and_message_hygiene() {
    let dir = tempfile::tempdir().unwrap();
    install(dir.path(), "bad", BAD, &[("bad.noisy", "")], 5_000);
    let registry = PluginRegistry::discover(dir.path()).unwrap();
    let result = run(
        &registry,
        &profile("    - {rule: bad.noisy, severity: info}\n"),
    );
    // The plugin claimed another rule id and severity; neither is honoured.
    assert!(result
        .findings
        .iter()
        .all(|f| f.rule_id.as_str() == "bad.noisy"));
    let f = only_finding(&result, "bad.noisy");
    assert_eq!(f.severity, Severity::Info);
    assert!(!f.message.contains('\u{7}'));
}

#[test]
fn plugins_do_not_inherit_the_hosts_environment() {
    std::env::set_var("TPT_QC_TEST_SECRET", "hunter2");
    let dir = tempfile::tempdir().unwrap();
    install(dir.path(), "bad", BAD, &[("bad.env", "")], 5_000);
    let registry = PluginRegistry::discover(dir.path()).unwrap();
    let result = run(&registry, &profile("    - {rule: bad.env}\n"));
    let f = only_finding(&result, "bad.env");
    assert_eq!(f.status, VerdictDecision::Pass, "{}", f.message);
    assert!(f.message.contains("leaked=false"), "{}", f.message);
    assert!(f.message.contains("protocol_var=true"), "{}", f.message);
}

#[test]
fn rules_that_need_absent_streams_do_not_start_the_plugin() {
    let dir = tempfile::tempdir().unwrap();
    // `bad.crash` would produce a finding if it ever ran.
    install(
        dir.path(),
        "bad",
        BAD,
        &[("bad.crash", r#","streams":["subtitle"]"#)],
        5_000,
    );
    let registry = PluginRegistry::discover(dir.path()).unwrap();
    // Streams reach the rules through the fixture's inspection.
    let mut fixture = MediaFixture::load_named("clean-master").unwrap();
    fixture
        .inspection
        .streams
        .retain(|s| s.kind != tpt_app_media_qc_model::asset::StreamKind::Subtitle);
    fixture
        .asset
        .streams
        .retain(|s| s.kind != tpt_app_media_qc_model::asset::StreamKind::Subtitle);
    let p = profile("    - {rule: bad.crash}\n");
    let engine = QcEngine::with_extra_rules(
        Arc::new(p.clone()),
        arc(FixtureInspector::new(&fixture)),
        registry.build_rules(&p).unwrap(),
    );
    let result = engine
        .check(&fixture.asset, InspectionLevel::MetadataOnly)
        .unwrap();
    assert!(
        result
            .findings
            .iter()
            .all(|f| f.rule_id.as_str() != "bad.crash"),
        "{:?}",
        result.findings
    );
}

#[test]
fn a_profile_naming_an_uninstalled_rule_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let registry = PluginRegistry::discover(dir.path()).unwrap();
    let p = profile("    - {rule: acme.nothing}\n");
    match registry.build_rules(&p) {
        Err(PluginError::MissingRules(rules, _)) => assert_eq!(rules, ["acme.nothing"]),
        other => panic!("expected MissingRules, got {:?}", other.err()),
    }
    // The empty registry is fine for profiles that use no plugins.
    assert!(PluginRegistry::empty()
        .build_rules(&Profile::default())
        .unwrap()
        .is_empty());
}

#[test]
fn discovery_skips_bad_plugins_and_rejects_duplicate_rules() {
    let dir = tempfile::tempdir().unwrap();
    install(dir.path(), "good", EXAMPLE, &[("good.a", "")], 1_000);

    // Invalid manifest: skipped with a warning.
    let broken = dir.path().join("broken");
    std::fs::create_dir(&broken).unwrap();
    std::fs::write(broken.join("plugin.json"), "{ not json").unwrap();

    // Command escaping the plugin directory: skipped with a warning.
    let escape = dir.path().join("escape");
    std::fs::create_dir(&escape).unwrap();
    std::fs::write(
        escape.join("plugin.json"),
        r#"{"schema":1,"id":"escape","name":"e","version":"1","protocol":1,
            "command":"../good/bin/plugin","rules":[{"id":"escape.a"}]}"#,
    )
    .unwrap();

    let registry = PluginRegistry::discover(dir.path()).unwrap();
    assert_eq!(registry.plugins().len(), 1);
    assert_eq!(registry.warnings.len(), 2, "{:?}", registry.warnings);
    assert!(registry.provides("good.a"));
    assert!(!registry.provides("escape.a"));

    // A plugin id may appear once; a second folder with the same id is skipped.
    install(dir.path(), "zzz", EXAMPLE, &[("zzz.a", "")], 1_000);
    let manifest = std::fs::read_to_string(dir.path().join("zzz/plugin.json")).unwrap();
    std::fs::write(
        dir.path().join("zzz/plugin.json"),
        manifest.replace("\"id\":\"zzz\"", "\"id\":\"good\""),
    )
    .unwrap();
    let registry = PluginRegistry::discover(dir.path()).unwrap();
    assert_eq!(registry.plugins().len(), 1);
    assert!(registry
        .warnings
        .iter()
        .any(|w| w.contains("skipped") && w.contains("zzz")));
}

#[test]
fn a_missing_plugin_directory_is_an_error() {
    assert!(matches!(
        PluginRegistry::discover(Path::new("definitely/not/a/dir")),
        Err(PluginError::Directory(..))
    ));
}
