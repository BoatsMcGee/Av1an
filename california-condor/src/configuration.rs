use std::{
    collections::HashMap,
    fs::{self, File, TryLockError},
    io::{self, Write},
    path::{Component, Path, PathBuf},
};

use andean_condor::{
    core::{Condor, SaveCallback, output::Output},
    ffmpeg::FFPixelFormat,
    models::{
        Condor as CondorModel,
        encoder::{Encoder, EncoderBase},
        input::{
            Ffms2Filter, Input as InputModel, VapourSynthImportMethod, VapourSynthScriptSource,
        },
        output::Output as OutputModel,
        sequence::{
            SequenceConfigHandler, SequenceDataHandler,
            benchmarker::{BenchmarkerConfig, BenchmarkerConfigHandler},
            bitrate_optimizer::{BitrateOptimizerConfig, BitrateOptimizerConfigHandler},
            noise_detector::{
                NoiseDetectorConfig, NoiseDetectorConfigHandler, NoiseDetectorData,
                NoiseDetectorDataHandler,
            },
            noise_scaler::{
                NoiseScalerConfig, NoiseScalerConfigHandler, NoiseScalerData,
                NoiseScalerDataHandler,
            },
            parallel_encoder::{
                ParallelEncoderConfig, ParallelEncoderConfigHandler, ParallelEncoderData,
                ParallelEncoderDataHandler,
            },
            quality_check::{
                QualityCheckConfig, QualityCheckConfigHandler, QualityCheckData,
                QualityCheckDataHandler,
            },
            scene_concatenator::{SceneConcatenatorConfig, SceneConcatenatorConfigHandler},
            scene_detector::{
                DEFAULT_MAX_SCENE_LENGTH_SECONDS, SceneDetectionMethod, SceneDetectorConfig,
                SceneDetectorData, SceneDetectorDataHandler, ScenecutMethod,
            },
            speed_scaler::{SpeedScalerConfig, SpeedScalerConfigHandler},
            target_quality::{
                TargetQualityConfig, TargetQualityConfigHandler, TargetQualityData,
                TargetQualityDataHandler,
            },
        },
    },
    vapoursynth::{plugins::resize::Scaler, vapoursynth_filters::VapourSynthFilter},
};
use anyhow::{Context, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    commands::DecoderMethod,
    tui::open_input,
    utils::hash_path::hash_path,
    zoner::{ZonerConfig, ZonerData},
};

/// Fallback schema URL used when the binary was built without git metadata
/// (e.g. from a source tarball). Normal builds bake the correct URL in via
/// `CONDOR_SCHEMA_URL` in `build.rs`.
pub const CONFIGURATION_SCHEMA_URL: &str =
    "https://github.com/rust-av/Av1an/releases/download/latest/configuration.schema.json";

#[inline]
fn default_schema_url() -> String {
    option_env!("CONDOR_SCHEMA_URL").unwrap_or(CONFIGURATION_SCHEMA_URL).to_owned()
}

/// Exclusive, nonblocking whole-job guard for CLI and direct Rust callers.
///
/// Acquire before loading; use [`Self::path`] for all handlers, loads and saves,
/// and retain through execution, persistence callbacks and worker joins. Low-level
/// APIs do not acquire this guard or protect stale caller-provided snapshots.
/// Closing the file (including process death) releases the OS lock. Never delete
/// the persistent sidecar: doing so can split lock ownership across two inodes.
/// Symlinks resolve to one target; separately named hardlinks are distinct jobs.
#[derive(Debug)]
pub struct JobLock {
    _file: File,
    config_path: PathBuf,
}

