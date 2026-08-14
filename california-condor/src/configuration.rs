use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use andean_condor::{
    core::{
        Condor,
        SaveCallback,
        input::{DecoderError, Input, ModifyNode},
        output::Output,
    },
    ffmpeg::FFPixelFormat,
    models::{
        Condor as CondorModel,
        encoder::{Encoder, EncoderBase},
        input::{Input as InputModel, VapourSynthImportMethod, VapourSynthScriptSource},
        output::Output as OutputModel,
        sequence::{
            SequenceConfigHandler,
            SequenceDataHandler,
            benchmarker::{BenchmarkerConfig, BenchmarkerConfigHandler},
            bitrate_optimizer::{BitrateOptimizerConfig, BitrateOptimizerConfigHandler},
            noise_detector::{
                NoiseDetectorConfig,
                NoiseDetectorConfigHandler,
                NoiseDetectorData,
                NoiseDetectorDataHandler,
            },
            noise_scaler::{
                NoiseScalerConfig,
                NoiseScalerConfigHandler,
                NoiseScalerData,
                NoiseScalerDataHandler,
            },
            parallel_encoder::{
                ParallelEncoderConfig,
                ParallelEncoderConfigHandler,
                ParallelEncoderData,
                ParallelEncoderDataHandler,
            },
            scene_concatenator::{SceneConcatenatorConfig, SceneConcatenatorConfigHandler},
            scene_detector::{
                DEFAULT_MAX_SCENE_LENGTH_SECONDS,
                SceneDetectionMethod,
                SceneDetectorConfig,
                SceneDetectorData,
                SceneDetectorDataHandler,
                ScenecutMethod,
            },
            speed_scaler::{SpeedScalerConfig, SpeedScalerConfigHandler},
            target_quality::{
                TargetQualityConfig,
                TargetQualityConfigHandler,
                TargetQualityData,
                TargetQualityDataHandler,
            },
        },
    },
    vapoursynth::{
        plugins::{
            bestsource::VideoSource,
            dgdecodenv::DGSource,
            ffms2::Source,
            lsmash::LWLibavSource,
            resize::Scaler,
        },
        script_builder::{VapourSynthPluginScript, script::VapourSynthScript},
        vapoursynth_filters::VapourSynthFilter,
    },
};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::info;

use crate::{commands::DecoderMethod, utils::hash_path::hash_path};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Configuration {
    pub condor:            CondorModel<CliSequenceData, CliSequenceConfig>,
    // Duplicated in case Condor instantiates a VapourSynthScript Input
    pub input:             PathBuf,
    pub temp:              PathBuf,
    pub input_filters:     Vec<VapourSynthFilter>,
    pub scd_input_filters: Vec<VapourSynthFilter>,
    pub tq_input_filters:  Vec<VapourSynthFilter>,
}

impl Configuration {
    #[inline]
    pub fn new(
        input: &Path,
        output: &Path,
        temp: Option<&Path>,
        vs_args: Option<&[String]>,
        decoder: Option<&DecoderMethod>,
    ) -> Result<Self> {
        let cwd = std::env::current_dir()?;
        let input_abs = path_abs::PathAbs::new(input)?;
        let temp = path_abs::PathAbs::new(
            temp.map_or_else(|| cwd.join(hash_path(input_abs.as_path())), PathBuf::from),
        )?
        .as_path()
        .to_path_buf();
        let input_data = Self::new_input_model(input, decoder, vs_args, None)?;
        info!("Indexing input...");
        let mut input_instance = Input::from_data(&input_data)?;
        let clip_info = input_instance.clip_info()?;
        let fps = *clip_info.frame_rate.numer() as f64 / *clip_info.frame_rate.denom() as f64;

        let scenes_directory = temp.join("scenes");

        let mut configuration = Self {
            condor: CondorModel {
                input:           input_data,
                output:          OutputModel {
                    path:       output.to_path_buf(),
                    tags:       HashMap::new(),
                    video_tags: HashMap::new(),
                },
                encoder:         Encoder::default(),
                scenes:          Vec::new(),
                sequence_config: CliSequenceConfig {
                    scene_detector:     SceneDetectorConfig {
                        input:  None,
                        method: SceneDetectionMethod::AVSceneChange {
                            minimum_length: fps.round() as usize,
                            maximum_length: DEFAULT_MAX_SCENE_LENGTH_SECONDS as usize
                                * fps.round() as usize,
                            method:         ScenecutMethod::Standard,
                        },
                    },
                    noise_detector:     None,
                    noise_scaler:       None,
                    benchmarker:        BenchmarkerConfig::default(),
                    parallel_encoder:   ParallelEncoderConfig::new(&scenes_directory),
                    scene_concatenator: SceneConcatenatorConfig::new(&scenes_directory),
                    target_quality:     None,
                    bitrate_optimizer:  BitrateOptimizerConfig::default(),
                    speed_scaler:       SpeedScalerConfig::default(),
                },
            },
            input: input.to_path_buf(),
            temp,
            input_filters: Vec::from(&[VapourSynthFilter::Resize {
                scaler: Some(Scaler::Bicubic),
                width:  None,
                height: None,
                format: Some(FFPixelFormat::YUV420P10LE),
            }]),
            scd_input_filters: Vec::new(),
            tq_input_filters: Vec::new(),
        };

        *configuration.condor.encoder.parameters_mut() = EncoderBase::SVTAV1.default_parameters();

        Ok(configuration)
    }

