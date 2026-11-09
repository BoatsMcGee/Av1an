use std::{collections::BTreeMap, path::PathBuf, time::SystemTime};

use anyhow::{Result, bail};
use av_scenechange::ScenecutResult;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use strum::{Display, EnumString, IntoStaticStr};
use thiserror::Error;

use crate::models::{input::Input as InputModel, sequence::SequenceConfigHandler};

pub static DEFAULT_MAX_SCENE_LENGTH_SECONDS: u8 = 10;
pub static DEFAULT_MIN_SCENE_LENGTH_FRAMES: u8 = 24;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SceneDetectorData
where
    Self: Default,
{
    pub scenecut_scores:   Option<BTreeMap<usize, ScenecutScore>>,
    /// Per-frame TransNetV2 transition probabilities, absolute frame index.
    pub transnetv2_scores: Option<BTreeMap<usize, f32>>,
    pub created_on:        SystemTime,
}

impl Default for SceneDetectorData {
    #[inline]
    fn default() -> Self {
        Self {
            scenecut_scores:   None,
            transnetv2_scores: None,
            created_on:        SystemTime::now(),
        }
    }
}

pub trait SceneDetectorDataHandler {
    fn get_scene_detection(&self) -> Result<&SceneDetectorData>;
    fn get_scene_detection_mut(&mut self) -> Result<&mut SceneDetectorData>;
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct SceneDetectorConfig
where
    Self: SequenceConfigHandler,
{
    pub method: SceneDetectionMethod,
    pub input:  Option<InputModel>,
}

impl SequenceConfigHandler for SceneDetectorConfig {
}

#[derive(Debug, Error)]
pub enum SceneDetectorConfigError {
    #[error("scene detection minimum length must be positive")]
    MinimumLengthNotPositive,
    #[error("scene detection maximum length must be positive")]
    MaximumLengthNotPositive,
    #[error(
        "scene detection minimum length ({minimum_length}) must not exceed maximum length \
         ({maximum_length})"
    )]
    LengthRangeInverted {
        minimum_length: usize,
        maximum_length: usize,
    },
}

// Not `Copy`: the TransNetV2 variant carries a `PathBuf` model override.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub enum SceneDetectionMethod {
    None {
        minimum_length: usize,
        maximum_length: usize,
    },
    AVSceneChange {
        minimum_length: usize,
        maximum_length: usize,
        method:         ScenecutMethod,
        /// Keep the per-frame scenecut scores in `SceneDetectorData`. Off by
        /// default: the scores flood the JSON config and nothing reads them.
        save_scores:    bool,
    },
    /// TransNetV2 neural network via ONNX Runtime. `model_path` pins the
    /// model file; when unset it is resolved from the standard locations.
    TransNetV2 {
        minimum_length: usize,
        maximum_length: usize,
        model_path:     Option<PathBuf>,
        /// Keep the per-frame transition probabilities in `SceneDetectorData`.
        /// Off by default: they flood the JSON config and nothing reads them.
        save_scores:    bool,
    },
}

impl Default for SceneDetectionMethod {
    #[inline]
    fn default() -> Self {
        Self::AVSceneChange {
            minimum_length: DEFAULT_MIN_SCENE_LENGTH_FRAMES as usize,
            maximum_length: (DEFAULT_MIN_SCENE_LENGTH_FRAMES as usize
                * DEFAULT_MAX_SCENE_LENGTH_SECONDS as usize),
            method:         ScenecutMethod::Standard,
            save_scores:    false,
        }
    }
}

impl SceneDetectionMethod {
    #[inline]
    pub fn minimum_length(&self) -> usize {
        match self {
            SceneDetectionMethod::AVSceneChange {
                minimum_length,
                maximum_length: _,
                method: _,
                save_scores: _,
            }
            | SceneDetectionMethod::TransNetV2 {
                minimum_length,
                maximum_length: _,
                model_path: _,
                save_scores: _,
            }
            | SceneDetectionMethod::None {
                minimum_length,
                maximum_length: _,
            } => *minimum_length,
        }
    }