impl JobLock {
    /// Resolve the selected config and try to lock its full-basename `.lock` sidecar.
    /// Busy jobs and filesystem/unsupported-lock errors fail without loading a source.
    /// The parent must already exist; callers creating a job prepare it explicitly.
    pub fn acquire(path: &Path) -> Result<Self> {
        let config_path = normalize_config_path(path)
            .with_context(|| format!("normalizing job config `{}`", path.display()))?;
        let mut name =
            config_path.file_name().context("job config needs a filename")?.to_os_string();
        name.push(".lock");
        let lock_path = config_path.with_file_name(name);
        let file = File::options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .with_context(|| {
                format!(
                    "opening job lock `{}` for config `{}`",
                    lock_path.display(),
                    config_path.display()
                )
            })?;
        match file.try_lock() {
            Ok(()) => Ok(Self {
                _file: file,
                config_path,
            }),
            Err(TryLockError::WouldBlock) => anyhow::bail!(
                "job config `{}` is busy: another process owns the job (lock `{}`); retry after it exits; do not delete the lock file",
                config_path.display(),
                lock_path.display()
            ),
            Err(TryLockError::Error(error)) => Err(error).with_context(|| {
                format!(
                    "locking job config `{}` with `{}`",
                    config_path.display(),
                    lock_path.display()
                )
            }),
        }
    }

    /// Stable absolute target to use throughout the guarded transaction.
    pub fn path(&self) -> &Path {
        &self.config_path
    }
}

// Resolve existing components before `..`, including a symlink parent of a new
// config. Only NotFound permits a missing component; dangling symlinks fail closed.
pub(crate) fn normalize_config_path(path: &Path) -> io::Result<PathBuf> {
    let mut resolved = PathBuf::new();
    for component in std::env::current_dir()?.join(path).components() {
        match component {
            Component::Prefix(_) | Component::RootDir => resolved.push(component),
            Component::CurDir => (),
            Component::Normal(_) | Component::ParentDir => {
                match fs::metadata(&resolved) {
                    Ok(meta) if !meta.is_dir() => {
                        return Err(io::Error::new(
                            io::ErrorKind::NotADirectory,
                            resolved.display().to_string(),
                        ));
                    },
                    Ok(_) => (),
                    Err(error) if error.kind() == io::ErrorKind::NotFound => (),
                    Err(error) => return Err(error),
                }
                if component == Component::ParentDir {
                    resolved.pop();
                } else {
                    resolved.push(component);
                    match fs::symlink_metadata(&resolved) {
                        Ok(_) => resolved = fs::canonicalize(&resolved)?,
                        Err(error) if error.kind() == io::ErrorKind::NotFound => (),
                        Err(error) => return Err(error),
                    }
                }
            },
        }
    }
    Ok(resolved)
}

fn stage_config(buffer: &[u8], path: &Path) -> io::Result<tempfile::NamedTempFile> {
    let directory = path.parent().expect("normalized config parent");
    fs::create_dir_all(directory)?;
    let mut staged = tempfile::NamedTempFile::new_in(directory)?;
    staged.write_all(buffer)?;
    staged.flush()?;
    Ok(staged)
}

