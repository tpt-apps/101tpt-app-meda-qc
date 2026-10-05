//! The inspector used by full CLI and desktop scans ([spec § 10]).
//!
//! Container and stream metadata come from the in-process, royalty-free-only
//! [`NativeInspector`]. Full scans add Kinetix AV1/VP9 frame decode (MP4,
//! Matroska/WebM, MPEG-TS), Cadence audio decode (WAV, AIFF/AIFC, FLAC) and
//! the optional voice analyses a profile asks for. No external program is
//! involved. Unsupported codecs and incomplete decode coverage stay explicit
//! in the inspection model.

use tpt_app_media_qc_core::error::Result;
use tpt_app_media_qc_decode::{
    CadenceAudioInspector, KinetixVideoInspector, VoiceAnalyzer, VoiceConfig,
};
use tpt_app_media_qc_model::asset::Asset;
use tpt_app_media_qc_model::inspection::Inspection;
use tpt_app_media_qc_pipeline::{InspectionLevel, Inspector};
use tpt_app_media_qc_probe::NativeInspector;

/// Composite inspector used by full scans.
pub struct HybridInspector {
    metadata: NativeInspector,
    video: KinetixVideoInspector,
    audio: CadenceAudioInspector,
    voice: VoiceAnalyzer,
}

impl Default for HybridInspector {
    fn default() -> Self {
        Self::with_voice(VoiceConfig::default())
    }
}

impl HybridInspector {
    /// An inspector that also runs the optional voice analyses in `voice`.
    pub fn with_voice(voice: VoiceConfig) -> Self {
        Self {
            metadata: NativeInspector,
            video: KinetixVideoInspector::new(),
            audio: CadenceAudioInspector::new(),
            voice: VoiceAnalyzer::new(voice),
        }
    }

    /// Voice analyses the profile asks for (spec 8.8). Nothing runs unless
    /// the profile configures a `rules.voice` check.
    pub fn voice_config_for(profile: &tpt_app_media_qc_profile::model::Profile) -> VoiceConfig {
        let v = &profile.rules.voice;
        VoiceConfig {
            speech: v.speech.is_some()
                || v.silence.is_some()
                || v.speakers.is_some()
                || v.speaker_changes.is_some(),
            transcript: v.transcript.is_some(),
        }
    }
}

impl Inspector for HybridInspector {
    fn name(&self) -> &str {
        "tpt-native-probe + tpt-kinetix-av1-vp9 + tpt-cadence"
    }

    fn inspect_metadata(&self, asset: &Asset) -> Result<Inspection> {
        self.metadata.inspect_metadata(asset)
    }

    fn inspect_decode(
        &self,
        asset: &Asset,
        metadata: &Inspection,
        level: InspectionLevel,
    ) -> Result<Inspection> {
        let video = self.video.inspect_decode(asset, metadata, level)?;
        let audio = self.audio.inspect_decode(asset, &video, level)?;
        Ok(self.voice.analyze(asset, &audio))
    }
}
