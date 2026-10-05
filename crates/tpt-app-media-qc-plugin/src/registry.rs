//! Plugin discovery and the [`QcRule`] adapter.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use tpt_app_media_qc_core::cost::CostClass;
use tpt_app_media_qc_model::asset::StreamKind;
use tpt_app_media_qc_model::finding::{QcFinding, RuleId};
use tpt_app_media_qc_model::severity::{Severity, VerdictDecision};
use tpt_app_media_qc_plugin_sdk::{PluginRequest, PluginStatus, PROTOCOL_VERSION};
use tpt_app_media_qc_profile::model::Profile;
use tpt_app_media_qc_rules::{
    Capabilities, DecodeRequirement, QcRule, RuleContext, RuleDescription, RuleResult,
};

use crate::manifest::{ManifestRule, PluginDecode, PluginManifest};
use crate::run::{run_plugin, RunSpec};

/// Plugin directories hold a handful of plugins; bound the scan anyway.
const MAX_PLUGINS: usize = 256;
/// Longest finding message kept from a plugin.
const MAX_MESSAGE_CHARS: usize = 2000;

#[derive(Debug, thiserror::Error)]
pub enum PluginError {
    #[error("plugin directory '{0}' cannot be read: {1}")]
    Directory(PathBuf, String),
    #[error("rule '{0}' is provided by two plugins")]
    DuplicateRule(String),
    #[error(
        "the profile uses rule(s) {0:?} but no installed plugin provides them (plugin directory: {1})"
    )]
    MissingRules(Vec<String>, String),
}

/// What `list-rules` shows about an installed plugin.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PluginInfo {
    pub id: String,
    pub name: String,
    pub version: String,
    pub rules: Vec<String>,
}

struct Installed {
    manifest: Arc<PluginManifest>,
    dir: PathBuf,
    rule: ManifestRule,
}

/// The installed plugins and the rules they provide.
#[derive(Default)]
pub struct PluginRegistry {
    rules: BTreeMap<String, Installed>,
    plugins: Vec<PluginInfo>,
    /// Plugins that were skipped, and why.
    pub warnings: Vec<String>,
    source: String,
}

impl PluginRegistry {
    /// A registry with no plugins.
    pub fn empty() -> Self {
        Self::default()
    }

    /// Scan `dir` for `<plugin>/plugin.json`. Plugins with an invalid
    /// manifest or an unusable command are skipped with a warning; two
    /// plugins providing the same rule is an error.
    pub fn discover(dir: &Path) -> Result<Self, PluginError> {
        let entries = std::fs::read_dir(dir)
            .map_err(|e| PluginError::Directory(dir.to_path_buf(), e.to_string()))?;
        let mut folders: Vec<PathBuf> = entries
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.join("plugin.json").is_file())
            .collect();
        folders.sort();
        folders.truncate(MAX_PLUGINS);

        let mut registry = PluginRegistry {
            source: dir.display().to_string(),
            ..Default::default()
        };
        for folder in folders {
            let manifest = match PluginManifest::load(&folder) {
                Ok(m) => m,
                Err(e) => {
                    registry
                        .warnings
                        .push(format!("skipped plugin in '{}': {e}", folder.display()));
                    continue;
                }
            };
            if let Err(e) = manifest.resolve_command(&folder) {
                registry
                    .warnings
                    .push(format!("skipped plugin '{}': {e}", manifest.id));
                continue;
            }
            if registry.plugins.iter().any(|p| p.id == manifest.id) {
                registry.warnings.push(format!(
                    "skipped second plugin with id '{}' in '{}'",
                    manifest.id,
                    folder.display()
                ));
                continue;
            }
            registry.plugins.push(PluginInfo {
                id: manifest.id.clone(),
                name: manifest.name.clone(),
                version: manifest.version.clone(),
                rules: manifest.rules.iter().map(|r| r.id.clone()).collect(),
            });
            let shared = Arc::new(manifest);
            for rule in &shared.rules {
                let installed = Installed {
                    manifest: Arc::clone(&shared),
                    dir: folder.clone(),
                    rule: rule.clone(),
                };
                if registry.rules.insert(rule.id.clone(), installed).is_some() {
                    return Err(PluginError::DuplicateRule(rule.id.clone()));
                }
            }
        }
        Ok(registry)
    }

    /// Installed plugins, in directory order.
    pub fn plugins(&self) -> &[PluginInfo] {
        &self.plugins
    }

    /// Whether any plugin provides `rule`.
    pub fn provides(&self, rule: &str) -> bool {
        self.rules.contains_key(rule)
    }

    /// Build the plugin rules a profile asks for. Fails, naming every missing
    /// rule, if the profile uses one that is not installed.
    pub fn build_rules(&self, profile: &Profile) -> Result<Vec<Box<dyn QcRule>>, PluginError> {
        let missing: Vec<String> = profile
            .rules
            .plugins
            .iter()
            .filter(|p| !self.rules.contains_key(&p.rule))
            .map(|p| p.rule.clone())
            .collect();
        if !missing.is_empty() {
            let place = if self.source.is_empty() {
                "none configured".to_string()
            } else {
                self.source.clone()
            };
            return Err(PluginError::MissingRules(missing, place));
        }
        Ok(profile
            .rules
            .plugins
            .iter()
            .map(|cfg| {
                let installed = &self.rules[&cfg.rule];
                Box::new(PluginRule {
                    id: cfg.rule.clone(),
                    manifest: Arc::clone(&installed.manifest),
                    dir: installed.dir.clone(),
                    rule: installed.rule.clone(),
                    severity: cfg.severity,
                    config: cfg.config.clone(),
                }) as Box<dyn QcRule>
            })
            .collect())
    }
}