fn publish_config(staged: tempfile::NamedTempFile, path: &Path) -> io::Result<()> {
    // PersistError owns the scratch file; dropping it cleans only that owned file.
    staged.persist(path).map(|_| ()).map_err(|error| error.error)
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Configuration {
    #[serde(rename = "$schema", default = "default_schema_url")]
    #[schemars(description = "URL of the JSON schema used to validate this configuration file")]
    pub schema: String,
    pub condor: CondorModel<CliSequenceData, CliSequenceConfig>,
    // Duplicated in case Condor instantiates a VapourSynthScript Input
    pub input: PathBuf,
    pub temp: PathBuf,
}

impl Configuration {
    /// Execution admission only; raw load/save remain usable for inspection and explicit reset.
    /// Never stamps results and never rewrites or removes a file on refusal.
    pub fn admit_results(&self) -> Result<()> {
        use andean_condor::models::sequence::provenance;
        let model = &self.condor;
        let pe = &model.sequence_config.parallel_encoder;
        let tq = model.sequence_config.target_quality.as_ref();
        let probe = provenance::probe_input(&model.input, pe, tq);
        let metric = tq.and_then(|c| c.metric_input.as_ref());
        let input = pe.input.as_ref().unwrap_or(&model.input);
        let qc = model.sequence_config.quality_check.as_ref();
        let reference = qc.and_then(|c| c.input.as_ref()).unwrap_or(&model.input);
        let output = model.sequence_config.scene_concatenator.effective_output(&model.output.path);
        for (index, scene) in model.scenes.iter().enumerate() {
            provenance::admit_target(probe, metric, scene, tq, index)?;
            provenance::admit_quality(
                reference,
                output,
                scene,
                qc,
                &scene.sequence_data.quality_check.quality,
                index,
            )?;
            let path = pe.scenes_directory.join(format!(
                "{}.{}",
                andean_condor::core::sequence::parallel_encoder::ParallelEncoder::scene_id(index),
                scene.encoder.output_extension()
            ));
            let data = &scene.sequence_data.parallel_encoder;
            if data.bytes.is_some()
                || data.started_on.is_some()
                || data.completed_on.is_some()
                || path.symlink_metadata().is_ok()
            {
                provenance::admit_encode(input, probe, metric, scene, tq, &path, index)?;
            }
        }
        Ok(())
    }

    /// Admit the effective final destination; offline edits check aliases without existence policy.
    #[inline]
    pub fn validate_output(&self, execution: bool) -> Result<()> {
        let mut sources = andean_condor::core::sequence::scene_concatenator::SceneConcatenator::protected_sources(&self.condor.input, &self.condor.sequence_config, &self.condor.scenes)?;
        sources.push(self.input.clone());
        let concat = &self.condor.sequence_config.scene_concatenator;
        let output = &self.condor.output;
        andean_condor::core::output::admit_output(&output.path, &sources, true, false)?;
        andean_condor::core::output::admit_output(
            concat.effective_output(&output.path),
            &sources,
            !execution || output.overwrite,
            execution,
        )
    }

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
        let input_data = Self::new_input_model(input, decoder, vs_args, None, None)?;
        // The startup open, needed here for the frame rate. This is also what
        // builds a missing index, which `open_input` reports on the log.
        let (_, clip_info) = open_input(
            &input_data,
            &std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        )?;
        let fps = *clip_info.frame_rate.numer() as f64 / *clip_info.frame_rate.denom() as f64;

        let scenes_directory = temp.join("scenes");

        let mut configuration = Self {
            schema: default_schema_url(),
            condor: CondorModel {
                input: input_data,
                output: OutputModel {
                    path: output.to_path_buf(),
                    overwrite: false,
                    tags: HashMap::new(),
                    video_tags: HashMap::new(),
                },
                encoder: Encoder::default(),
                scenes: Vec::new(),
                sequence_config: CliSequenceConfig {
                    scene_detector: SceneDetectorConfig {
                        input: None,
                        method: SceneDetectionMethod::AVSceneChange {
                            minimum_length: fps.round() as usize,
                            maximum_length: DEFAULT_MAX_SCENE_LENGTH_SECONDS as usize
                                * fps.round() as usize,
                            method: ScenecutMethod::Standard,
                            save_scores: false,
                        },
                    },
                    zoner: None,
                    noise_detector: None,
                    noise_scaler: None,
                    benchmarker: BenchmarkerConfig::default(),
                    parallel_encoder: ParallelEncoderConfig::new(&scenes_directory),
                    scene_concatenator: SceneConcatenatorConfig::new(&scenes_directory),
                    target_quality: None,
                    quality_check: None,
                    bitrate_optimizer: BitrateOptimizerConfig::default(),
                    speed_scaler: SpeedScalerConfig::default(),
                    source_path: None,
                },
            },
            input: input.to_path_buf(),
            temp,
        };

        // `init` defaults to encoding 10-bit, so the input carries the
        // conversion rather than the config holding a filter list.
        match &mut configuration.condor.input {
            InputModel::Video { filters, .. } => {
                *filters = vec![Ffms2Filter::OutputFormat {
                    bit_depth: Some(10),
                    chroma: None,
                    width: None,
                    height: None,
                }];
            },
            InputModel::VapourSynth { filters, .. }
            | InputModel::VapourSynthScript { filters, .. } => {
                *filters = vec![VapourSynthFilter::Resize {
                    scaler: Some(Scaler::Bicubic),
                    width: None,
                    height: None,
                    format: Some(FFPixelFormat::YUV420P10LE),
                }];
            },
        }

        *configuration.condor.encoder.parameters_mut() = EncoderBase::SVTAV1.default_parameters();

        Ok(configuration)
    }

    /// Atomically save via an owned unique sibling; no fsync durability guarantee.
    /// Callers must retain a [`JobLock`] acquired before loading the snapshot.
    #[inline]
    pub fn save(&self, path: &Path) -> Result<(), ConfigError> {
        Self::save_data(self, path)?;
        Ok(())
    }

    /// Checks global and per-scene encoders before CLI edits are saved or workers start.
    pub(crate) fn validate_encoder_settings(&self) -> Result<()> {
        self.condor.sequence_config.parallel_encoder.validate()?;
        self.condor.encoder.validate_settings()?;
        for scene in &self.condor.scenes {
            scene.encoder.validate_settings()?;
        }
        Ok(())
    }

    /// Checks loaded or edited Target Quality without opening inputs or resolving binaries.
    pub(crate) fn validate_target_quality(&self) -> Result<()> {
        if let Some(quality) = &self.condor.sequence_config.target_quality {
            quality.validate(&self.condor.encoder)?;
            for (index, scene) in self.condor.scenes.iter().enumerate() {
                quality
                    .validate(&scene.encoder)
                    .map_err(|error| anyhow::anyhow!("Target Quality scene {index}: {error:#}"))?;
            }
        }
        Ok(())
    }

    /// See [`Self::save`]; this does not acquire or reacquire a job lock.
    #[inline]
    pub fn save_data(data: &Configuration, path: &Path) -> Result<(), ConfigError> {
        data.validate_target_quality().map_err(|error| {
            ConfigError::Save(std::io::Error::new(std::io::ErrorKind::InvalidInput, error))
        })?;
        data.condor.sequence_config.parallel_encoder.validate().map_err(|error| {
            ConfigError::Save(std::io::Error::new(std::io::ErrorKind::InvalidInput, error))
        })?;
        let mut buffer = vec![];
        let formatter = serde_json::ser::PrettyFormatter::with_indent(b"    ");
        let mut serializer = serde_json::Serializer::with_formatter(&mut buffer, formatter);
        data.serialize(&mut serializer).map_err(ConfigError::Serialize)?;
        let path = normalize_config_path(path).map_err(ConfigError::Save)?;
        let staged = stage_config(&buffer, &path).map_err(ConfigError::Save)?;
        publish_config(staged, &path).map_err(ConfigError::Save)?;
        Ok(())
    }

    /// Read an atomic snapshot. For an edit/run/save transaction, acquire a
    /// [`JobLock`] first and retain it until all work and callbacks have joined.
    #[inline]
    pub fn load(config_path: &Path) -> Result<Option<Configuration>, ConfigError> {
        if !config_path.exists() {
            return Ok(None);
        }
        let data = std::fs::read_to_string(config_path)
            .map_err(|_| ConfigError::Load(config_path.to_path_buf()))?;
        let data = serde_json::from_str(&data).map_err(ConfigError::Parse)?;

        Ok(Some(data))
    }

    #[inline]
    pub fn instantiate_condor(
        &self,
        save_callback: SaveCallback<CliSequenceData, CliSequenceConfig>,
    ) -> Result<Condor<CliSequenceData, CliSequenceConfig>> {
        self.validate_target_quality()?;
        self.condor.sequence_config.parallel_encoder.validate()?;
        // The input carries its own filters, so every input kind is opened the
        // same way — and opened once here, so the `Input` every later phase
        // reads through `condor.input` is the one indexed here.
        let (input, _clip_info) = open_input(
            &self.condor.input,
            &std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        )?;
        let output = Output::new(&self.condor.output)?;

        let mut sequence_config = self.condor.sequence_config.clone();
        sequence_config.source_path = Some(self.input.clone());
        let condor = Condor {
            input,
            output,
            encoder: self.condor.encoder.clone(),
            scenes: self.condor.scenes.clone(),
            sequence_config,
            save_callback,
        };

        Ok(condor)
    }

    #[inline]
    pub fn new_input_model(
        input: &Path,
        decoder: Option<&DecoderMethod>,
        vs_args: Option<&[String]>,
        index: Option<u8>,
        cache_path: Option<&Path>,
    ) -> Result<InputModel> {
        if Self::input_is_script(input) {
            return Self::new_vs_input_model(input, None, vs_args, index);
        }

        let input_model = match decoder {
            Some(DecoderMethod::FFMS2) => InputModel::Video {
                path: input.to_path_buf(),
                import_method: andean_condor::models::input::ImportMethod::FFMS2 {
                    index,
                    cache_path: cache_path.map(Path::to_path_buf),
                },
                filters: Vec::new(),
            },
            Some(decoder) => Self::new_vs_input_model(
                input,
                match decoder {
                    DecoderMethod::VSFFMS2 => Some(VapourSynthImportMethod::FFMS2 { index }),
                    DecoderMethod::BestSource => {
                        Some(VapourSynthImportMethod::BestSource { index })
                    },
                    DecoderMethod::DGDecodeNV => Some(VapourSynthImportMethod::DGDecNV {
                        dgindexnv_executable: None,
                    }),
                    DecoderMethod::LSMASHWorks => {
                        Some(VapourSynthImportMethod::LSMASHWorks { index })
                    },
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
                filters: Vec::new(),
                stream_concurrently: true,
            }
        } else {
            InputModel::VapourSynth {
                path: input.to_path_buf(),
                import_method: decoder.unwrap_or(VapourSynthImportMethod::BestSource { index }),
                cache_path: None,
                filters: Vec::new(),
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

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CliSequenceConfig
where
    Self: SequenceConfigHandler
        + BenchmarkerConfigHandler
        + NoiseDetectorConfigHandler
        + NoiseScalerConfigHandler
        + TargetQualityConfigHandler
        + QualityCheckConfigHandler
        + BitrateOptimizerConfigHandler
        + SpeedScalerConfigHandler
        + ParallelEncoderConfigHandler
        + SceneConcatenatorConfigHandler,
{
    pub scene_detector: SceneDetectorConfig,
    pub zoner: Option<ZonerConfig>,
    pub noise_detector: Option<NoiseDetectorConfig>,
    pub noise_scaler: Option<NoiseScalerConfig>,
    pub benchmarker: BenchmarkerConfig,
    pub parallel_encoder: ParallelEncoderConfig,
    pub scene_concatenator: SceneConcatenatorConfig,
    pub target_quality: Option<TargetQualityConfig>,
    pub quality_check: Option<QualityCheckConfig>,
    pub bitrate_optimizer: BitrateOptimizerConfig,
    pub speed_scaler: SpeedScalerConfig,
    /// CLI root-path identity retained for final rechecks of inline script jobs.
    /// Runtime-only: Configuration.input remains the saved authority.
    #[serde(skip)]
    pub source_path: Option<PathBuf>,
}

impl Default for CliSequenceConfig {
    #[inline]
    fn default() -> Self {
        Self {
            scene_detector: SceneDetectorConfig::default(),
            zoner: None,
            noise_detector: None,
            noise_scaler: None,
            benchmarker: BenchmarkerConfig::default(),
            parallel_encoder: ParallelEncoderConfig::default(),
            scene_concatenator: SceneConcatenatorConfig::default(),
            target_quality: None,
            quality_check: None,
            bitrate_optimizer: BitrateOptimizerConfig::default(),
            speed_scaler: SpeedScalerConfig::default(),
            source_path: None,
        }
    }
}

impl SequenceConfigHandler for CliSequenceConfig {}

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

impl QualityCheckConfigHandler for CliSequenceConfig {
    fn quality_check(&self) -> Result<&Option<QualityCheckConfig>> {
        Ok(&self.quality_check)
    }

    fn quality_check_mut(&mut self) -> Result<&mut Option<QualityCheckConfig>> {
        Ok(&mut self.quality_check)
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
    fn protected_sources(&self) -> Vec<PathBuf> {
        [
            self.scene_detector.input.as_ref(),
            self.parallel_encoder.input.as_ref(),
            self.noise_detector.as_ref().and_then(|config| config.input.as_ref()),
            self.target_quality.as_ref().and_then(|config| config.input.as_ref()),
            self.target_quality.as_ref().and_then(|config| config.metric_input.as_ref()),
            self.quality_check.as_ref().and_then(|config| config.input.as_ref()),
        ]
        .into_iter()
        .flatten()
        .filter_map(InputModel::path)
        .map(Path::to_path_buf)
        .chain(self.source_path.iter().cloned())
        .collect()
    }

    fn scene_concatenator(&self) -> Result<&SceneConcatenatorConfig> {
        Ok(&self.scene_concatenator)
    }

    fn scene_concatenator_mut(&mut self) -> Result<&mut SceneConcatenatorConfig> {
        Ok(&mut self.scene_concatenator)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CliSequenceData
where
    Self: SequenceDataHandler
        + SceneDetectorDataHandler
        + NoiseDetectorDataHandler
        + NoiseScalerDataHandler
        + ParallelEncoderDataHandler
        + TargetQualityDataHandler
        + QualityCheckDataHandler,
{
    pub scene_detection: SceneDetectorData,
    pub noise_detection: Option<NoiseDetectorData>,
    pub noise_scaling: Option<NoiseScalerData>,
    pub parallel_encoder: ParallelEncoderData,
    pub target_quality: TargetQualityData,
    pub quality_check: QualityCheckData,
    /// The Zoner's record of having processed the list; absent in
    /// configurations written before zoning became idempotent.
    #[serde(default)]
    pub zoner: ZonerData,
}

impl SequenceDataHandler for CliSequenceData {}

impl Default for CliSequenceData {
    #[inline]
    fn default() -> Self {
        Self {
            scene_detection: SceneDetectorData::default(),
            noise_detection: None,
            noise_scaling: None,
            parallel_encoder: ParallelEncoderData::default(),
            target_quality: TargetQualityData::default(),
            quality_check: QualityCheckData::default(),
            zoner: ZonerData::default(),
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

impl QualityCheckDataHandler for CliSequenceData {
    fn get_quality_check(&self) -> Result<&QualityCheckData> {
        Ok(&self.quality_check)
    }

    fn get_quality_check_mut(&mut self) -> Result<&mut QualityCheckData> {
        Ok(&mut self.quality_check)
    }
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("Failed to load config file: {0}")]
    Load(PathBuf),
    #[error("Failed to parse config file: {0}")]
    Parse(serde_json::Error),
    #[error("Failed to serialize/deserialize config file: {0}")]
    Serialize(#[from] serde_json::Error),
    #[error("Failed to save config file: {0}")]
    Save(#[from] std::io::Error),
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        process::{Child, Command, Stdio},
        time::{Duration, Instant},
    };

    fn fixture(directory: &Path) -> Configuration {
        use crate::test_helpers::{TestVideo, default_config};
        let source = directory.join("source.vpy");
        fs::write(&source, "import vapoursynth as vs\nvs.core.std.BlankClip(width=32,height=16,length=2,format=vs.YUV420P8).set_output()\n").expect("tiny native source");
        let video = TestVideo {
            path: source.clone(),
            width: 32,
            height: 16,
            frames: 2,
            fps_rational: (24, 1),
            format: FFPixelFormat::YUV420P,
            scenes: vec![(0, 2)],
        };
        let mut config =
            default_config(&video, &directory.join("out.mkv"), &directory.join("work"));
        config.condor.input = Configuration::new_input_model(&source, None, None, None, None)
            .expect("script model without native open");
        config.condor.scenes = video.mock_scenes(&config.condor.encoder);
        config
    }

    struct Reap(Child);
    impl Drop for Reap {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    fn wait_until(mut ready: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(30);
        while !ready() {
            assert!(
                Instant::now() < deadline,
                "C11 child coordination timed out"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn c11_callback_child() {
        let Some(directory) = std::env::var_os("CONDOR_C11_CHILD") else {
            return;
        };
        let directory = PathBuf::from(directory);
        let guard =
            JobLock::acquire(&directory.join("job.json")).expect("caller-held guard before load");
        let mut config = Configuration::load(guard.path()).expect("load").expect("config");
        config.schema = "C11 callback snapshot".into();
        let path = guard.path().to_path_buf();
        // Native state is created and dropped on its worker, never shared across threads.
        let worker = std::thread::spawn(move || {
            let condor = crate::instantiate_saved_condor(&config, &path)
                .expect("actual production callback");
            condor.save().expect("real callback publication");
            fs::write(directory.join("entered"), b"saved").expect("ready");
            wait_until(|| directory.join("release").exists());
            condor.save().expect("callback before worker exits");
        });
        worker.join().expect("join while caller still holds guard");
        drop(guard);
    }

    #[test]
    fn c11_direct_guard_covers_real_callback_and_join() {
        let temp = tempfile::tempdir().expect("fixture");
        let config = fixture(temp.path());
        let file = temp.path().join("job.json");
        config.save(&file).expect("initial");
        #[cfg(unix)]
        let before = {
            use std::os::unix::fs::MetadataExt;
            fs::metadata(&file).expect("inode").ino()
        };
        let mut child = Reap(
            Command::new(std::env::current_exe().expect("test executable"))
                .current_dir(temp.path())
                .args(["--exact", "configuration::tests::c11_callback_child", "--nocapture"])
                .env("CONDOR_C11_CHILD", temp.path())
                .stdout(Stdio::null())
                .stderr(File::create(temp.path().join("child.log")).expect("log"))
                .spawn()
                .expect("callback child"),
        );
        wait_until(|| {
            assert!(
                child.0.try_wait().expect("status").is_none(),
                "callback child exited before ready"
            );
            temp.path().join("entered").exists()
        });
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            assert_ne!(fs::metadata(&file).expect("replaced inode").ino(), before);
        }
        assert_eq!(
            Configuration::load(&file).expect("snapshot").expect("saved").schema,
            "C11 callback snapshot"
        );
        let error = JobLock::acquire(&file).expect_err("busy after callback while joining");
        assert!(error.to_string().contains("another process"), "{error:#}");
        let independent =
            JobLock::acquire(&temp.path().join("job.other")).expect("full basename independence");
        drop(independent);
        fs::write(temp.path().join("release"), b"join").expect("release");
        wait_until(|| child.0.try_wait().expect("status").is_some());
        assert!(child.0.wait().expect("reap").success());
        let guard = JobLock::acquire(&file).expect("normal return releases persistent sidecar");
        assert_eq!(
            Configuration::load(guard.path()).expect("snapshot").expect("saved").schema,
            "C11 callback snapshot"
        );
    }

    #[test]
    fn c11_unique_staging_and_failed_publication_preserve_original_bytes() {
        let temp = tempfile::tempdir().expect("fixture");
        let config = fixture(temp.path());
        let file = temp.path().join("job.json");
        config.save(&file).expect("initial config");
        let original = fs::read(&file).expect("snapshot");
        std::thread::scope(|scope| {
            // Both owned files remain alive regardless of which staging call runs first.
            let first =
                scope.spawn(|| stage_config(b"first", &file).expect("first production staging"));
            let second = stage_config(b"second", &file).expect("second production staging");
            let first = first.join().expect("stage thread");
            assert_ne!(first.path(), second.path());
            assert_eq!(fs::read(first.path()).expect("first staged"), b"first");
            assert_eq!(fs::read(second.path()).expect("second staged"), b"second");
            fs::remove_file(first.path())
                .expect("force native publish failure without chmod assumptions");
            assert_eq!(
                publish_config(first, &file).expect_err("missing staged file").kind(),
                io::ErrorKind::NotFound
            );
            assert_eq!(fs::read(&file).expect("original preserved"), original);
            assert!(
                second.path().exists(),
                "failure only cleans its owned staging"
            );
        });
        let mut invalid = config;
        invalid.condor.sequence_config.parallel_encoder.max_tries = 0;
        assert!(invalid.save(&file).is_err(), "validation before staging");
        assert_eq!(fs::read(&file).expect("unchanged JSON"), original);
        invalid.condor.sequence_config.parallel_encoder.max_tries = 1;
        invalid.condor.scenes[0].sequence_data.noise_detection = Some(NoiseDetectorData {
            noise: 1.0,
            luminance: 1.0,
            created_on: std::time::SystemTime::UNIX_EPOCH - Duration::from_secs(1),
        });
        assert!(
            matches!(invalid.save(&file), Err(ConfigError::Serialize(_))),
            "real serialization failure before staging"
        );
        assert_eq!(
            fs::read(&file).expect("serialization preserves original bytes"),
            original
        );
        assert_eq!(
            fs::read_dir(temp.path()).expect("owned temps cleaned").count(),
            2
        );
    }

    #[cfg(unix)]
    #[test]
    fn c11_symlink_admission_stays_coherent_after_saves_and_for_new_configs() {
        use std::os::unix::fs::symlink;
        let temp = tempfile::tempdir().expect("fixture");
        let base = fs::canonicalize(temp.path()).expect("base");
        let config = fixture(&base);
        fs::create_dir_all(base.join("real/child")).expect("parent");
        symlink("real/child", base.join("alias-parent")).expect("parent alias");
        let file = base.join("real/job.json");
        let guard = JobLock::acquire(&base.join("alias-parent/../job.json"))
            .expect("new path through symlink and dotdot");
        assert_eq!(guard.path(), file);
        config.save(guard.path()).expect("save under guard");
        symlink("real/job.json", base.join("alias.json")).expect("file alias");
        for alias in ["alias.json", "real/./job.json", "alias-parent/../job.json"] {
            assert!(
                JobLock::acquire(&base.join(alias))
                    .expect_err("same target")
                    .to_string()
                    .contains("another process")
            );
            config.save(&base.join(alias)).expect("low-level saves resolve same target");
            assert!(
                JobLock::acquire(&base.join(alias)).is_err(),
                "save must not split identity"
            );
        }
        assert!(
            fs::symlink_metadata(base.join("alias.json"))
                .expect("alias retained")
                .is_symlink()
        );
        drop(guard);
        let guard = JobLock::acquire(&base.join("alias.json")).expect("reopen same sidecar");
        assert_eq!(guard.path(), file);
    }

    #[test]
    fn c11_lock_open_errors_fail_closed_with_context() {
        let temp = tempfile::tempdir().expect("fixture");
        let file = temp.path().join("job.json");
        fs::create_dir_all(temp.path().join("job.json.lock"))
            .expect("directory blocks sidecar open");
        let error = JobLock::acquire(&file).expect_err("no unprotected fallback");
        let message = format!("{error:#}");
        assert!(
            message.contains("opening job lock")
                && message.contains("job.json.lock")
                && message.contains("job.json"),
            "{message}"
        );
        assert!(!file.exists());
    }

    #[test]
    fn c11_direct_guard_does_not_prepare_missing_parents() {
        let temp = tempfile::tempdir().expect("fixture");
        let file = temp.path().join("missing/deep/job.json");
        let result = JobLock::acquire(&file);
        assert!(
            !temp.path().join("missing").exists(),
            "generic acquisition must not create parents or sidecars"
        );
        let error = result.expect_err("caller must prepare a new job's parent deliberately");
        assert!(format!("{error:#}").contains("job.json.lock"));
        assert!(error.chain().any(|cause| {
            cause
                .downcast_ref::<io::Error>()
                .is_some_and(|error| error.kind() == io::ErrorKind::NotFound)
        }));
        fs::create_dir_all(file.parent().expect("parent")).expect("caller prepares parent");
        let guard = JobLock::acquire(&file).expect("explicit new job under an existing parent");
        assert_eq!(guard.path(), file);
        assert!(!file.exists(), "guard does not create the config");
        assert!(file.with_file_name("job.json.lock").is_file());
    }

    #[cfg(unix)]
    #[test]
    fn c11_lock_permission_errors_use_actual_open_probe() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().expect("fixture");
        let file = temp.path().join("job.json");
        let sidecar = temp.path().join("job.json.lock");
        fs::write(&sidecar, b"").expect("persistent sidecar");
        fs::set_permissions(&sidecar, fs::Permissions::from_mode(0o000)).expect("deny access");
        let probe = File::options().read(true).write(true).open(&sidecar);
        let result = JobLock::acquire(&file);
        fs::set_permissions(&sidecar, fs::Permissions::from_mode(0o600))
            .expect("restore before assertions");
        match probe {
            Err(error) => {
                assert_eq!(
                    error.kind(),
                    io::ErrorKind::PermissionDenied,
                    "actual open probe"
                );
                let error = result.expect_err("denied opens cannot fall back");
                assert!(format!("{error:#}").contains("job.json.lock"));
                assert!(error.chain().any(|cause| {
                    cause
                        .downcast_ref::<io::Error>()
                        .is_some_and(|error| error.kind() == io::ErrorKind::PermissionDenied)
                }));
                eprintln!("C11 actual sidecar open denied; contextual refusal proved");
            },
            Ok(_) => {
                drop(result.expect("actual open allowed; kernel locking still required"));
                eprintln!(
                    "C11 chmod did not deny actual sidecar open; permission-denial proof unavailable on this host"
                );
            },
        }
        assert!(!file.exists());
    }
}
