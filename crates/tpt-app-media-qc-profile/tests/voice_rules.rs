use tpt_app_media_qc_profile::{per_rule_config_hash, Profile, SpeechExpectation};

fn parse(voice: &str) -> Result<Profile, String> {
    Profile::from_yaml(&format!("name: t\nrules:\n  voice:\n{voice}")).map_err(|e| e.to_string())
}

#[test]
fn parses_every_voice_rule() {
    let p = parse(
        "    speech: {expect: absent, min_ratio: 0.1, severity: error}\n    silence: {max_non_speech_ms: 5000}\n    speakers: {min: 1, max: 2}\n    speaker_changes: {max_per_minute: 12.5}\n    transcript: {max_wer: 0.15, max_unexpected_words: 4}\n",
    )
    .unwrap();
    let v = &p.rules.voice;
    assert_eq!(v.speech.unwrap().expect, SpeechExpectation::Absent);
    assert_eq!(v.silence.unwrap().max_non_speech_ms, 5000);
    assert_eq!(v.speakers.unwrap().max, Some(2));
    assert_eq!(v.speaker_changes.unwrap().max_per_minute, 12.5);
    assert_eq!(v.transcript.unwrap().max_unexpected_words, 4);
}

#[test]
fn speech_ratio_defaults_apply() {
    let p = parse("    speech: {expect: present}").unwrap();
    assert_eq!(p.rules.voice.speech.unwrap().min_ratio, 0.05);
}

#[test]
fn rejects_bad_voice_rules() {
    for (voice, needle) in [
        ("    speech: error", "mapping"),
        ("    speech: {expect: maybe}", "unknown expectation"),
        (
            "    speech: {expect: present, min_ratio: 2}",
            "between 0.0 and 1.0",
        ),
        ("    speech: {expect: present, bogus: 1}", "unknown key"),
        ("    speakers: {}", "at least one"),
        ("    speakers: {min: 3, max: 2}", "must not exceed"),
        ("    silence: {}", "max_non_speech_ms"),
        ("    speaker_changes: {max_per_minute: -1}", "non-negative"),
        ("    transcript: {max_unexpected_words: 1}", "max_wer"),
        ("    nonsense: {max: 1}", "unknown key"),
    ] {
        let err = parse(voice).unwrap_err();
        assert!(err.contains(needle), "{voice}: {err}");
    }
}

#[test]
fn voice_rules_have_their_own_config_hash() {
    let a = parse("    speakers: {max: 2}\n    silence: {max_non_speech_ms: 5000}").unwrap();
    let b = parse("    speakers: {max: 3}\n    silence: {max_non_speech_ms: 5000}").unwrap();
    assert_ne!(
        per_rule_config_hash(&a, "voice.speakers"),
        per_rule_config_hash(&b, "voice.speakers")
    );
    assert_eq!(
        per_rule_config_hash(&a, "voice.silence"),
        per_rule_config_hash(&b, "voice.silence")
    );
    assert!(per_rule_config_hash(&a, "voice.transcript").is_none());
}
