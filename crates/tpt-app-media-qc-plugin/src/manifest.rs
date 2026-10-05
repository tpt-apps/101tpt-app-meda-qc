//! `plugin.json` manifest parsing and validation.

use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use tpt_app_media_qc_model::asset::StreamKind;
use tpt_app_media_qc_plugin_sdk::PROTOCOL_VERSION;
use tpt_app_media_qc_profile::model::{valid_plugin_rule_id, RESERVED_RULE_PREFIXES};

/// Longest per-rule timeout a manifest may ask for.
pub const MAX_TIMEOUT_MS: u64 = 300_000;
const DEFAULT_TIMEOUT_MS: u64 = 10_000;
const MAX_RULES: usize = 64;
const MAX_ARGS: usize = 32;
const MAX_ARG_LEN: usize = 256;
const MAX_MANIFEST_BYTES: u64 = 256 * 1024;

/// Whether a rule needs decoded measurements.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginDecode {
    /// Metadata only; runs in the quick scan too.
    #[default]
    None,
    /// Needs decode-pass measurements; skipped (inconclusive) in quick scans.
    Stream,
}

/// One rule a plugin provides.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestRule {
    /// Rule id, `<plugin id>.<name>`.
    pub id: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub decode: PluginDecode,
    /// Stream kinds the rule needs; the rule is skipped for files without
    /// them. Allowed: `video`, `audio`, `subtitle`.
    #[serde(default)]
    pub streams: Vec<StreamKind>,
}

/// A parsed, validated `plugin.json`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginManifest {
    /// Manifest schema version; currently `1`.
    pub schema: u32,
    /// Plugin id: lowercase letters, digits, `_` and `-`; the prefix of every
    /// rule id.
    pub id: String,
    pub name: String,
    pub version: String,
    /// Wire protocol the plugin speaks; must match the host's.
    pub protocol: u32,
    /// Program to run: a path inside the plugin directory (`bin/qc`), or a
    /// bare program name such as `python` found on `PATH`.
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    pub rules: Vec<ManifestRule>,
}

impl PluginManifest {
    /// Effective per-rule timeout.
    pub fn timeout_ms(&self) -> u64 {
        self.timeout_ms.unwrap_or(DEFAULT_TIMEOUT_MS)
    }

    /// Read and validate `<dir>/plugin.json`.
    pub fn load(dir: &Path) -> Result<Self, String> {
        let path = dir.join("plugin.json");
        let size = std::fs::metadata(&path)
            .map_err(|e| format!("cannot read {}: {e}", path.display()))?
            .len();
        if size > MAX_MANIFEST_BYTES {
            return Err(format!(
                "{} is larger than {MAX_MANIFEST_BYTES} bytes",
                path.display()
            ));
        }
        let text = std::fs::read_to_string(&path)
            .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        let manifest: PluginManifest =
            serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        manifest.validate()?;
        Ok(manifest)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.schema != 1 {
            return Err(format!("unsupported manifest schema {}", self.schema));
        }
        if self.protocol != PROTOCOL_VERSION {
            return Err(format!(
                "plugin speaks protocol {}, this host speaks {PROTOCOL_VERSION}",
                self.protocol
            ));
        }
        let id_ok = !self.id.is_empty()
            && self
                .id
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-');
        if !id_ok || RESERVED_RULE_PREFIXES.contains(&self.id.as_str()) {
            return Err(format!(
                "plugin id '{}' must be lowercase letters, digits, '_' or '-' and not one of: {}",
                self.id,
                RESERVED_RULE_PREFIXES.join(", ")
            ));
        }
        if self.name.trim().is_empty() || self.version.trim().is_empty() {
            return Err("'name' and 'version' are required".into());
        }
        if self.rules.is_empty() || self.rules.len() > MAX_RULES {
            return Err(format!("a plugin provides 1 to {MAX_RULES} rules"));
        }
        let prefix = format!("{}.", self.id);
        for (i, rule) in self.rules.iter().enumerate() {
            if !rule.id.starts_with(&prefix) || !valid_plugin_rule_id(&rule.id) {
                return Err(format!(
                    "rule '{}' must be '{prefix}<name>' using lowercase letters, digits, '_', '-' or '.'",
                    rule.id
                ));
            }
            if self.rules[..i].iter().any(|r| r.id == rule.id) {
                return Err(format!("rule '{}' is declared twice", rule.id));
            }
            if rule.streams.iter().any(|k| {
                !matches!(
                    k,
                    StreamKind::Video | StreamKind::Audio | StreamKind::Subtitle
                )
            }) {
                return Err(format!(
                    "rule '{}': streams may only be video, audio or subtitle",
                    rule.id
                ));
            }
        }
        if let Some(t) = self.timeout_ms {
            if t == 0 || t > MAX_TIMEOUT_MS {
                return Err(format!("timeout_ms must be 1..={MAX_TIMEOUT_MS}"));
            }
        }
        if self.args.len() > MAX_ARGS || self.args.iter().any(|a| a.len() > MAX_ARG_LEN) {
            return Err(format!(
                "at most {MAX_ARGS} arguments of {MAX_ARG_LEN} characters are allowed"
            ));
        }
        if self.command.trim().is_empty() || self.command.contains('\0') {
            return Err("'command' is required".into());
        }
        Ok(())
    }

