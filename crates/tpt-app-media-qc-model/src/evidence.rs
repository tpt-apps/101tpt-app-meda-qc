//! Evidence attached to findings ([spec § 6.4]).
//!
//! Evidence is lazily generated where possible; payloads here are eagerly
//! stored so the type stays serializable and stateless. The desktop UI and
//! reporting layers decide which evidence kinds to render.

use serde::{Deserialize, Serialize};

/// Supported evidence forms.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceKind {
    /// A thumbnail image (PNG/JPEG bytes).
    Thumbnail,
    /// A full frame capture.
    FrameCapture,
    /// A region of audio waveform.
    WaveformRegion,
    /// A spectral plot payload.
    Spectrum,
    /// A numerical measurement (already captured in the finding's `measured`).
    Numeric,
    /// An excerpt of container/stream metadata.
    MetadataExcerpt,
    /// Arbitrary JSON diagnostic payload.
    JsonDiagnostic,
}

/// Serialized payload carried by an evidence item.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum EvidencePayload {
    /// Raw bytes with a declared media type, e.g. PNG.
    Bytes { media_type: String, data: Vec<u8> },
    /// Plain text excerpt.
    Text(String),
    /// Structured JSON.
    Json(serde_json::Value),
}

/// A single evidence artifact attached to a finding.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Evidence {
    pub kind: EvidenceKind,
    pub label: String,
    pub description: Option<String>,
    pub payload: EvidencePayload,
}

impl Evidence {
    pub fn text(kind: EvidenceKind, label: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            kind,
            label: label.into(),
            description: None,
            payload: EvidencePayload::Text(text.into()),
        }
    }

    pub fn json(kind: EvidenceKind, label: impl Into<String>, payload: serde_json::Value) -> Self {
        Self {
            kind,
            label: label.into(),
            description: None,
            payload: EvidencePayload::Json(payload),
        }
    }

    pub fn describe(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }
}
