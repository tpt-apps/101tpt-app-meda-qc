use tpt_app_media_qc_model::severity::Severity;
use tpt_app_media_qc_profile::{per_rule_config_hash, valid_plugin_rule_id, Profile};

fn parse(plugins: &str) -> Result<Profile, String> {
    Profile::from_yaml(&format!("name: t\nrules:\n  plugins:\n{plugins}"))
        .map_err(|e| e.to_string())
}

#[test]
fn parses_plugin_rules_with_config_and_defaults() {
    let p = parse(
        "    - rule: acme.min_bitrate\n      severity: warning\n      config: {min_bps: 5000000, tags: [a, b]}\n    - rule: acme.other\n",
    )
    .unwrap();
    let first = &p.rules.plugins[0];
    assert_eq!(first.rule, "acme.min_bitrate");
    assert_eq!(first.severity, Severity::Warning);
    assert_eq!(first.config["min_bps"], 5_000_000);
    assert_eq!(first.config["tags"][1], "b");

    let second = &p.rules.plugins[1];
    assert_eq!(second.severity, Severity::Error);
    assert!(second.config.is_null());
}

#[test]
fn rejects_bad_plugin_rules() {
    for (plugins, needle) in [
        ("    - {rule: video.black_frames}", "built-in prefix"),
        ("    - {rule: custom.mine}", "built-in prefix"),
        ("    - {rule: nodot}", "must look like"),
        ("    - {rule: Acme.Upper}", "must look like"),
        ("    - {rule: acme.x, command: /bin/sh}", "unknown key"),
        ("    - {config: {}}", "rule"),
        ("    - {rule: acme.x}\n    - {rule: acme.x}", "twice"),
        ("    acme.x: {}", "expected"),
    ] {
        let err = parse(plugins).unwrap_err();
        assert!(err.contains(needle), "{plugins}: {err}");
    }
}

#[test]
fn a_profile_cannot_name_a_program_to_run() {
    // Executable choice belongs to the operator's plugin directory.
    for key in ["command", "path", "exe", "args"] {
        let err = parse(&format!("    - {{rule: acme.x, {key}: whatever}}")).unwrap_err();
        assert!(err.contains("unknown key"), "{key}: {err}");
    }
}

#[test]
fn id_validation_matches_the_documented_shape() {
    for ok in ["acme.min_bitrate", "a-b.c_d", "acme.group.rule", "x1.y2"] {
        assert!(valid_plugin_rule_id(ok), "{ok}");
    }
    for bad in [
        "", "acme", ".x", "acme.", "audio.x", "voice.x", "Acme.x", "a b.c",
    ] {
        assert!(!valid_plugin_rule_id(bad), "{bad}");
    }
}

#[test]
fn plugin_rules_are_never_cached() {
    // A plugin can change between runs without the profile changing, so no
    // configuration hash (and therefore no cache entry) exists for them.
    let p = parse("    - {rule: acme.x, config: {n: 1}}").unwrap();
    assert!(per_rule_config_hash(&p, "acme.x").is_none());
}
