use tpt_app_media_qc_profile::{per_rule_config_hash, profile_sha256, CustomOp, Profile};

fn parse(rules: &str) -> Result<Profile, String> {
    Profile::from_yaml(&format!("name: t\nrules:\n  custom:\n{rules}")).map_err(|e| e.to_string())
}

#[test]
fn parses_a_full_rule() {
    let p = parse(
        "    - id: custom.min_bitrate\n      scope: video\n      metric: bitrate\n      op: '>='\n      value: 5000000\n      severity: warning\n      message: too low\n",
    )
    .unwrap();
    let r = &p.rules.custom[0];
    assert_eq!(r.op, CustomOp::Ge);
    assert_eq!(r.message.as_deref(), Some("too low"));
}

#[test]
fn rejects_bad_rules() {
    for (rules, needle) in [
        ("    - {id: bitrate, scope: video, metric: bitrate, op: '>', value: 1}", "custom."),
        ("    - {id: custom.a, scope: video, metric: nope, op: '>', value: 1}", "unknown video metric"),
        ("    - {id: custom.a, scope: video, metric: codec, op: '>', value: x}", "numeric metric"),
        ("    - {id: custom.a, scope: video, metric: bitrate, op: '>', value: x}", "numeric but"),
        ("    - {id: custom.a, scope: video, metric: codec, op: '==', value: 264}", "quote it"),
        ("    - {id: custom.a, scope: video, metric: bitrate, op: '>', value: [1, 2]}", "exactly one"),
        ("    - {id: custom.a, scope: video, metric: bitrate, op: '>', value: 1, bogus: 1}", "unknown key"),
        ("    - {id: custom.a, scope: container, streams: all, metric: size_bytes, op: '>', value: 1}", "does not apply"),
        ("    - {id: custom.a, scope: video, metric: bitrate, op: '>', value: 1, tolerance: 1}", "tolerance"),
        (
            "    - {id: custom.a, scope: video, metric: bitrate, op: '>', value: 1}\n    - {id: custom.a, scope: audio, metric: channels, op: '==', value: 2}",
            "duplicate",
        ),
    ] {
        let err = parse(rules).unwrap_err();
        assert!(err.contains(needle), "{rules}: {err}");
    }
}

#[test]
fn per_rule_hash_tracks_only_its_own_rule() {
    let a = parse("    - {id: custom.a, scope: video, metric: bitrate, op: '>', value: 1}\n    - {id: custom.b, scope: audio, metric: channels, op: '==', value: 2}").unwrap();
    let b = parse("    - {id: custom.a, scope: video, metric: bitrate, op: '>', value: 9}\n    - {id: custom.b, scope: audio, metric: channels, op: '==', value: 2}").unwrap();
    assert_ne!(
        per_rule_config_hash(&a, "custom.a"),
        per_rule_config_hash(&b, "custom.a")
    );
    assert_eq!(
        per_rule_config_hash(&a, "custom.b"),
        per_rule_config_hash(&b, "custom.b")
    );
    assert!(per_rule_config_hash(&a, "custom.missing").is_none());
    assert_ne!(profile_sha256(&a), profile_sha256(&b));
}

#[test]
fn profiles_without_custom_rules_keep_their_hash_shape() {
    let p = Profile::from_yaml("name: t\n").unwrap();
    assert!(!tpt_app_media_qc_profile::hash::canonical_yaml(&p).contains("custom"));
}