    #[inline]
    pub fn set_minimum_length(&mut self, length: usize) -> Result<()> {
        match self {
            SceneDetectionMethod::AVSceneChange {
                minimum_length,
                maximum_length: _,
                method: _,
                save_scores: _,
            }
            | SceneDetectionMethod::TransNetV2 {
                minimum_length,
                maximum_length: _,
                model_path: _,
                save_scores: _,
            }
            | SceneDetectionMethod::None {
                minimum_length,
                maximum_length: _,
            } => {
                *minimum_length = length;
                Ok(())
            },
        }
    }

    #[inline]
    pub fn maximum_length(&self) -> usize {
        match self {
            SceneDetectionMethod::AVSceneChange {
                minimum_length: _,
                maximum_length,
                method: _,
                save_scores: _,
            }
            | SceneDetectionMethod::TransNetV2 {
                minimum_length: _,
                maximum_length,
                model_path: _,
                save_scores: _,
            }
            | SceneDetectionMethod::None {
                minimum_length: _,
                maximum_length,
            } => *maximum_length,
        }
    }

    #[inline]
    pub fn set_maximum_length(&mut self, length: usize) -> Result<()> {
        match self {
            SceneDetectionMethod::AVSceneChange {
                minimum_length: _,
                maximum_length,
                method: _,
                save_scores: _,
            }
            | SceneDetectionMethod::TransNetV2 {
                minimum_length: _,
                maximum_length,
                model_path: _,
                save_scores: _,
            }
            | SceneDetectionMethod::None {
                minimum_length: _,
                maximum_length,
            } => {
                *maximum_length = length;
                Ok(())
            },
        }
    }

    /// The maximum scene length in frames for `fps`.
    ///
    /// A zero or non-finite frame rate yields the minimum length.
    #[inline]
    pub fn maximum_length_for_fps(fps: f64) -> usize {
        if !fps.is_finite() || fps <= 0.0 {
            return DEFAULT_MIN_SCENE_LENGTH_FRAMES as usize;
        }
        (fps * DEFAULT_MAX_SCENE_LENGTH_SECONDS as f64).round().max(1.0) as usize
    }

    /// The `SceneDetectionMethod::default()` maximum length, derived from the
    /// minimum-length constant rather than the frame rate.
    #[inline]
    pub fn is_default_maximum_length(&self) -> bool {
        self.maximum_length()
            == DEFAULT_MIN_SCENE_LENGTH_FRAMES as usize * DEFAULT_MAX_SCENE_LENGTH_SECONDS as usize
    }

    /// Replaces the frame-rate-independent default maximum length with one
    /// derived from `fps`. A user-supplied value is left untouched.
    #[inline]
    pub fn resolve_defaults(&mut self, fps: f64) {
        if self.is_default_maximum_length() {
            let _ = self.set_maximum_length(Self::maximum_length_for_fps(fps));
        }
    }

    #[inline]
    pub fn validate_lengths(&self) -> Result<()> {
        let minimum_length = self.minimum_length();
        let maximum_length = self.maximum_length();
        if minimum_length == 0 {
            bail!(SceneDetectorConfigError::MinimumLengthNotPositive);
        }
        if maximum_length == 0 {
            bail!(SceneDetectorConfigError::MaximumLengthNotPositive);
        }
        if minimum_length > maximum_length {
            bail!(SceneDetectorConfigError::LengthRangeInverted {
                minimum_length,
                maximum_length,
            });
        }
        Ok(())
    }

    #[inline]
    pub fn method(&self) -> Result<ScenecutMethod> {
        match self {
            SceneDetectionMethod::AVSceneChange {
                minimum_length: _,
                maximum_length: _,
                method,
                save_scores: _,
            } => Ok(*method),
            _ => {
                bail!("Cannot get method for None method");
            },
        }
    }