    #[inline]
    pub fn save(&self, path: &Path) -> Result<(), ConfigError> {
        Self::save_data(self, path)?;
        Ok(())
    }

    #[inline]
    pub fn save_data(data: &Configuration, path: &Path) -> Result<(), ConfigError> {
        let mut buffer = vec![];
        let formatter = serde_json::ser::PrettyFormatter::with_indent(b"    ");
        let mut serializer = serde_json::Serializer::with_formatter(&mut buffer, formatter);
        data.serialize(&mut serializer).map_err(ConfigError::Serialize)?;
        let directory = path.parent();
        if let Some(directory) = directory {
            std::fs::create_dir_all(directory).map_err(ConfigError::Save)?;
        }
        std::fs::write(path, buffer).map_err(ConfigError::Save)?;
        Ok(())
    }

    #[inline]
    pub fn load(config_path: &Path) -> Result<Option<Configuration>, ConfigError> {
        if !config_path.exists() {
            return Ok(None);
        }
        let data = std::fs::read_to_string(config_path)
            .map_err(|_| ConfigError::Load(config_path.to_path_buf()))?;
        let data = serde_json::from_str(&data)
            .map_err(|_| ConfigError::Load(config_path.to_path_buf()))?;

        Ok(Some(data))
    }

    #[inline]
    pub fn instantiate_condor(
        &self,
        save_callback: SaveCallback<CliSequenceData, CliSequenceConfig>,
    ) -> Result<Condor<CliSequenceData, CliSequenceConfig>> {
        // let input = Self::instantiate_input_with_filters(&self.condor.input,
        // &self.input_filters)?;
        let input = {
            if matches!(&self.condor.input, InputModel::Video { .. }) {
                Input::from_video(&self.condor.input)?
            } else if self.input_filters.iter().any(|filter| filter.is_script_only()) {
                Self::instantiate_input_with_filters(&self.condor.input, &self.input_filters)?
            } else {
                let filters = self.input_filters.clone();
                let node_modifier: ModifyNode = Box::new(move |core, node| {
                    let mut node = node.expect("node exists");
                    for filter in &filters {
                        node = filter.invoke_plugin_function(core, &node).map_err(|e| {
                            DecoderError::VapoursynthScriptError {
                                cause: e.to_string(),
                            }
                        })?;
                    }

                    Ok(node)
                });
                Input::from_vapoursynth(&self.condor.input, Some(node_modifier))?
            }
        };
        let output = Output::new(&self.condor.output)?;

        let condor = Condor {
            input,
            output,
            encoder: self.condor.encoder.clone(),
            scenes: self.condor.scenes.clone(),
            sequence_config: self.condor.sequence_config.clone(),
            save_callback,
        };

        Ok(condor)
    }