struct PluginRule {
    id: String,
    manifest: Arc<PluginManifest>,
    dir: PathBuf,
    rule: ManifestRule,
    severity: Severity,
    config: serde_json::Value,
}

/// `&'static` slices for the stream combinations a manifest can ask for.
fn static_streams(kinds: &[StreamKind]) -> &'static [StreamKind] {
    let has = |k| kinds.contains(&k);
    match (
        has(StreamKind::Video),
        has(StreamKind::Audio),
        has(StreamKind::Subtitle),
    ) {
        (false, false, false) => &[],
        (true, false, false) => &[StreamKind::Video],
        (false, true, false) => &[StreamKind::Audio],
        (false, false, true) => &[StreamKind::Subtitle],
        (true, true, false) => &[StreamKind::Video, StreamKind::Audio],
        (true, false, true) => &[StreamKind::Video, StreamKind::Subtitle],
        (false, true, true) => &[StreamKind::Audio, StreamKind::Subtitle],
        (true, true, true) => &[StreamKind::Video, StreamKind::Audio, StreamKind::Subtitle],
    }
}

impl PluginRule {
    fn problem(&self, message: impl std::fmt::Display) -> RuleResult {
        RuleResult::from_finding(
            QcFinding::new(RuleId::new(self.id.clone()))
                .status(VerdictDecision::Inconclusive)
                .severity(self.severity)
                .set_message(format!(
                    "plugin '{}' {} did not complete: {message}",
                    self.manifest.id, self.manifest.version
                )),
        )
    }
}

impl QcRule for PluginRule {
    fn id(&self) -> RuleId {
        RuleId::new(self.id.clone())
    }

    fn description(&self) -> RuleDescription {
        RuleDescription {
            name: "Plugin rule",
            summary: "Third-party rule provided by an installed plugin.",
            version: "plugin",
        }
    }

    fn capabilities(&self) -> Capabilities {
        match self.rule.decode {
            PluginDecode::None => Capabilities {
                required_streams: static_streams(&self.rule.streams),
                decode: DecodeRequirement::None,
                incremental: false,
                gpu: false,
                cost: CostClass::Metadata,
            },
            PluginDecode::Stream => Capabilities {
                required_streams: static_streams(&self.rule.streams),
                decode: DecodeRequirement::Stream,
                incremental: false,
                gpu: false,
                cost: CostClass::ExpensiveAnalysis,
            },
        }
    }

    fn run(&self, ctx: &RuleContext<'_>) -> RuleResult {
        // A rule that declares the streams it needs has nothing to say about a
        // file without them; do not start a process for it.
        if self
            .rule
            .streams
            .iter()
            .any(|kind| ctx.streams_of(*kind).next().is_none())
        {
            return RuleResult::pass();
        }
        let spec = match self.manifest.resolve_command(&self.dir) {
            Ok(program) => RunSpec {
                program,
                args: self.manifest.args.clone(),
                working_dir: self.dir.clone(),
                timeout: Duration::from_millis(self.manifest.timeout_ms()),
            },
            Err(e) => return self.problem(e),
        };
        let request = PluginRequest {
            protocol: PROTOCOL_VERSION,
            rule: self.id.clone(),
            config: self.config.clone(),
            asset: ctx.asset.clone(),
            inspection: ctx.inspection.clone(),
        };
        let bytes = match serde_json::to_vec(&request) {
            Ok(b) => b,
            Err(e) => return self.problem(format!("request could not be encoded: {e}")),
        };
        let response = match run_plugin(&spec, bytes) {
            Ok(r) => r,
            Err(e) => return self.problem(e),
        };

        let findings = response
            .findings
            .into_iter()
            .map(|f| {
                let status = match f.status {
                    PluginStatus::Pass => VerdictDecision::Pass,
                    PluginStatus::Warn => VerdictDecision::Warn,
                    PluginStatus::Fail => VerdictDecision::Fail,
                    PluginStatus::Inconclusive => VerdictDecision::Inconclusive,
                };
                let mut message: String = f.message.chars().take(MAX_MESSAGE_CHARS).collect();
                message.retain(|c| !c.is_control() || c == '\n');
                // Rule id and severity belong to the host and the profile, not
                // to the plugin.
                let mut finding = QcFinding::new(RuleId::new(self.id.clone()))
                    .status(status)
                    .severity(self.severity)
                    .set_message(message);
                finding.measured = f.measured;
                finding.expected = f.expected;
                finding.time_range = f.time_range;
                if let Some(index) = f.stream {
                    finding = finding.stream(tpt_app_media_qc_model::asset::StreamId::new(index));
                }
                finding
            })
            .collect();
        RuleResult::findings(findings)
    }
}