    #[inline]
    pub fn set_method(&mut self, scenecut_method: ScenecutMethod) -> Result<()> {
        match self {
            SceneDetectionMethod::AVSceneChange {
                minimum_length: _,
                maximum_length: _,
                method,
                save_scores: _,
            } => {
                *method = scenecut_method;
                Ok(())
            },
            _ => {
                bail!("Cannot set method for None method");
            },
        }
    }
}

#[derive(
    Serialize,
    Deserialize,
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    EnumString,
    IntoStaticStr,
    Display,
    JsonSchema,
)]
pub enum ScenecutMethod {
    #[strum(serialize = "fast")]
    Fast,
    #[strum(serialize = "standard")]
    Standard,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ScenecutScore {
    pub inter_cost:             f64,
    pub imp_block_cost:         f64,
    pub backward_adjusted_cost: f64,
    pub forward_adjusted_cost:  f64,
    pub threshold:              f64,
}

impl ScenecutScore {
    #[inline]
    pub fn from_scenecutresult(result: &ScenecutResult) -> Self {
        Self {
            inter_cost:             result.inter_cost,
            imp_block_cost:         result.imp_block_cost,
            backward_adjusted_cost: result.backward_adjusted_cost,
            forward_adjusted_cost:  result.forward_adjusted_cost,
            threshold:              result.threshold,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lengths(method: &SceneDetectionMethod) -> (usize, usize) {
        (method.minimum_length(), method.maximum_length())
    }

    #[test]
    fn zero_lengths_are_rejected() {
        for (minimum_length, maximum_length) in [(0, 100), (100, 0), (0, 0)] {
            let method = SceneDetectionMethod::None {
                minimum_length,
                maximum_length,
            };
            assert!(
                method.validate_lengths().is_err(),
                "{minimum_length} - {maximum_length} should be rejected"
            );
        }
    }

    #[test]
    fn inverted_length_range_is_rejected() {
        let method = SceneDetectionMethod::None {
            minimum_length: 101,
            maximum_length: 100,
        };
        assert!(method.validate_lengths().is_err());

        let mut method = SceneDetectionMethod::None {
            minimum_length: 100,
            maximum_length: 100,
        };
        assert!(method.validate_lengths().is_ok(), "equal lengths are valid");

        method.set_minimum_length(99).expect("set minimum length");
        assert!(method.validate_lengths().is_ok());
    }

    #[test]
    fn maximum_length_defaults_to_ten_seconds_of_frames() {
        assert_eq!(SceneDetectionMethod::maximum_length_for_fps(24.0), 240);
        assert_eq!(SceneDetectionMethod::maximum_length_for_fps(23.976), 240);
        assert_eq!(SceneDetectionMethod::maximum_length_for_fps(60.0), 600);
        // A degenerate frame rate still yields a length that splits the clip.
        assert_eq!(
            SceneDetectionMethod::maximum_length_for_fps(0.0),
            DEFAULT_MIN_SCENE_LENGTH_FRAMES as usize
        );
        assert_eq!(
            SceneDetectionMethod::maximum_length_for_fps(f64::NAN),
            DEFAULT_MIN_SCENE_LENGTH_FRAMES as usize
        );
    }

    /// The default is frame-rate independent, so it must be replaced before the
    /// range is checked; a user-supplied value must survive untouched.
    #[test]
    fn only_the_default_maximum_length_is_resolved_from_the_frame_rate() {
        let mut method = SceneDetectionMethod::default();
        assert!(method.is_default_maximum_length());
        method.resolve_defaults(60.0);
        assert_eq!(lengths(&method), (24, 600));

        method.set_maximum_length(100).expect("set maximum length");
        assert!(!method.is_default_maximum_length());
        method.resolve_defaults(60.0);
        assert_eq!(lengths(&method), (24, 100), "a set value must survive");
    }
}
