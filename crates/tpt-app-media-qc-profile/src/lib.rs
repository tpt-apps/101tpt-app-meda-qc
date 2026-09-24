//! Human-readable, versioned, deterministic QC profiles ([spec § 9]).
//!
//! A profile selects rules and their thresholds. The typed model here is the
//! canonical form: parsing ([`parse`]) validates the human YAML form, and the
//! canonical YAML serialization of this model is what gets hashed for profile
//! integrity and cache keys ([spec § 14.1], [spec § 19]).

pub mod hash;
pub mod model;
pub mod parse;

pub use hash::{per_rule_config_hash, profile_sha256};
pub use model::{
    AudioRules, ChannelLayoutRule, ContainerRules, CountThresholdRule, DbThresholdRule,
    DurationThresholdRule, ExpectValueRule, FrameRateRule, LoudnessRule, LoudnessStandard,
    MinBitrateRule, Policy, Profile, Resolution, ResolutionRule, RuleSetConfig, SubtitleRules,
    TimestampContinuityRule, ToleranceRule, VideoRules, VoiceRules,
};
pub use parse::{parse_str, ProfileError};

impl Profile {
    /// Build the typed profile from YAML source.
    pub fn from_yaml(source: &str) -> Result<Self, ProfileError> {
        parse_str(source)
    }
}