    #[inline]
    pub fn instantiate_input_with_filters(
        input_data: &InputModel,
        filters: &[VapourSynthFilter],
    ) -> Result<Input> {
        let input = {
            match input_data {
                InputModel::Video {
                    ..
                } => Input::from_video(input_data)?,
                InputModel::VapourSynth {
                    path,
                    import_method,
                    // cache_path, // Cache Path not yet supported
                    ..
                } => {
                    const SCRIPT_OUTPUT_INDEX: u8 = 0;
                    const SCRIPT_NODE_NAME: &str = "clip";
                    let mut script = VapourSynthScript::default();
                    let script = {
                        let (dec_import_lines, dec_lines) = match import_method {
                            VapourSynthImportMethod::LSMASHWorks {
                                ..
                            } => LWLibavSource::new(path)
                                .generate_script(SCRIPT_NODE_NAME.to_owned())?,
                            VapourSynthImportMethod::DGDecNV {
                                ..
                            } => {
                                DGSource::new(path).generate_script(SCRIPT_NODE_NAME.to_owned())?
                            },
                            VapourSynthImportMethod::FFMS2 {
                                ..
                            } => Source::new(path).generate_script(SCRIPT_NODE_NAME.to_owned())?,
                            VapourSynthImportMethod::BestSource {
                                ..
                            } => VideoSource::new(path)
                                .generate_script(SCRIPT_NODE_NAME.to_owned())?,
                        };
                        if let Some(dec_import_lines) = dec_import_lines {
                            script.add_imports(dec_import_lines);
                        }
                        script.add_lines(dec_lines);
                        for filter in filters {
                            let (import_lines, filter_lines) =
                                filter.generate_script(SCRIPT_NODE_NAME.to_owned())?;

                            if let Some(import_lines) = import_lines {
                                script.add_imports(import_lines);
                            }
                            script.add_lines(filter_lines);
                        }

                        script.outputs.insert(SCRIPT_OUTPUT_INDEX, SCRIPT_NODE_NAME.to_owned());
                        script
                    };
                    let script_input_data = InputModel::VapourSynthScript {
                        source:    VapourSynthScriptSource::Text(script.to_string()),
                        variables: HashMap::new(),
                        index:     SCRIPT_OUTPUT_INDEX,
                    };

                    Input::from_vapoursynth(&script_input_data, None)?
                },
                InputModel::VapourSynthScript {
                    ..
                } => Input::from_data(input_data)?,
            }
        };

        Ok(input)
    }

    #[inline]
    pub fn new_input_model(
        input: &Path,
        decoder: Option<&DecoderMethod>,
        vs_args: Option<&[String]>,
        index: Option<u8>,
        // cache_path: Option<&Path>, // TODO: Support Cache Path
    ) -> Result<InputModel> {
        if Self::input_is_script(input) {
            return Self::new_vs_input_model(input, None, vs_args, index);
        }

        let input_model = match decoder {
            Some(DecoderMethod::FFMS2) => InputModel::Video {
                path:          input.to_path_buf(),
                import_method: andean_condor::models::input::ImportMethod::FFMS2 {
                    index,
                },
            },
            Some(decoder) => Self::new_vs_input_model(
                input,
                match decoder {
                    DecoderMethod::VSFFMS2 => Some(VapourSynthImportMethod::FFMS2 {
                        index,
                    }),
                    DecoderMethod::BestSource => Some(VapourSynthImportMethod::BestSource {
                        index,
                    }),
                    DecoderMethod::DGDecodeNV => Some(VapourSynthImportMethod::DGDecNV {
                        dgindexnv_executable: None,
                    }),
                    DecoderMethod::LSMASHWorks => Some(VapourSynthImportMethod::LSMASHWorks {
                        index,
                    }),
                    _ => unreachable!(),
                },
                vs_args,
                index,
            )?,
            None => Self::new_vs_input_model(input, None, vs_args, index)?,
        };

        Ok(input_model)
    }

    #[inline]
    pub fn new_vs_input_model(
        input: &Path,
        decoder: Option<VapourSynthImportMethod>,
        vs_args: Option<&[String]>,
        index: Option<u8>,
    ) -> Result<InputModel> {
        let input_data = if Self::input_is_script(input) {
            let variables = vs_args.map_or_else(HashMap::new, |vs_args| {
                vs_args
                    .iter()
                    .map(|arg| {
                        let (key, value) = arg.split_once('=').unwrap_or((arg, ""));
                        (key.to_string(), value.to_string())
                    })
                    .collect()
            });
            InputModel::VapourSynthScript {
                source: VapourSynthScriptSource::Path(input.to_path_buf()),
                variables,
                index: index.unwrap_or_default(),
            }
        } else {
            InputModel::VapourSynth {
                path:          input.to_path_buf(),
                import_method: decoder.unwrap_or(VapourSynthImportMethod::BestSource {
                    index,
                }),
                cache_path:    None,
            }
        };

        Ok(input_data)
    }