    /// Resolve `command` to a program path.
    ///
    /// A command containing a path separator must be relative, contain no
    /// `..`, and resolve (after following links) to an existing file inside
    /// `dir`. A bare name is left for the operating system to find on `PATH`.
    pub fn resolve_command(&self, dir: &Path) -> Result<PathBuf, String> {
        let command = Path::new(&self.command);
        let has_separator = self.command.contains('/') || self.command.contains('\\');
        if !has_separator {
            return Ok(PathBuf::from(&self.command));
        }
        if command.is_absolute()
            || command.components().any(|c| {
                matches!(
                    c,
                    Component::ParentDir | Component::Prefix(_) | Component::RootDir
                )
            })
        {
            return Err(format!(
                "command '{}' must be relative to the plugin directory and may not contain '..'",
                self.command
            ));
        }
        let root = dir
            .canonicalize()
            .map_err(|e| format!("plugin directory is unreadable: {e}"))?;
        let resolved = root
            .join(command)
            .canonicalize()
            .map_err(|e| format!("command '{}' not found: {e}", self.command))?;
        if !resolved.starts_with(&root) || !resolved.is_file() {
            return Err(format!(
                "command '{}' must be a file inside the plugin directory",
                self.command
            ));
        }
        Ok(resolved)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(json: &str) -> Result<PluginManifest, String> {
        let m: PluginManifest = serde_json::from_str(json).map_err(|e| e.to_string())?;
        m.validate()?;
        Ok(m)
    }

    const GOOD: &str = r#"{"schema":1,"id":"acme","name":"Acme","version":"1.0.0","protocol":1,
        "command":"bin/acme","rules":[{"id":"acme.min_bitrate","streams":["video"]}]}"#;

    #[test]
    fn accepts_a_valid_manifest() {
        let m = manifest(GOOD).unwrap();
        assert_eq!(m.timeout_ms(), 10_000);
        assert_eq!(m.rules[0].decode, PluginDecode::None);
    }

    #[test]
    fn rejects_bad_manifests() {
        let edit = |from: &str, to: &str| GOOD.replace(from, to);
        for (json, needle) in [
            (edit("\"schema\":1", "\"schema\":2"), "schema"),
            (edit("\"protocol\":1", "\"protocol\":7"), "protocol"),
            (edit("\"id\":\"acme\"", "\"id\":\"video\""), "not one of"),
            (edit("\"id\":\"acme\"", "\"id\":\"Acme!\""), "lowercase"),
            (edit("acme.min_bitrate", "other.min_bitrate"), "must be"),
            (edit("acme.min_bitrate", "acme."), "must be"),
            (edit("\"video\"", "\"data\""), "streams may only"),
            (
                edit("\"rules\":[", "\"timeout_ms\":0,\"rules\":["),
                "timeout_ms",
            ),
            (
                edit("\"rules\":[", "\"surprise\":1,\"rules\":["),
                "unknown field",
            ),
            (
                edit("\"command\":\"bin/acme\"", "\"command\":\" \""),
                "command",
            ),
        ] {
            let err = manifest(&json).unwrap_err();
            assert!(err.contains(needle), "{json}: {err}");
        }
        let duplicated = GOOD.replace(
            "[{\"id\":\"acme.min_bitrate\",\"streams\":[\"video\"]}]",
            "[{\"id\":\"acme.a\"},{\"id\":\"acme.a\"}]",
        );
        assert!(manifest(&duplicated).unwrap_err().contains("twice"));
    }

    #[test]
    fn commands_cannot_escape_the_plugin_directory() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("bin")).unwrap();
        std::fs::write(dir.path().join("bin").join("acme"), b"x").unwrap();
        let with = |c: &str| {
            let mut m = manifest(GOOD).unwrap();
            m.command = c.into();
            m.resolve_command(dir.path())
        };
        assert!(with("bin/acme").unwrap().ends_with("acme"));
        assert_eq!(with("python").unwrap(), PathBuf::from("python"));
        for bad in [
            "../evil",
            "bin/../../evil",
            "/usr/bin/evil",
            "C:\\evil.exe",
            "bin/missing",
            "bin/",
        ] {
            assert!(with(bad).is_err(), "{bad} must be rejected");
        }
    }
}
