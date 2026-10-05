//! Human-readable, versioned, deterministic QC profiles ([spec § 9]).
//!
//! A profile selects rules and their thresholds. The typed model here is the
//! canonical form: parsing ([`parse`]) validates the human YAML form, and the
//! canonical YAML serialization of this model is what gets hashed for profile
//! integrity and cache keys ([spec § 14.1], [spec § 19]).

pub mod custom;
pub mod hash;
pub mod model;
pub mod parse;

pub use custom::{CustomOp, CustomOperand, CustomRule, CustomScope, CustomTarget};
pub use hash::{per_rule_config_hash, profile_sha256};
pub use model::{
    valid_plugin_rule_id, AudioRules, BlockinessRule, BlurRule, ChannelLayoutRule, ContainerRules,
    CountThresholdRule, DbThresholdRule, DeadPixelRule, DurationThresholdRule, ExpectValueRule,
    FrameRateRule, LoudnessRule, LoudnessStandard, MinBitrateRule, NoiseRule, PluginRuleConfig,
    Policy, Profile, Resolution, ResolutionRule, RuleSetConfig, SpeakerChangeRule,
    SpeakerCountRule, SpeechExpectation, SpeechRule, SubtitleContentRule, SubtitleDurationRule,
    SubtitleLanguageRule, SubtitlePresenceRule, SubtitleRules, SubtitleTimingRule,
    TimestampContinuityRule, ToleranceRule, TranscriptRule, VideoRules, VoiceRules,
    VoiceSilenceRule, RESERVED_RULE_PREFIXES,
};
pub use parse::{parse_str, ProfileError};

impl Profile {
    /// Build the typed profile from YAML source.
    pub fn from_yaml(source: &str) -> Result<Self, ProfileError> {
        parse_str(source)
    }
}