    /// Returns `true` if the input path is a VapourSynth script, ending with
    /// `.vpy` or `.py`.
    #[inline]
    pub fn input_is_script(input: &Path) -> bool {
        input
            .extension()
            .map(|s| s.to_str())
            .is_some_and(|s| s.is_some_and(|extension| matches!(extension, "vpy" | "py")))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CliSequenceConfig
where
    Self: SequenceConfigHandler
        + BenchmarkerConfigHandler
        + NoiseDetectorConfigHandler
        + NoiseScalerConfigHandler
        + TargetQualityConfigHandler
        + BitrateOptimizerConfigHandler
        + SpeedScalerConfigHandler
        + ParallelEncoderConfigHandler
        + SceneConcatenatorConfigHandler,
{
    pub scene_detector:     SceneDetectorConfig,
    pub noise_detector:     Option<NoiseDetectorConfig>,
    pub noise_scaler:       Option<NoiseScalerConfig>,
    pub benchmarker:        BenchmarkerConfig,
    pub parallel_encoder:   ParallelEncoderConfig,
    pub scene_concatenator: SceneConcatenatorConfig,
    pub target_quality:     Option<TargetQualityConfig>,
    pub bitrate_optimizer:  BitrateOptimizerConfig,
    pub speed_scaler:       SpeedScalerConfig,
}

impl Default for CliSequenceConfig {
    #[inline]
    fn default() -> Self {
        Self {
            scene_detector:     SceneDetectorConfig::default(),
            noise_detector:     None,
            noise_scaler:       None,
            benchmarker:        BenchmarkerConfig::default(),
            parallel_encoder:   ParallelEncoderConfig::default(),
            scene_concatenator: SceneConcatenatorConfig::default(),
            target_quality:     None,
            bitrate_optimizer:  BitrateOptimizerConfig::default(),
            speed_scaler:       SpeedScalerConfig::default(),
        }
    }
}

impl SequenceConfigHandler for CliSequenceConfig {
}

impl BenchmarkerConfigHandler for CliSequenceConfig {
    fn benchmarker(&self) -> Result<&BenchmarkerConfig> {
        Ok(&self.benchmarker)
    }

    fn benchmarker_mut(&mut self) -> Result<&mut BenchmarkerConfig> {
        Ok(&mut self.benchmarker)
    }
}

impl NoiseDetectorConfigHandler for CliSequenceConfig {
    fn noise_detector(&self) -> Result<&Option<NoiseDetectorConfig>> {
        Ok(&self.noise_detector)
    }

    fn noise_detector_mut(&mut self) -> Result<&mut Option<NoiseDetectorConfig>> {
        Ok(&mut self.noise_detector)
    }
}

impl NoiseScalerConfigHandler for CliSequenceConfig {
    fn noise_scaler(&self) -> Result<&Option<NoiseScalerConfig>> {
        Ok(&self.noise_scaler)
    }

    fn noise_scaler_mut(&mut self) -> Result<&mut Option<NoiseScalerConfig>> {
        Ok(&mut self.noise_scaler)
    }
}

impl TargetQualityConfigHandler for CliSequenceConfig {
    fn target_quality(&self) -> Result<&Option<TargetQualityConfig>> {
        Ok(&self.target_quality)
    }

    fn target_quality_mut(&mut self) -> Result<&mut Option<TargetQualityConfig>> {
        Ok(&mut self.target_quality)
    }
}

impl BitrateOptimizerConfigHandler for CliSequenceConfig {
    fn bitrate_optimizer(&self) -> Result<&BitrateOptimizerConfig> {
        Ok(&self.bitrate_optimizer)
    }

    fn bitrate_optimizer_mut(&mut self) -> Result<&mut BitrateOptimizerConfig> {
        Ok(&mut self.bitrate_optimizer)
    }
}

impl SpeedScalerConfigHandler for CliSequenceConfig {
    fn speed_scaler(&self) -> Result<&SpeedScalerConfig> {
        Ok(&self.speed_scaler)
    }

    fn speed_scaler_mut(&mut self) -> Result<&mut SpeedScalerConfig> {
        Ok(&mut self.speed_scaler)
    }
}

impl ParallelEncoderConfigHandler for CliSequenceConfig {
    fn parallel_encoder(&self) -> Result<&ParallelEncoderConfig> {
        Ok(&self.parallel_encoder)
    }

    fn parallel_encoder_mut(&mut self) -> Result<&mut ParallelEncoderConfig> {
        Ok(&mut self.parallel_encoder)
    }
}

impl SceneConcatenatorConfigHandler for CliSequenceConfig {
    fn scene_concatenator(&self) -> Result<&SceneConcatenatorConfig> {
        Ok(&self.scene_concatenator)
    }

    fn scene_concatenator_mut(&mut self) -> Result<&mut SceneConcatenatorConfig> {
        Ok(&mut self.scene_concatenator)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CliSequenceData
where
    Self: SequenceDataHandler
        + SceneDetectorDataHandler
        + NoiseDetectorDataHandler
        + NoiseScalerDataHandler
        + ParallelEncoderDataHandler
        + TargetQualityDataHandler, // + QualityCheckDataHandler,
{
    pub scene_detection:  SceneDetectorData,
    pub noise_detection:  Option<NoiseDetectorData>,
    pub noise_scaling:    Option<NoiseScalerData>,
    pub parallel_encoder: ParallelEncoderData,
    pub target_quality:   TargetQualityData,
    // pub quality_check:   QualityCheckData,
}

impl SequenceDataHandler for CliSequenceData {
}

impl Default for CliSequenceData {
    #[inline]
    fn default() -> Self {
        Self {
            scene_detection:  SceneDetectorData::default(),
            noise_detection:  None,
            noise_scaling:    None,
            parallel_encoder: ParallelEncoderData::default(),
            target_quality:   TargetQualityData::default(),
            // quality_check:   QualityCheckData::default(),
        }
    }
}

impl SceneDetectorDataHandler for CliSequenceData {
    fn get_scene_detection(&self) -> Result<&SceneDetectorData> {
        Ok(&self.scene_detection)
    }

    fn get_scene_detection_mut(&mut self) -> Result<&mut SceneDetectorData> {
        Ok(&mut self.scene_detection)
    }
}

impl NoiseDetectorDataHandler for CliSequenceData {
    fn get_noise_detection(&self) -> Result<&Option<NoiseDetectorData>> {
        Ok(&self.noise_detection)
    }

    fn get_noise_detection_mut(&mut self) -> Result<&mut Option<NoiseDetectorData>> {
        Ok(&mut self.noise_detection)
    }
}

impl NoiseScalerDataHandler for CliSequenceData {
    fn get_noise_scaling(&self) -> Result<&Option<NoiseScalerData>> {
        Ok(&self.noise_scaling)
    }

    fn get_noise_scaling_mut(&mut self) -> Result<&mut Option<NoiseScalerData>> {
        Ok(&mut self.noise_scaling)
    }
}

impl ParallelEncoderDataHandler for CliSequenceData {
    fn get_parallel_encoder(&self) -> Result<&ParallelEncoderData> {
        Ok(&self.parallel_encoder)
    }

    fn get_parallel_encoder_mut(&mut self) -> Result<&mut ParallelEncoderData> {
        Ok(&mut self.parallel_encoder)
    }
}

impl TargetQualityDataHandler for CliSequenceData {
    fn get_target_quality(&self) -> Result<&TargetQualityData> {
        Ok(&self.target_quality)
    }

    fn get_target_quality_mut(&mut self) -> Result<&mut TargetQualityData> {
        Ok(&mut self.target_quality)
    }
}

// impl QualityCheckDataHandler for DefaultProcessData {
//     #[inline]
//     fn quality(&self) -> Result<&QualityPass> {
//         Ok(&self.quality_check.quality)
//     }

//     #[inline]
//     fn quality_mut(&mut self) -> Result<&mut QualityPass> {
//         Ok(&mut self.quality_check.quality)
//     }
// }

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("Failed to load config file: {0}")]
    Load(PathBuf),
    #[error("Failed to serialize config file: {0}")]
    Serialize(#[from] serde_json::Error),
    #[error("Failed to save config file: {0}")]
    Save(#[from] std::io::Error),
}
