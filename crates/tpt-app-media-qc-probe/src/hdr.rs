//! Colour signalling helpers shared by every reader.
//!
//! Containers and AV1/VP9 configuration records carry colour as ITU-T H.273
//! (CICP) code points. Rules expect the descriptive names that probe tools
//! have always used (`bt709`, `bt2020`, `smpte2084`, `arib-std-b67`,
//! `bt2020nc`, `tv`/`pc`), so the mapping lives here once.

use tpt_app_media_qc_model::inspection::{HdrMetadata, MasteringDisplay};

/// Colour primaries code point -> name. `2` (unspecified) and unknown values
/// yield `None` ("not signalled").
pub fn primaries_name(code: u8) -> Option<&'static str> {
    Some(match code {
        1 => "bt709",
        4 => "bt470m",
        5 => "bt470bg",
        6 => "smpte170m",
        7 => "smpte240m",
        8 => "film",
        9 => "bt2020",
        10 => "smpte428",
        11 => "smpte431",
        12 => "smpte432",
        22 => "ebu3213",
        _ => return None,
    })
}

/// Transfer characteristics code point -> name.
pub fn transfer_name(code: u8) -> Option<&'static str> {
    Some(match code {
        1 => "bt709",
        4 => "gamma22",
        5 => "gamma28",
        6 => "smpte170m",
        7 => "smpte240m",
        8 => "linear",
        11 => "iec61966-2-4",
        13 => "iec61966-2-1",
        14 => "bt2020-10",
        15 => "bt2020-12",
        16 => "smpte2084",
        17 => "smpte428",
        18 => "arib-std-b67",
        _ => return None,
    })
}

/// Matrix coefficients code point -> name.
pub fn matrix_name(code: u8) -> Option<&'static str> {
    Some(match code {
        0 => "gbr",
        1 => "bt709",
        4 => "fcc",
        5 => "bt470bg",
        6 => "smpte170m",
        7 => "smpte240m",
        8 => "ycgco",
        9 => "bt2020nc",
        10 => "bt2020c",
        11 => "smpte2085",
        14 => "ictcp",
        _ => return None,
    })
}

/// Range flag -> `tv` (limited) or `pc` (full).
pub fn range_name(full_range: bool) -> &'static str {
    if full_range {
        "pc"
    } else {
        "tv"
    }
}

/// Decoded colour tags of one video stream.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Cicp {
    pub primaries: Option<u8>,
    pub transfer: Option<u8>,
    pub matrix: Option<u8>,
    /// `None` when the range is not signalled.
    pub full_range: Option<bool>,
}

impl Cicp {
    /// Apply these tags to `hdr` (fields already set are kept).
    pub fn apply(&self, hdr: &mut HdrMetadata) {
        if hdr.primaries.is_none() {
            hdr.primaries = self.primaries.and_then(primaries_name).map(str::to_string);
        }
        if hdr.transfer.is_none() {
            hdr.transfer = self.transfer.and_then(transfer_name).map(str::to_string);
        }
        if hdr.matrix.is_none() {
            hdr.matrix = self.matrix.and_then(matrix_name).map(str::to_string);
        }
        if hdr.range.is_none() {
            hdr.range = self.full_range.map(|f| range_name(f).to_string());
        }
    }

    /// The matrix name, which is what `VideoMeasurements::colorspace` holds.
    pub fn colorspace(&self) -> Option<String> {
        self.matrix.and_then(matrix_name).map(str::to_string)
    }
}

/// `Some(hdr)` unless nothing at all is signalled.
pub fn non_empty(hdr: HdrMetadata) -> Option<HdrMetadata> {
    (hdr != HdrMetadata::default()).then_some(hdr)
}

/// ST 2086 mastering display from the fixed-point form used by the ISO-BMFF
/// `mdcv` box and the AV1 metadata OBU: chromaticities in units of 0.00002,
/// luminance in units of 0.0001 cd/m^2. `primaries` is ordered red, green,
/// blue (the order Matroska uses and the one the `mdcv` box stores as G, B, R
/// must be reordered by the caller).
pub fn mastering_from_fixed(
    primaries: [(u16, u16); 3],
    white_point: (u16, u16),
    max_luminance: u32,
    min_luminance: u32,
) -> MasteringDisplay {
    let xy = |(x, y): (u16, u16)| (f64::from(x) * 0.00002, f64::from(y) * 0.00002);
    MasteringDisplay {
        min_luminance_nits: Some(f64::from(min_luminance) * 0.0001),
        max_luminance_nits: Some(f64::from(max_luminance) * 0.0001),
        primaries_xy: Some([xy(primaries[0]), xy(primaries[1]), xy(primaries[2])]),
        white_point_xy: Some(xy(white_point)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hdr10_code_points_map_to_the_names_rules_expect() {
        let mut hdr = HdrMetadata::default();
        Cicp {
            primaries: Some(9),
            transfer: Some(16),
            matrix: Some(9),
            full_range: Some(false),
        }
        .apply(&mut hdr);
        assert_eq!(hdr.primaries.as_deref(), Some("bt2020"));
        assert_eq!(hdr.transfer.as_deref(), Some("smpte2084"));
        assert_eq!(hdr.matrix.as_deref(), Some("bt2020nc"));
        assert_eq!(hdr.range.as_deref(), Some("tv"));
        assert_eq!(
            hdr.dynamic_range(),
            Some(tpt_app_media_qc_model::inspection::DynamicRange::Pq)
        );
    }

    #[test]
    fn unspecified_code_points_are_not_signalled() {
        let mut hdr = HdrMetadata::default();
        Cicp {
            primaries: Some(2),
            transfer: Some(2),
            matrix: Some(2),
            full_range: None,
        }
        .apply(&mut hdr);
        assert_eq!(non_empty(hdr), None);
    }

    #[test]
    fn mastering_display_units() {
        let m = mastering_from_fixed(
            [(34000, 16000), (13250, 34500), (7500, 3000)],
            (15635, 16450),
            10_000_000,
            50,
        );
        assert_eq!(m.max_luminance_nits, Some(1000.0));
        assert!((m.min_luminance_nits.unwrap() - 0.005).abs() < 1e-9);
        assert!((m.primaries_xy.unwrap()[0].0 - 0.68).abs() < 1e-9);
        assert!((m.white_point_xy.unwrap().0 - 0.3127).abs() < 1e-4);
    }
}
