//! QC rule framework ([spec § 7]) and built-in rule catalogue.
//!
//! Rules are components: they declare their identity, description and
//! capabilities, and execute against a [`rule::RuleContext`] containing the
//! asset and its [`Inspection`] measurements. Rules emit findings only for
//! **non-pass** outcomes — pass is the absence of findings, so aggregation and
//! reports stay simple. Rules must report `Inconclusive` rather than guessing
//! on missing measurements ([spec § 3.4]).
//!
//! The crate is organised so a rule can run in the metadata-only (pass 1) or
//! decode (pass 2) phase: [`rule::Capabilities`] routes this.

mod audio;
mod container;
#[cfg(test)]
mod testutil;
mod util;
mod video;

pub mod rule;

pub use audio::build as build_audio_rules;
pub use container::build as build_container_rules;
pub use rule::{Capabilities, DecodeRequirement, QcRule, RuleContext, RuleDescription, RuleExt, RuleResult};
pub use video::build as build_video_rules;

/// Build every rule configured by a profile ([spec § 9]).
///
/// The order is deterministic (container, video, audio) so engines and reports
/// iterate rules in a stable order.
pub fn build_rules(profile: &tpt_app_media_qc_profile::model::Profile) -> Vec<Box<dyn QcRule>> {
    let mut rules: Vec<Box<dyn QcRule>> = Vec::new();
    container::build(profile, &mut rules);
    video::build(profile, &mut rules);
    audio::build(profile, &mut rules);
    rules
}

/// All rule IDs the built-in catalogue knows, for `--list-rules` style tooling.
pub fn known_rule_ids() -> Vec<&'static str> {
    container::RULE_IDS
        .iter()
        .chain(video::RULE_IDS.iter())
        .chain(audio::RULE_IDS.iter())
        .copied()
        .collect()
}