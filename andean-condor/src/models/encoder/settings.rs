//! Pure settings checks; these never resolve or run an encoder executable.

use thiserror::Error;

use super::{Encoder, EncoderBase, EncoderPasses, cli_parameter::CLIParameter};

#[derive(Debug, Error, PartialEq)]
pub enum EncoderSettingsError {
    #[error("the {0} encoder is single-pass only and does not take a pass count")]
    PassesNotSupported(EncoderBase),
    #[error("the {0} encoder does not support Photon Noise")]
    PhotonNoiseNotSupported(EncoderBase),
    #[error("chroma Photon Noise requires luma Photon Noise")]
    ChromaWithoutLuma,
    #[error(
        "the {0} encoder does not take --ffmpeg-filter; pass `-vf` through its parameters instead"
    )]
    FfmpegFilterNotSupported(EncoderBase),
    #[error("{0} does not support pass {1} of {2} in this frontend (only {3} pass(es))")]
    InvalidPass(EncoderBase, u8, u8, u8),
    #[error("{encoder} parameter {name} must be {requirement} (and finite)")]
    InvalidNumber {
        encoder: EncoderBase,
        name: String,
        requirement: &'static str,
    },
    #[error("SVT-AV1 settings conflict: {0}")]
    SvtMode(&'static str),
    #[error("Photon Noise {0} must contain at most {1} coefficients")]
    NoiseCoefficients(&'static str, usize),
}

impl EncoderBase {
    /// Checks a frontend pass tuple, independently of an encoder's stored pass setting.
    #[inline]
    pub fn validate_pass(&self, (current, total): (u8, u8)) -> Result<(), EncoderSettingsError> {
        let max = if matches!(self, Self::AVM | Self::FFmpeg) {
            1
        } else {
            2
        };
        // AOM has a native third pass, but this frontend only implements two.
        if current == 0 || current > total || total == 0 || total > max {
            return Err(EncoderSettingsError::InvalidPass(
                *self, current, total, max,
            ));
        }
        Ok(())
    }
}

impl Encoder {
    /// Validates stored/loaded settings without probing binaries or writing sidecars.
    /// Unknown options remain available; only numeric finiteness is universal.
    #[inline]
    pub fn validate_settings(&self) -> Result<(), EncoderSettingsError> {
        let pass = match self.passes() {
            EncoderPasses::All(total) => (1, total),
            EncoderPasses::Specific(current, total) => (current, total),
        };
        self.validate_pass_settings(pass)?;
        if let Some(noise) = self.photon_noise() {
            for (name, coefficients, max) in
                [("c_y", &noise.c_y, 24), ("ccb", &noise.ccb, 25), ("ccr", &noise.ccr, 25)]
            {
                if coefficients.as_ref().is_some_and(|values| values.len() > max) {
                    return Err(EncoderSettingsError::NoiseCoefficients(name, max));
                }
            }
        }
        Ok(())
    }

    /// Checks explicit pass calls as well as settings which affect native pass selection.
    #[inline]
    pub fn validate_pass_settings(&self, pass: (u8, u8)) -> Result<(), EncoderSettingsError> {
        let base = self.base();
        base.validate_pass(pass)?;
        for (name, parameter) in self.parameters() {
            if let CLIParameter::Number { value, .. } = parameter
                && !value.is_finite()
            {
                return Err(self.invalid_number(name, "finite"));
            }
            let (min, max, step, requirement) = match (base, name.as_str()) {
                (EncoderBase::AOM | EncoderBase::VPX, "cq-level") => {
                    (0.0, 63.0, 1.0, "an integer in 0..=63")
                },
                (EncoderBase::RAV1E, "quantizer") => (0.0, 255.0, 1.0, "an integer in 0..=255"),
                (EncoderBase::RAV1E, "speed") => (0.0, 10.0, 1.0, "an integer in 0..=10"),
                (EncoderBase::AVM, "qp") => (
                    -96.0,
                    255.0,
                    1.0,
                    "an integer in -96..=255 (bit-depth dependent)",
                ),
                (EncoderBase::X264, "crf") => (-12.0, 51.0, 0.0, "in -12..=51"),
                (EncoderBase::X264, "qp") => {
                    (0.0, 81.0, 1.0, "an integer in 0..=81 (bit-depth dependent)")
                },
                (EncoderBase::X265, "crf") => (0.0, 51.0, 0.0, "in 0..=51"),
                (EncoderBase::VVenC, "qp") => (0.0, 63.0, 0.0, "in 0..=63"),
                (EncoderBase::SVTAV1, "crf" | "cqp") => {
                    (0.0, 70.0, 0.25, "in 0..=70 in quarter steps")
                },
                (EncoderBase::SVTAV1, "qp" | "q" | "min-qp" | "max-qp") => {
                    (0.0, 63.0, 1.0, "an integer in 0..=63")
                },
                (EncoderBase::SVTAV1, "rc" | "aq-mode") => (0.0, 2.0, 1.0, "an integer in 0..=2"),
                (EncoderBase::SVTAV1, "preset") => (-1.0, 13.0, 1.0, "an integer in -1..=13"),
                (EncoderBase::SVTAV1, "pred-struct") => (1.0, 2.0, 1.0, "1 or 2"),
                (EncoderBase::SVTAV1, "keyint") => (
                    -2.0,
                    f64::from(i32::MAX),
                    1.0,
                    "an integer in -2..=2147483647 (optionally suffixed s)",
                ),
                (EncoderBase::SVTAV1, "tbr" | "mbr") => (0.0, 100_000.0, 0.0, "in 0..=100000 kbps"),
                (EncoderBase::FFmpeg, _) => continue,
                (_, "passes") => (
                    1.0,
                    if base == EncoderBase::AVM { 1.0 } else { 2.0 },
                    1.0,
                    "a frontend-supported integer pass count",
                ),
                (_, "pass") => (
                    if matches!(
                        base,
                        EncoderBase::SVTAV1
                            | EncoderBase::AVM
                            | EncoderBase::X264
                            | EncoderBase::X265
                    ) {
                        0.0
                    } else {
                        1.0
                    },
                    if base == EncoderBase::AVM { 1.0 } else { 2.0 },
                    1.0,
                    "a frontend-supported integer pass index",
                ),
                _ => continue,
            };
            let value = self.numeric_setting(name)?.expect("parameter exists");
            if !value.is_finite()
                || value < min
                || value > max
                || (step != 0.0 && (value / step).fract() != 0.0)
            {
                return Err(self.invalid_number(name, requirement));
            }
        }
        if base == EncoderBase::SVTAV1 {
            let rc = self.numeric_setting("rc")?.unwrap_or(0.0);
            if rc == 1.0 && self.numeric_setting("keyint")?.is_some_and(|keyint| keyint <= 0.0) {
                return Err(EncoderSettingsError::SvtMode(
                    "VBR requires a positive explicit keyint; replace keyint 0/-1/-2 with an interval",
                ));
            }
            if rc != 0.0
                && (self.parameters().contains_key("crf") || self.parameters().contains_key("cqp"))
            {
                return Err(EncoderSettingsError::SvtMode(
                    "crf/cqp require rc 0; remove the conflicting quality option",
                ));
            }
            if self.parameters().contains_key("crf") && self.parameters().contains_key("cqp") {
                return Err(EncoderSettingsError::SvtMode(
                    "choose either crf or cqp, not both",
                ));
            }
            let total = f64::from(pass.1).max(self.numeric_setting("passes")?.unwrap_or(1.0)).max(
                if self.numeric_setting("pass")?.is_some_and(|pass| pass > 0.0) {
                    2.0
                } else {
                    1.0
                },
            );
            if total > 1.0 && (rc != 1.0 || self.numeric_setting("preset")?.unwrap_or(8.0) > 9.0) {
                return Err(EncoderSettingsError::SvtMode(
                    "multi-pass requires VBR (rc 1) and preset <= 9; CRF/CQP and CBR are single-pass only",
                ));
            }
            let pred = self.numeric_setting("pred-struct")?.unwrap_or(2.0);
            if (rc == 1.0 && pred != 2.0) || (rc == 2.0 && pred != 1.0) {
                return Err(EncoderSettingsError::SvtMode(
                    "VBR requires random access (pred-struct 2); CBR requires low delay (pred-struct 1)",
                ));
            }
        }
        Ok(())
    }

    fn invalid_number(&self, name: &str, requirement: &'static str) -> EncoderSettingsError {
        EncoderSettingsError::InvalidNumber {
            encoder: self.base(),
            name: name.to_owned(),
            requirement,
        }
    }

    fn numeric_setting(&self, name: &str) -> Result<Option<f64>, EncoderSettingsError> {
        let Some(parameter) = self.parameters().get(name) else {
            return Ok(None);
        };
        let value = match parameter {
            CLIParameter::Number { value, .. } => Some(*value),
            CLIParameter::String { value, .. } => {
                if self.base() == EncoderBase::SVTAV1 && name == "rc" {
                    match value.as_str() {
                        "crf" | "cqp" => Some(0.0),
                        "vbr" => Some(1.0),
                        "cbr" => Some(2.0),
                        _ => value.parse().ok(),
                    }
                } else if self.base() == EncoderBase::SVTAV1 && name == "keyint" {
                    value.strip_suffix('s').unwrap_or(value).parse().ok()
                } else if self.base() == EncoderBase::SVTAV1 && matches!(name, "tbr" | "mbr") {
                    let (number, scale) = match value.as_bytes().last() {
                        Some(b'b' | b'B') => {
                            (value.strip_suffix(['b', 'B']).expect("ASCII suffix"), 0.001)
                        },
                        Some(b'k' | b'K') => {
                            (value.strip_suffix(['k', 'K']).expect("ASCII suffix"), 1.0)
                        },
                        Some(b'm' | b'M') => (
                            value.strip_suffix(['m', 'M']).expect("ASCII suffix"),
                            1000.0,
                        ),
                        _ => (value.as_str(), 1.0),
                    };
                    number.parse::<f64>().ok().map(|v| v * scale)
                } else {
                    value.parse().ok()
                }
            },
            CLIParameter::Bool { .. } => None,
        };
        value.map(Some).ok_or_else(|| self.invalid_number(name, "numeric"))
    }
}
