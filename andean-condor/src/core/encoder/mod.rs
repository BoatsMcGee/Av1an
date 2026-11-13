use std::{
    collections::HashMap,
    fmt::Display,
    hash::{DefaultHasher, Hash, Hasher},
    io::{Cursor, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
        mpsc::{self},
    },
    thread,
    time::Instant,
};

use crate::{
    core::process::OwnedProcess,
    utils::cancellation::{Cancellation, Cancelled},
};
use anyhow::{Context, Result, bail};
use av1_grain::{GrainTableSegment, NoiseGenArgs, TransferFunction, generate_photon_noise_params};
use cfg_if::cfg_if;
use nom::AsBytes;
use path_abs::PathInfo;
use thiserror::Error;
use tracing::info;

use crate::{
    core::input::{Input, clip_info::ClipInfo, color_range::ColorRange},
    models::encoder::{
        Encoder, EncoderBase, EncoderPasses, cli_parameter::CLIParameter, photon_noise::PhotonNoise,
    },
};

pub mod capability;
pub mod color_data;
pub mod parse;
pub(crate) mod pipes;
pub(crate) mod spool;

#[cfg(test)]
#[cfg(target_os = "linux")]
pub(crate) mod e12_tests;
pub mod string_or_bytes;
pub use capability::{Container, EncoderCapability};
pub use parse::*;
pub use pipes::{EncoderProcessError, join_worker};
use pipes::{PassWorkers, progress_reader, spawn_reader};
pub use string_or_bytes::StringOrBytes;

const NULL_OUTPUT: &str = if cfg!(windows) { "nul" } else { "/dev/null" };

pub(crate) fn cancellation_result<T>(result: Result<T>, cancellation: &Cancellation) -> Result<T> {
    if cancellation.is_forced() {
        return match result {
            Err(error) => Err(error.context(Cancelled::Forced)),
            Ok(_) => Err(Cancelled::Forced.into()),
        };
    }
    result
}

fn pass_result(
    result: Result<EncoderResult>,
    cancellation: &Cancellation,
) -> Result<EncoderResult> {
    if cancellation.is_forced() {
        let error = match result {
            Ok(result) => anyhow::Error::from(result),
            Err(error) => error,
        };
        return Err(error.context(Cancelled::Forced));
    }
    result
}

fn complete_output(
    result: EncoderResult,
    output: &mut crate::core::process::InterruptedOutput,
) -> Result<EncoderResult> {
    match output.complete() {
        Ok(()) => Ok(result),
        Err(_) => Err(anyhow::Error::from(result).context(Cancelled::Forced)),
    }
}

fn spawn_waiter(
    encoder: OwnedProcess,
    stats: Option<PassStatistics>,
) -> (
    mpsc::Sender<()>,
    thread::JoinHandle<Result<std::process::Output>>,
) {
    let (reap, drained) = mpsc::channel();
    let waiter = thread::spawn(move || {
        let _stats = stats;
        // E01: locals drop in reverse order on recv/wait errors and unwind too.
        let mut encoder = encoder;
        drained.recv().context("Waiting for encoder pipes to drain")?;
        #[cfg(all(test, target_os = "linux"))]
        assert_ne!(
            std::env::var("E12_CASE").as_deref(),
            Ok("stats-unwind"),
            "injected waiter unwind"
        );
        Ok(std::process::Output {
            status: encoder.wait()?,
            stdout: vec![],
            stderr: vec![],
        })
    });
    (reap, waiter)
}

/// Statistics shared by every pass of one attempt. Clones keep owned files alive.
///
/// Use `new(output)` for isolated, automatically cleaned statistics, or `external(base)`
/// for caller-owned files. Keep this context across direct per-pass calls; never create
/// a separate context for pass two. Native path overrides remain caller-owned.
#[derive(Clone, Debug)]
pub struct PassStatistics {
    base: PathBuf,
    _directory: Option<Arc<StatisticsDirectory>>,
}

#[derive(Debug)]
struct StatisticsDirectory(PathBuf);

impl Drop for StatisticsDirectory {
    fn drop(&mut self) {
        if let Err(error) = std::fs::remove_dir_all(&self.0) {
            tracing::warn!(path = %self.0.display(), %error, "Could not remove pass statistics");
        }
    }
}

impl PassStatistics {
    /// Creates a uniquely owned directory under the output's absolute parent.
    /// The parent must already exist. Errors do not start an encoder process.
    #[inline]
    pub fn new(output: &Path) -> Result<Self> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let output = absolute_file_path(output)?;
        let parent = output.parent().context("Output needs a parent directory")?;
        loop {
            let directory = parent.join(format!(
                ".av1an-stats-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            let mut builder = std::fs::DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            // RATIONALE: Atomic create (not exists + create_dir_all) establishes ownership.
            match builder.create(&directory) {
                Ok(()) => {
                    return Ok(Self {
                        base: directory
                            .join(output.file_stem().context("Output needs a basename")?),
                        _directory: Some(Arc::new(StatisticsDirectory(directory))),
                    });
                },
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {},
                Err(error) => {
                    return Err(error).with_context(|| {
                        format!("Creating pass statistics under {}", parent.display())
                    });
                },
            }
        }
    }

    /// Uses a caller-owned base without creating or deleting any files.
    #[inline]
    pub fn external(base: &Path) -> Result<Self> {
        Ok(Self {
            base: absolute_file_path(base)?,
            _directory: None,
        })
    }

    /// Absolute base; the encoder appends its native statistics/analysis suffix.
    #[inline]
    pub fn base(&self) -> &Path {
        &self.base
    }
}

fn absolute_file_path(path: &Path) -> Result<PathBuf> {
    let text = path.to_str().context("Encoder paths must be UTF-8")?;
    anyhow::ensure!(!text.contains('\0'), "Encoder paths cannot contain NUL");
    anyhow::ensure!(path.file_name().is_some(), "Encoder path needs a basename");
    std::path::absolute(path).with_context(|| format!("Resolving encoder path {}", path.display()))
}

impl Encoder {
    #[inline]
    pub fn validate(&self) -> Result<()> {
        self.validate_settings()?;
        self.resolve_executable()?;

        Ok(())
    }

    /// Requested binary identity, without filesystem access or PATH lookup.
    /// Returns the stock name only when no custom executable is configured.
    #[inline]
    pub fn executable(&self) -> &Path {
        let (default_name, executable) = match self {
            Encoder::AOM { executable, .. } => ("aomenc", executable),
            Encoder::RAV1E { executable, .. } => ("rav1e", executable),
            Encoder::VPX { executable, .. } => ("vpxenc", executable),
            Encoder::SVTAV1 { executable, .. } => ("SvtAv1EncApp", executable),
            Encoder::AVM { executable, .. } => ("avmenc", executable),
            Encoder::X264 { executable, .. } => ("x264", executable),
            Encoder::X265 { executable, .. } => ("x265", executable),
            Encoder::VVenC { executable, .. } => ("vvencapp", executable),
            Encoder::FFmpeg { executable, .. } => ("ffmpeg", executable),
        };

        executable.as_deref().unwrap_or_else(|| Path::new(default_name))
    }

    /// Resolves the requested path or PATH name, never substituting a stock binary.
    /// Fails when the requested binary is missing, invalid, or not executable.
    #[inline]
    #[allow(
        clippy::unnecessary_debug_formatting,
        reason = "Escape control characters and invalid OS path bytes in diagnostics"
    )]
    pub fn resolve_executable(&self) -> Result<PathBuf> {
        which::which(self.executable()).with_context(|| {
            format!(
                "Could not resolve {} encoder executable {:?}",
                self.base(),
                self.executable()
            )
        })
    }

    /// Checks if the encoder binary supports a specific capability,
    /// using the global capability cache to avoid repeated expensive probes.
    #[inline]
    pub fn supports_capability(&self, capability: EncoderCapability) -> bool {
        let Ok(executable_path) = self.resolve_executable() else {
            return false;
        };

        capability::check_capability(self.base(), &executable_path, capability)
    }

    #[inline]
    pub fn supports_capability_cancel(
        &self,
        capability: EncoderCapability,
        cancellation: &Cancellation,
    ) -> Result<bool> {
        cancellation.check_forced()?;
        let executable = self.resolve_executable()?;
        capability::check_capability_cancel(self.base(), &executable, capability, cancellation)
    }

    /// The container this encoder binary writes, or `None` when it cannot be
    /// split per scene (Matroska/WebM).
    ///
    /// A probe result is cached, so this is cheap after the first call.
    #[inline]
    pub fn container(&self) -> Option<Container> {
        let executable_path = self.resolve_executable().ok()?;

        capability::encoder_container(self.base(), &executable_path)
    }

    #[inline]
    pub fn container_cancel(&self, cancellation: &Cancellation) -> Result<Option<Container>> {
        cancellation.check_forced()?;
        let executable = self.resolve_executable()?;
        capability::encoder_container_cancel(self.base(), &executable, cancellation)
    }

    #[inline]
    /// Builds argv without creating files. The output-derived statistics base is
    /// caller-owned and shared across calls; use `parameters_with_stats` for isolation.
    pub fn parameters_with_output(
        &self,
        pass: (u8, u8),
        output: &Path,
        clip_info: &ClipInfo,
    ) -> Result<Vec<String>> {
        self.validate_settings()?;
        self.validate_pass_settings(pass)?;
        let base = absolute_file_path(output)?.with_extension("");
        self.parameters_with_stats(pass, output, clip_info, &base)
    }

    /// Builds argv using an explicit shared statistics base; does not own or create it.
    /// User-provided native statistics options override this generated base unchanged.
    #[inline]
    pub fn parameters_with_stats(
        &self,
        pass: (u8, u8),
        output: &Path,
        clip_info: &ClipInfo,
        stats_base: &Path,
    ) -> Result<Vec<String>> {
        self.validate_settings()?;
        self.validate_pass_settings(pass)?;
        absolute_file_path(output)?;
        let output_path = output.to_str().context("Encoder output must be UTF-8")?;
        let stats_base = absolute_file_path(stats_base)?;
        let pass_parameters = self.base().pass_parameters(
            pass,
            stats_base.to_str().context("Statistics base must be UTF-8")?,
        )?;
        let color_parameters = if self.append_color_data() {
            color_data::color_parameters(self.base(), clip_info)
        } else {
            HashMap::new()
        };

        Ok(match self {
            Encoder::AOM { options, .. } => {
                let mut parameters = ["-".to_owned()].to_vec();
                let mut params = pass_parameters;
                params.extend(color_parameters);
                params.extend(options.clone());
                params.extend(CLIParameter::new_strings(
                    "-",
                    " ",
                    &[(
                        "o",
                        if pass.0 == pass.1 {
                            output_path
                        } else {
                            NULL_OUTPUT
                        },
                    )],
                ));
                for (key, value) in params {
                    match value.to_string_pair(&key) {
                        (Some(prefixed_key), Some(separate_value)) => {
                            parameters.push(prefixed_key);
                            parameters.push(separate_value);
                        },
                        (Some(prefixed_and_delimited), None) => {
                            parameters.push(prefixed_and_delimited);
                        },
                        _ => (), // Boolean/Flag set to false, nothing to push
                    }
                }

                parameters
            },
            Encoder::RAV1E { options, .. } => {
                let mut parameters = ["-".to_owned()].to_vec();
                let mut params = pass_parameters;
                params.extend(color_parameters);
                params.extend(options.clone());
                params.extend(CLIParameter::new_strings(
                    "--",
                    " ",
                    &[(
                        "output",
                        if pass.0 == pass.1 {
                            output_path
                        } else {
                            NULL_OUTPUT
                        },
                    )],
                ));
                for (key, value) in params {
                    match value.to_string_pair(&key) {
                        (Some(prefixed_key), Some(separate_value)) => {
                            parameters.push(prefixed_key);
                            parameters.push(separate_value);
                        },
                        (Some(prefixed_and_delimited), None) => {
                            parameters.push(prefixed_and_delimited);
                        },
                        _ => (), // Boolean/Flag set to false, nothing to push
                    }
                }

                parameters
            },
            Encoder::VPX { options, .. } => {
                let mut parameters = ["-".to_owned()].to_vec();
                let mut params = pass_parameters;
                // The generated VPX color map is VP9-only; raw user options still win below.
                if !matches!(options.get("codec"), Some(CLIParameter::String { value, .. }) if value == "vp8")
                {
                    params.extend(color_parameters);
                }
                params.extend(options.clone());
                params.extend(CLIParameter::new_strings(
                    "-",
                    " ",
                    &[(
                        "o",
                        if pass.0 == pass.1 {
                            output_path
                        } else {
                            NULL_OUTPUT
                        },
                    )],
                ));
                for (key, value) in params {
                    match value.to_string_pair(&key) {
                        (Some(prefixed_key), Some(separate_value)) => {
                            parameters.push(prefixed_key);
                            parameters.push(separate_value);
                        },
                        (Some(prefixed_and_delimited), None) => {
                            parameters.push(prefixed_and_delimited);
                        },
                        _ => (), // Boolean/Flag set to false, nothing to push
                    }
                }

                // Chunk consumers require IVF, even when user options select WebM.
                parameters.push("--ivf".to_owned());
                parameters
            },
            Encoder::SVTAV1 { options, .. } => {
                let mut parameters = Vec::new();
                let mut params = pass_parameters;
                params.extend(color_parameters);
                params.extend(options.clone());
                // Force progress 2. Progress 0-1 is unsupported
                params.extend(CLIParameter::new_numbers("--", " ", &[("progress", 2.0)]));
                params.extend(CLIParameter::new_strings(
                    "-",
                    " ",
                    &[
                        ("i", "stdin"),
                        (
                            "b",
                            if pass.0 == pass.1 {
                                output_path
                            } else {
                                NULL_OUTPUT
                            },
                        ),
                    ],
                ));
                for (key, value) in params {
                    match value.to_string_pair(&key) {
                        (Some(prefixed_key), Some(separate_value)) => {
                            parameters.push(prefixed_key);
                            parameters.push(separate_value);
                        },
                        (Some(prefixed_and_delimited), None) => {
                            parameters.push(prefixed_and_delimited);
                        },
                        _ => (), // Boolean/Flag set to false, nothing to push
                    }
                }

                parameters
            },
            Encoder::AVM { options, .. } => {
                let mut parameters = ["-".to_owned()].to_vec();
                let mut params = pass_parameters;
                params.extend(color_parameters);
                params.extend(options.clone());
                params.extend(CLIParameter::new_strings(
                    "-",
                    " ",
                    &[(
                        "o",
                        if pass.0 == pass.1 {
                            output_path
                        } else {
                            NULL_OUTPUT
                        },
                    )],
                ));
                for (key, value) in params {
                    match value.to_string_pair(&key) {
                        (Some(prefixed_key), Some(separate_value)) => {
                            parameters.push(prefixed_key);
                            parameters.push(separate_value);
                        },
                        (Some(prefixed_and_delimited), None) => {
                            parameters.push(prefixed_and_delimited);
                        },
                        _ => (), // Boolean/Flag set to false, nothing to push
                    }
                }

                parameters
            },
            Encoder::X264 { options, .. } => {
                let mut parameters = ["-".to_owned()].to_vec();
                let mut params = pass_parameters;
                params.extend(color_parameters);
                params.extend(options.clone());
                params.extend(CLIParameter::new_strings(
                    "-",
                    " ",
                    &[(
                        "o",
                        if pass.0 == pass.1 {
                            output_path
                        } else {
                            NULL_OUTPUT
                        },
                    )],
                ));
                for (key, value) in params {
                    match value.to_string_pair(&key) {
                        (Some(prefixed_key), Some(separate_value)) => {
                            parameters.push(prefixed_key);
                            parameters.push(separate_value);
                        },
                        (Some(prefixed_and_delimited), None) => {
                            parameters.push(prefixed_and_delimited);
                        },
                        _ => (), // Boolean/Flag set to false, nothing to push
                    }
                }

                parameters
            },
            Encoder::X265 { options, .. } => {
                let mut parameters = ["-".to_owned()].to_vec();
                let mut params = pass_parameters;
                params.extend(color_parameters);
                params.extend(options.clone());
                params.extend(CLIParameter::new_strings(
                    "-",
                    " ",
                    &[(
                        "o",
                        if pass.0 == pass.1 {
                            output_path
                        } else {
                            NULL_OUTPUT
                        },
                    )],
                ));
                for (key, value) in params {
                    match value.to_string_pair(&key) {
                        (Some(prefixed_key), Some(separate_value)) => {
                            parameters.push(prefixed_key);
                            parameters.push(separate_value);
                        },
                        (Some(prefixed_and_delimited), None) => {
                            parameters.push(prefixed_and_delimited);
                        },
                        _ => (), // Boolean/Flag set to false, nothing to push
                    }
                }

                parameters
            },
            Encoder::VVenC { options, .. } => {
                let mut parameters = ["-i".to_owned(), "-".to_owned()].to_vec();
                let mut params = pass_parameters;
                params.extend(color_parameters);
                params.extend(options.clone());
                params.extend(CLIParameter::new_strings(
                    "-",
                    " ",
                    &[(
                        "o",
                        if pass.0 == pass.1 {
                            output_path
                        } else {
                            NULL_OUTPUT
                        },
                    )],
                ));
                for (key, value) in params {
                    match value.to_string_pair(&key) {
                        (Some(prefixed_key), Some(separate_value)) => {
                            parameters.push(prefixed_key);
                            parameters.push(separate_value);
                        },
                        (Some(prefixed_and_delimited), None) => {
                            parameters.push(prefixed_and_delimited);
                        },
                        _ => (), // Boolean/Flag set to false, nothing to push
                    }
                }

                parameters
            },
            Encoder::FFmpeg { options, .. } => {
                let mut params = pass_parameters;
                params.extend(color_parameters);
                params.extend(options.clone());

                // FFmpeg re-derives the encoder context's colour properties
                // from each frame (`fftools/ffmpeg_enc.c`), so applying them
                // as output options is a no-op. They have to describe the
                // input instead, which also keeps a user override effective
                // because `params` already carries their value.
                let mut parameters = Vec::new();
                for key in color_data::ffmpeg_input_parameters() {
                    if let Some(value) = params.remove(*key)
                        && let (Some(prefixed_key), Some(separate_value)) =
                            value.to_string_pair(key)
                    {
                        parameters.push(prefixed_key);
                        parameters.push(separate_value);
                    }
                }

                parameters.push("-i".to_owned());
                parameters.push("-".to_owned());

                if let Some(codec) = params.remove("c:v") {
                    parameters.push("-c:v".to_owned());
                    parameters.push(codec.to_string_value());
                }
                for (key, value) in params {
                    match value.to_string_pair(&key) {
                        (Some(prefixed_key), Some(separate_value)) => {
                            parameters.push(prefixed_key);
                            parameters.push(separate_value);
                        },
                        (Some(prefixed_and_delimited), None) => {
                            parameters.push(prefixed_and_delimited);
                        },
                        _ => (), // Boolean/Flag set to false, nothing to push
                    }
                }

                parameters.push(output_path.to_owned());
                parameters
            },
        })
    }

    /// Automatic multi-pass attempts own isolated statistics. `Specific` reuses
    /// caller-owned output-derived files; use a shared context for isolated manual passes.
    /// Raw output paths do not grant overwrite consent. Explicit native options
    /// retain their meaning; use the final-output sequence for protected publication.
    /// Stops after the first failed pass, once its workers have joined.
    /// A nonzero exit without worker errors returns an `EncoderResult` error.
    #[inline]
    pub fn encode_with_input(
        &self,
        input: &mut Input,
        output: &Path,
        start_frame: usize,
        end_frame: usize,
        progress_tx: mpsc::Sender<EncodeProgress>,
    ) -> Result<EncoderResult> {
        self.encode_with_input_cancel(
            input,
            output,
            start_frame,
            end_frame,
            progress_tx,
            &Cancellation::default(),
        )
    }

    /// Force-capable attempt. Graceful finishes every pass of an admitted scene.
    #[inline]
    #[allow(clippy::too_many_arguments)]
    pub fn encode_with_input_cancel(
        &self,
        input: &mut Input,
        output: &Path,
        start_frame: usize,
        end_frame: usize,
        progress_tx: mpsc::Sender<EncodeProgress>,
        cancellation: &Cancellation,
    ) -> Result<EncoderResult> {
        self.validate()?;
        cancellation.check_admission()?;
        let cancellation = &cancellation.active();
        let mut output_guard = crate::core::process::InterruptedOutput::new(output, cancellation);
        let stats = if matches!(self.passes(), EncoderPasses::All(total) if total > 1) {
            Some(PassStatistics::new(output)?)
        } else {
            None
        };
        let result: Result<EncoderResult> = match self.passes() {
            EncoderPasses::All(1) => Ok(self.encode_pass_with_input_cancel(
                input,
                output,
                start_frame,
                end_frame,
                (1, 1),
                progress_tx,
                None,
                cancellation,
            )?),
            EncoderPasses::Specific(current, total) => Ok(self.encode_pass_with_input_cancel(
                input,
                output,
                start_frame,
                end_frame,
                (current, total),
                progress_tx,
                None,
                cancellation,
            )?),
            EncoderPasses::All(total) => {
                let clip_info = input.clip_info()?;
                let mut result = EncoderResult {
                    encoder: self.base(),
                    parameters: self.parameters_with_output((1, total), output, &clip_info)?,
                    status: std::process::ExitStatus::default(),
                    stdout: StringOrBytes::from(Vec::new()),
                    stderr: StringOrBytes::from(Vec::new()),
                };
                for pass in 1..=total {
                    let progress_tx = progress_tx.clone();
                    result = self.encode_pass_with_input_cancel(
                        input,
                        output,
                        start_frame,
                        end_frame,
                        (pass, total),
                        progress_tx,
                        stats.as_ref(),
                        cancellation,
                    )?;
                }

                Ok(result)
            },
        };
        complete_output(result?, &mut output_guard)
    }

    /// Encodes one pass. Reuse the same `PassStatistics` for all passes of an attempt.
    /// `None` uses caller-owned output-derived files (no automatic cleanup/isolation).
    /// After all pass workers join, a nonzero exit without worker errors returns
    /// an `EncoderResult` error.
    #[inline]
    #[allow(clippy::too_many_arguments)]
    #[allow(
        clippy::unnecessary_debug_formatting,
        reason = "Escape control characters and invalid OS path bytes in diagnostics"
    )]
    pub fn encode_pass_with_input(
        &self,
        input: &mut Input,
        output: &Path,
        start_frame: usize,
        end_frame: usize,
        pass: (u8, u8),
        progress_tx: mpsc::Sender<EncodeProgress>,
        stats: Option<&PassStatistics>,
    ) -> Result<EncoderResult> {
        self.encode_pass_with_input_cancel(
            input,
            output,
            start_frame,
            end_frame,
            pass,
            progress_tx,
            stats,
            &Cancellation::default(),
        )
    }

    /// Force-capable direct pass; manual multipass callers share one token and statistics.
    #[inline]
    #[allow(clippy::too_many_arguments, clippy::unnecessary_debug_formatting)]
    pub fn encode_pass_with_input_cancel(
        &self,
        input: &mut Input,
        output: &Path,
        start_frame: usize,
        end_frame: usize,
        pass: (u8, u8),
        progress_tx: mpsc::Sender<EncodeProgress>,
        stats: Option<&PassStatistics>,
        cancellation: &Cancellation,
    ) -> Result<EncoderResult> {
        self.validate_settings()?;
        self.validate_pass_settings(pass)?;
        let executable = self.resolve_executable()?;
        cancellation.check_admission()?;
        let mut output_guard = crate::core::process::InterruptedOutput::new(output, cancellation);
        let stats_base = stats.map_or_else(
            || absolute_file_path(output).map(|p| p.with_extension("")),
            |s| Ok(s.base().to_path_buf()),
        )?;
        // TODO:
        // - Report mapped errors
        // - Report CPU, memory, disk usage - Performance testing required

        let base_encoder = self.base();
        let clip_info = input.clip_info()?;
        let parameters = self.parameters_with_stats(pass, output, &clip_info, &stats_base)?;
        let y4m_header = input.y4m_header(Some(end_frame - start_frame))?;
        let mut reader = input.frame_reader()?;

        let mut encoder = OwnedProcess::spawn(
            Command::new(&executable)
                .args(&parameters)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped()),
            cancellation,
        )
        .with_context(|| {
            format!(
                "Could not spawn {} encoder executable {:?} (resolved to {executable:?})",
                self.base(),
                self.executable()
            )
        })?;
        let pid = encoder.id();

        // Data produced and consumed throughout
        let mut frames_decoded = 0;
        let frames_encoded = Arc::new(AtomicU64::new(0));
        let (frame_tx, frame_rx) = mpsc::channel();

        let mut stdin = encoder.stdin.take().expect("Encoder should have STDIN");
        let stdout = encoder.stdout.take().expect("Encoder should have STDOUT");
        let stderr = encoder.stderr.take().expect("Encoder should have STDERR");
        let encoder_stdout_thread = spawn_reader(
            stdout,
            progress_reader(
                base_encoder,
                pass,
                progress_tx.clone(),
                true,
                Arc::clone(&frames_encoded),
                None,
            ),
        );
        let encoder_stderr_thread = spawn_reader(
            stderr,
            progress_reader(
                base_encoder,
                pass,
                progress_tx,
                false,
                Arc::clone(&frames_encoded),
                Some(pid),
            ),
        );
        let (reap, encoder_output_thread) = spawn_waiter(encoder, stats.cloned());

        let writer_cancellation = cancellation.clone();
        let encoder_stdin_thread = thread::spawn(move || -> Result<()> {
            stdin.write_all(y4m_header.as_bytes())?;
            let mut frames_sent = 0;

            loop {
                if frames_sent >= (end_frame - start_frame) {
                    break;
                }

                let frame: Vec<u8> = loop {
                    writer_cancellation.check_forced()?;
                    match frame_rx.recv_timeout(std::time::Duration::from_millis(10)) {
                        Ok(frame) => break frame,
                        Err(mpsc::RecvTimeoutError::Timeout) => {},
                        Err(error) => return Err(error.into()),
                    }
                };
                stdin.write_all(frame.as_bytes())?;
                frames_sent += 1;
            }

            stdin.flush()?;
            drop(stdin);
            Ok(())
        });

        // Decode frames in main thread
        let decode_start_time = Instant::now();
        let feed_result = (|| -> Result<()> {
            for index in start_frame..end_frame {
                cancellation.check_forced()?;
                let frame = reader
                    .y4m_frame(index)
                    .with_context(|| format!("Decoding encoder input frame {index}"))?;
                frames_decoded += 1;
                frame_tx.send(frame.into_inner()).context("Sending encoder input frame")?;
            }
            Ok(())
        })();
        // EOF must reach a waiting writer even when decoding or sending failed.
        drop(frame_tx);
        let decode_duration = decode_start_time.elapsed();
        let encoder_result = PassWorkers {
            stdin: encoder_stdin_thread,
            stdout: encoder_stdout_thread,
            stderr: encoder_stderr_thread,
            wait: encoder_output_thread,
            reap: Some(reap),
        }
        .finish(base_encoder, parameters, feed_result);
        let encoder_result = pass_result(encoder_result, cancellation)?;
        let encode_duration = decode_start_time.elapsed();

        if !encoder_result.status.success() {
            return Err(encoder_result.into());
        }

        info!(
            "{}/{} frames decoded/encoded within {}/{} seconds",
            frames_decoded,
            // frames_encoded.lock().expect("mutex should acquire lock"),
            frames_encoded.load(Ordering::Relaxed),
            decode_duration.as_secs_f64(),
            encode_duration.as_secs_f64()
        );

        complete_output(encoder_result, &mut output_guard)
    }

    #[inline]
    /// Runs all requested passes with one isolated, owned statistics context.
    /// Statistics ownership does not grant output overwrite consent. Explicit raw
    /// options retain native behavior, not the final-output sequence's guarantees.
    /// `Specific` is caller-managed: it reuses output-derived statistics without cleanup.
    /// For isolated manual attempts, call `encode_pass_with_stream` with a shared context.
    /// Stops after the first failed pass, once its workers have joined.
    /// A nonzero exit without worker errors returns an `EncoderResult` error.
    /// Automatic multipass streams first capture one uncompressed Y4M spool under
    /// the output parent, then replay bounded chunks. Allow disk space for the full
    /// stream per active attempt; capture completes before encoding starts.
    pub fn encode_with_stream(
        &self,
        frames_rx: crossbeam_channel::Receiver<Cursor<Vec<u8>>>,
        output: &Path,
        progress_tx: mpsc::Sender<EncodeProgress>,
        clip_info: &ClipInfo,
    ) -> Result<EncoderResult> {
        self.encode_with_stream_cancel(
            frames_rx,
            output,
            progress_tx,
            clip_info,
            &Cancellation::default(),
        )
    }

    /// Force-capable streaming attempt; an already graceful token admits no new scene.
    #[inline]
    pub fn encode_with_stream_cancel(
        &self,
        frames_rx: crossbeam_channel::Receiver<Cursor<Vec<u8>>>,
        output: &Path,
        progress_tx: mpsc::Sender<EncodeProgress>,
        clip_info: &ClipInfo,
        cancellation: &Cancellation,
    ) -> Result<EncoderResult> {
        self.encode_with_stream_impl(
            frames_rx,
            output,
            progress_tx,
            clip_info,
            cancellation,
            false,
        )
    }

    /// Only for paths created inside the caller's exclusive attempt TempDir,
    /// retained through all workers/validation. Never pass a caller/final path.
    /// Ownership is supplied by the parallel/serial owner, not inferred from paths
    /// or statistics. This is unrelated to final-output overwrite consent.
    pub(crate) fn encode_owned_with_stream_cancel(
        &self,
        frames_rx: crossbeam_channel::Receiver<Cursor<Vec<u8>>>,
        output: &Path,
        progress_tx: mpsc::Sender<EncodeProgress>,
        clip_info: &ClipInfo,
        cancellation: &Cancellation,
    ) -> Result<EncoderResult> {
        self.encode_with_stream_impl(
            frames_rx,
            output,
            progress_tx,
            clip_info,
            cancellation,
            true,
        )
    }

    fn encode_with_stream_impl(
        &self,
        frames_rx: crossbeam_channel::Receiver<Cursor<Vec<u8>>>,
        output: &Path,
        progress_tx: mpsc::Sender<EncodeProgress>,
        clip_info: &ClipInfo,
        cancellation: &Cancellation,
        owned_output: bool,
    ) -> Result<EncoderResult> {
        self.validate()?;
        cancellation.check_admission()?;
        let cancellation = &cancellation.active();
        let mut output_guard = crate::core::process::InterruptedOutput::new(output, cancellation);
        let result: Result<EncoderResult> = match self.passes() {
            EncoderPasses::All(1) => {
                let _ = progress_tx.send(EncodeProgress {
                    pass: (1, 1),
                    frame: 0,
                    usage: EncodeResourceUsage::default(),
                });
                Ok(self.encode_pass_with_stream_impl(
                    (1, 1),
                    frames_rx,
                    output,
                    progress_tx,
                    clip_info,
                    None,
                    cancellation,
                    owned_output,
                )?)
            },
            EncoderPasses::Specific(current, total) => {
                let _ = progress_tx.send(EncodeProgress {
                    pass: (current, total),
                    frame: 0,
                    usage: EncodeResourceUsage::default(),
                });
                Ok(self.encode_pass_with_stream_impl(
                    (current, total),
                    frames_rx,
                    output,
                    progress_tx,
                    clip_info,
                    None,
                    cancellation,
                    owned_output,
                )?)
            },
            EncoderPasses::All(total) => {
                let stats = PassStatistics::new(output)?;
                let file = spool::create(
                    stats.base().parent().context("Statistics need a parent")?,
                    output,
                )?;
                spool::encode_cancel(
                    file,
                    frames_rx,
                    output,
                    total,
                    cancellation,
                    |pass, frames_rx| {
                        let _ = progress_tx.send(EncodeProgress {
                            pass: (pass, total),
                            frame: 0,
                            usage: EncodeResourceUsage::default(),
                        });
                        self.encode_pass_with_stream_impl(
                            (pass, total),
                            frames_rx,
                            output,
                            progress_tx.clone(),
                            clip_info,
                            Some(&stats),
                            cancellation,
                            owned_output,
                        )
                    },
                )
            },
        };
        complete_output(result?, &mut output_guard)
    }

    #[inline]
    /// Encodes a manual pass; keep `stats` across calls through the final pass.
    /// `None` retains caller-owned output-derived statistics. Explicit native path
    /// overrides are never cleaned. For VVenC `All(1)` plus native `--pass N`, provide
    /// `--rcstatsfile` yourself: the frontend `(1, 1)` tuple generates no stats option.
    /// After all pass workers join, a nonzero exit without worker errors returns
    /// an `EncoderResult` error.
    #[allow(
        clippy::unnecessary_debug_formatting,
        reason = "Escape control characters and invalid OS path bytes in diagnostics"
    )]
    pub fn encode_pass_with_stream(
        &self,
        pass: (u8, u8),
        frames_rx: crossbeam_channel::Receiver<Cursor<Vec<u8>>>,
        output: &Path,
        progress_tx: mpsc::Sender<EncodeProgress>,
        clip_info: &ClipInfo,
        stats: Option<&PassStatistics>,
    ) -> Result<EncoderResult> {
        self.encode_pass_with_stream_cancel(
            pass,
            frames_rx,
            output,
            progress_tx,
            clip_info,
            stats,
            &Cancellation::default(),
        )
    }

    /// Force-capable direct streaming pass. All pipe/wait workers join before return.
    #[inline]
    #[allow(clippy::too_many_arguments, clippy::unnecessary_debug_formatting)]
    pub fn encode_pass_with_stream_cancel(
        &self,
        pass: (u8, u8),
        frames_rx: crossbeam_channel::Receiver<Cursor<Vec<u8>>>,
        output: &Path,
        progress_tx: mpsc::Sender<EncodeProgress>,
        clip_info: &ClipInfo,
        stats: Option<&PassStatistics>,
        cancellation: &Cancellation,
    ) -> Result<EncoderResult> {
        self.encode_pass_with_stream_impl(
            pass,
            frames_rx,
            output,
            progress_tx,
            clip_info,
            stats,
            cancellation,
            false,
        )
    }

    #[allow(clippy::too_many_arguments, clippy::unnecessary_debug_formatting)]
    fn encode_pass_with_stream_impl(
        &self,
        pass: (u8, u8),
        frames_rx: crossbeam_channel::Receiver<Cursor<Vec<u8>>>,
        output: &Path,
        progress_tx: mpsc::Sender<EncodeProgress>,
        clip_info: &ClipInfo,
        stats: Option<&PassStatistics>,
        cancellation: &Cancellation,
        owned_output: bool,
    ) -> Result<EncoderResult> {
        self.validate_settings()?;
        self.validate_pass_settings(pass)?;
        let executable = self.resolve_executable()?;
        cancellation.check_admission()?;
        let mut output_guard = crate::core::process::InterruptedOutput::new(output, cancellation);
        let stats_base = stats.map_or_else(
            || absolute_file_path(output).map(|p| p.with_extension("")),
            |s| Ok(s.base().to_path_buf()),
        )?;
        let mut parameters = self.parameters_with_stats(pass, output, clip_info, &stats_base)?;
        if owned_output && matches!(self, Encoder::FFmpeg { .. }) {
            let emits = |flag| {
                self.parameters()
                    .iter()
                    .any(|(key, value)| value.to_string_pair(key).0.as_deref() == Some(flag))
            };
            anyhow::ensure!(
                !emits("-n"),
                "-n conflicts with owned FFmpeg attempt overwrite"
            );
            if !emits("-y") {
                parameters.insert(0, "-y".to_owned());
            }
        }
        // TODO:
        // - Report mapped errors
        // - Report CPU, memory, disk usage - Done-ish

        let base_encoder = self.base();

        // println!("Encoding with params: {:?}", self.parameters(pass,
        // output)); let mut system = sysinfo::System::new();
        let mut encoder = OwnedProcess::spawn(
            Command::new(&executable)
                .args(&parameters)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped()),
            cancellation,
        )
        .with_context(|| {
            format!(
                "Could not spawn {} encoder executable {:?} (resolved to {executable:?})",
                self.base(),
                self.executable()
            )
        })?;
        // let pid = encoder.id();

        // Data produced and consumed throughout
        let frames_encoded = Arc::new(AtomicU64::new(0));

        let mut stdin = encoder.stdin.take().expect("Encoder should have STDIN");
        let stdout = encoder.stdout.take().expect("Encoder should have STDOUT");
        let stderr = encoder.stderr.take().expect("Encoder should have STDERR");

        let encoder_stdout_thread = spawn_reader(
            stdout,
            progress_reader(
                base_encoder,
                pass,
                progress_tx.clone(),
                true,
                Arc::clone(&frames_encoded),
                None,
            ),
        );
        let encoder_stderr_thread = spawn_reader(
            stderr,
            progress_reader(base_encoder, pass, progress_tx, false, frames_encoded, None),
        );

        let writer_cancellation = cancellation.clone();
        let encoder_stdin_thread = thread::spawn(move || -> Result<()> {
            loop {
                let frame = crossbeam_channel::select_biased! {
                    recv(writer_cancellation.forced()) -> _ => return Err(Cancelled::Forced.into()),
                    recv(frames_rx) -> frame => match frame { Ok(frame) => frame, Err(_) => break },
                };
                stdin.write_all(frame.into_inner().as_bytes())?;
            }

            stdin.flush()?;
            drop(stdin);
            Ok(())
        });

        let (reap, encoder_output_thread) = spawn_waiter(encoder, stats.cloned());
        let encoder_result = PassWorkers {
            stdin: encoder_stdin_thread,
            stdout: encoder_stdout_thread,
            stderr: encoder_stderr_thread,
            wait: encoder_output_thread,
            reap: Some(reap),
        }
        .finish(base_encoder, parameters, Ok(()));
        let encoder_result = pass_result(encoder_result, cancellation)?;
        if !encoder_result.status.success() {
            return Err(encoder_result.into());
        }
        complete_output(encoder_result, &mut output_guard)
    }

    /// Stores the request on variants with a Photon Noise field, including legacy AVM.
    /// Storage is not support: validate_settings rejects unsupported requests; None clears them.
    #[inline]
    pub fn set_photon_noise(&mut self, photon_noise: Option<PhotonNoise>) {
        match self {
            Encoder::AOM {
                photon_noise: pn, ..
            } => *pn = photon_noise,
            Encoder::RAV1E {
                photon_noise: pn, ..
            } => *pn = photon_noise,
            Encoder::SVTAV1 {
                photon_noise: pn, ..
            } => *pn = photon_noise,
            Encoder::AVM {
                photon_noise: pn, ..
            } => *pn = photon_noise,
            _ => {},
        }
    }

    #[inline]
    pub fn generate_photon_noise_table(
        &self,
        width: u32,
        height: u32,
        transfer_function: TransferFunction,
        color_range: Option<ColorRange>,
    ) -> Result<Option<(String, GrainTableSegment)>> {
        self.validate_settings()?;
        let photon_noise = match self {
            Encoder::AOM { photon_noise, .. } => photon_noise.as_ref(),
            Encoder::RAV1E { photon_noise, .. } => photon_noise.as_ref(),
            Encoder::SVTAV1 { photon_noise, .. } => photon_noise.as_ref(),
            _ => None,
        };
        if photon_noise.is_none() {
            return Ok(None);
        }
        let photon_noise = photon_noise.expect("photon noise should exist");
        let width = photon_noise.width.unwrap_or(width);
        let height = photon_noise.height.unwrap_or(height);
        let full_range = matches!(color_range, Some(ColorRange::Full));
        // Normalize overrides so equivalent generation requests share a table.
        let resolved = PhotonNoise {
            width: Some(width),
            height: Some(height),
            ..photon_noise.clone()
        };
        let transfer_tag = match transfer_function {
            TransferFunction::BT1886 => 0_u8,
            TransferFunction::SMPTE2084 => 1_u8,
        };
        let mut hasher = DefaultHasher::new();
        (resolved, transfer_tag, full_range).hash(&mut hasher);
        // Keep unvalidated settings-only legacy files outside this namespace.
        let hash_name = format!("photon-v2-{:x}", hasher.finish());
        let mut params = generate_photon_noise_params(
            0,
            u64::MAX,
            NoiseGenArgs {
                iso_setting: photon_noise.iso,
                width,
                height,
                transfer_function,
                chroma_grain: photon_noise
                    .chroma_iso
                    .is_some_and(|c_iso| c_iso == photon_noise.iso),
                random_seed: None,
                full_range,
            },
        );

        if let Some(chroma_iso) = photon_noise.chroma_iso
            && chroma_iso != photon_noise.iso
        {
            let chroma_params = generate_photon_noise_params(
                0,
                u64::MAX,
                NoiseGenArgs {
                    iso_setting: chroma_iso,
                    width,
                    height,
                    transfer_function,
                    chroma_grain: true,
                    random_seed: None,
                    full_range,
                },
            );
            params.scaling_points_cr = chroma_params.scaling_points_cr;
            params.scaling_points_cb = chroma_params.scaling_points_cb;
        }
        photon_noise.apply_coefficients(&mut params)?;

        Ok(Some((hash_name, params)))
    }

    #[inline]
    pub fn apply_photon_noise_parameters(&mut self, table_path: &Path) -> Result<()> {
        match self {
            Encoder::AOM { options, .. } => {
                options.insert(
                    "film-grain-table".to_owned(),
                    CLIParameter::new_string("--", "=", &format!("{}", table_path.display())),
                );
            },
            Encoder::RAV1E { options, .. } => {
                options.insert(
                    "photon-noise-table".to_owned(),
                    CLIParameter::new_string("--", " ", &format!("{}", table_path.display())),
                );
            },
            Encoder::SVTAV1 { options, .. } => {
                options.insert(
                    "fgs-table".to_owned(),
                    CLIParameter::new_string("--", " ", &format!("{}", table_path.display())),
                );
            },
            _ => bail!(EncoderError::PhotonNoiseNotSupported(self.base())),
        }

        Ok(())
    }

    #[inline]
    pub fn quantizer(&self) -> Option<f64> {
        match self {
            Encoder::AOM { options, .. } => options.get("cq-level").and_then(|p| match p {
                CLIParameter::Number { value, .. } => Some(*value),
                _ => None,
            }),
            Encoder::RAV1E { options, .. } => options.get("quantizer").and_then(|p| match p {
                CLIParameter::Number { value, .. } => Some(*value),
                _ => None,
            }),
            Encoder::SVTAV1 { options, .. } => options.get("crf").and_then(|p| match p {
                CLIParameter::Number { value, .. } => Some(*value),
                _ => None,
            }),
            Encoder::AVM { options, .. } => options.get("qp").and_then(|p| match p {
                CLIParameter::Number { value, .. } => Some(*value),
                _ => None,
            }),
            Encoder::VPX { options, .. } => options.get("cq-level").and_then(|p| match p {
                CLIParameter::Number { value, .. } => Some(*value),
                _ => None,
            }),
            Encoder::X264 { options, .. } => options.get("crf").and_then(|p| match p {
                CLIParameter::Number { value, .. } => Some(*value),
                _ => None,
            }),
            Encoder::X265 { options, .. } => options.get("crf").and_then(|p| match p {
                CLIParameter::Number { value, .. } => Some(*value),
                _ => None,
            }),
            Encoder::VVenC { options, .. } => options.get("qp").and_then(|p| match p {
                CLIParameter::Number { value, .. } => Some(*value),
                _ => None,
            }),
            Encoder::FFmpeg { options, .. } => options.get("crf").and_then(|p| match p {
                CLIParameter::Number { value, .. } => Some(*value),
                _ => None,
            }),
        }
    }

    #[inline]
    pub fn set_quantizer(&mut self, quantizer: f64) {
        match self {
            Encoder::AOM { options, .. } => options.insert(
                "cq-level".to_owned(),
                CLIParameter::new_number("--", "=", quantizer),
            ),
            Encoder::RAV1E { options, .. } => options.insert(
                "quantizer".to_owned(),
                CLIParameter::new_number("--", " ", quantizer),
            ),
            Encoder::SVTAV1 { options, .. } => options.insert(
                "crf".to_owned(),
                CLIParameter::new_number("--", " ", quantizer),
            ),
            Encoder::AVM { options, .. } => options.insert(
                "qp".to_owned(),
                CLIParameter::new_number("--", "=", quantizer),
            ),
            Encoder::VPX { options, .. } => options.insert(
                "cq-level".to_owned(),
                CLIParameter::new_number("--", "=", quantizer),
            ),
            Encoder::X264 { options, .. } => options.insert(
                "crf".to_owned(),
                CLIParameter::new_number("--", " ", quantizer),
            ),
            Encoder::X265 { options, .. } => options.insert(
                "crf".to_owned(),
                CLIParameter::new_number("--", " ", quantizer),
            ),
            Encoder::VVenC { options, .. } => options.insert(
                "qp".to_owned(),
                CLIParameter::new_number("--", " ", quantizer),
            ),
            Encoder::FFmpeg { options, .. } => options.insert(
                "crf".to_owned(),
                CLIParameter::new_number("-", " ", quantizer),
            ),
        };
    }

    #[inline]
    pub fn set_speed(&mut self, speed: i8) {
        match self {
            Encoder::AOM { options, .. } => options.insert(
                "cpu-used".to_owned(),
                CLIParameter::new_number("--", "=", speed as f64),
            ),
            Encoder::RAV1E { options, .. } => options.insert(
                "speed".to_owned(),
                CLIParameter::new_number("--", " ", speed as f64),
            ),
            Encoder::SVTAV1 { options, .. } => options.insert(
                "preset".to_owned(),
                CLIParameter::new_number("--", " ", speed as f64),
            ),
            Encoder::AVM { options, .. } => options.insert(
                "cpu-used".to_owned(),
                CLIParameter::new_number("--", "=", speed as f64),
            ),
            Encoder::VPX { options, .. } => options.insert(
                "cpu-used".to_owned(),
                CLIParameter::new_number("--", "=", speed as f64),
            ),
            Encoder::X264 { options, .. } => options.insert(
                "preset".to_owned(),
                CLIParameter::new_string(
                    "--",
                    " ",
                    match speed {
                        i8::MIN..=0 => "placebo",
                        1 => "veryslow",
                        2 => "slower",
                        3 => "slow",
                        4 => "medium",
                        5 => "fast",
                        6 => "faster",
                        7 => "veryfast",
                        8 => "superfast",
                        9..=i8::MAX => "ultrafast",
                    },
                ),
            ),
            Encoder::X265 { options, .. } => options.insert(
                "preset".to_owned(),
                CLIParameter::new_string(
                    "--",
                    " ",
                    match speed {
                        i8::MIN..=0 => "placebo",
                        1 => "veryslow",
                        2 => "slower",
                        3 => "slow",
                        4 => "medium",
                        5 => "fast",
                        6 => "faster",
                        7 => "veryfast",
                        8 => "superfast",
                        9..=i8::MAX => "ultrafast",
                    },
                ),
            ),
            Encoder::VVenC { options, .. } => options.insert(
                "preset".to_owned(),
                CLIParameter::new_string(
                    "--",
                    " ",
                    match speed {
                        i8::MIN..=0 => "slower",
                        1 => "slow",
                        2 => "medium",
                        3 => "fast",
                        4..=i8::MAX => "faster",
                    },
                ),
            ),
            Encoder::FFmpeg { options, .. } => options.insert(
                "preset".to_owned(),
                CLIParameter::new_number("-", " ", speed as f64),
            ),
        };
    }

    #[inline]
    pub fn speed(&self) -> Option<String> {
        match self {
            Encoder::AOM { options, .. } => options.get("cpu-used").and_then(|p| match p {
                CLIParameter::Number { value, .. } => Some(value.to_string()),
                _ => None,
            }),
            Encoder::RAV1E { options, .. } => options.get("speed").and_then(|p| match p {
                CLIParameter::Number { value, .. } => Some(value.to_string()),
                _ => None,
            }),
            Encoder::SVTAV1 { options, .. } => options.get("preset").and_then(|p| match p {
                CLIParameter::Number { value, .. } => Some(value.to_string()),
                _ => None,
            }),
            Encoder::AVM { options, .. } => options.get("cpu-used").and_then(|p| match p {
                CLIParameter::Number { value, .. } => Some(value.to_string()),
                _ => None,
            }),
            Encoder::VPX { options, .. } => options.get("cpu-used").and_then(|p| match p {
                CLIParameter::Number { value, .. } => Some(value.to_string()),
                _ => None,
            }),
            Encoder::X264 { options, .. } => options.get("preset").and_then(|p| match p {
                CLIParameter::String { value, .. } => Some(value.clone()),
                _ => None,
            }),
            Encoder::X265 { options, .. } => options.get("preset").and_then(|p| match p {
                CLIParameter::String { value, .. } => Some(value.clone()),
                CLIParameter::Number { value, .. } => Some(value.to_string()),
                _ => None,
            }),
            Encoder::VVenC { options, .. } => options.get("preset").and_then(|p| match p {
                CLIParameter::String { value, .. } => Some(value.clone()),
                CLIParameter::Number { value, .. } => Some(value.to_string()),
                _ => None,
            }),
            Encoder::FFmpeg { options, .. } => options.get("preset").and_then(|p| match p {
                CLIParameter::String { value, .. } => Some(value.clone()),
                CLIParameter::Number { value, .. } => Some(value.to_string()),
                _ => None,
            }),
        }
    }

    #[inline]
    pub fn parse_encoded_frames_base(encoder: &EncoderBase, line: &str) -> Option<u64> {
        match encoder {
            EncoderBase::AOM | EncoderBase::VPX | EncoderBase::AVM => {
                cfg_if! {
                    if #[cfg(any(target_arch = "x86", target_arch = "x86_64"))] {
                        if is_x86_feature_detected!("sse4.1") && is_x86_feature_detected!("ssse3") {
                            // SAFETY: We verified that the CPU has the required feature set
                            return unsafe { parse_aom_vpx_frames_sse41(line.as_bytes()) };
                        }
                    }
                }

                parse_aom_vpx_frames(line)
            },
            EncoderBase::RAV1E => parse_rav1e_frames(line),
            EncoderBase::SVTAV1 => parse_svt_av1_frames(line),
            EncoderBase::X264 | EncoderBase::X265 => parse_x26x_frames(line),
            EncoderBase::VVenC => parse_vvc_frames(line),
            EncoderBase::FFmpeg => parse_ffmpeg_frames(line),
        }
    }

    #[inline]
    pub fn parse_encoded_frames(&self, line: &str) -> Option<u64> {
        Self::parse_encoded_frames_base(&self.base(), line)
    }

    #[inline]
    pub fn version_text(&self) -> Option<String> {
        let executable = self.resolve_executable().ok()?;
        match self {
            Encoder::AOM { .. } => {
                let result =
                    Command::new(&executable).arg("--help").stdin(Stdio::null()).output().ok()?;
                let stdout = String::from_utf8_lossy(&result.stdout);
                let version_line = stdout.lines().find(|line| line.starts_with("    av1"))?;
                Some(
                    version_line
                        .split_once('-')
                        .expect("unexpected aom version string format")
                        .1
                        .replace("(default)", "")
                        .trim()
                        .to_string(),
                )
            },
            Encoder::RAV1E { .. } => {
                let result = Command::new(&executable)
                    .arg("--version")
                    .stdin(Stdio::null())
                    .output()
                    .ok()?;
                let stdout = String::from_utf8_lossy(&result.stdout);
                let version_line = stdout.lines().find(|line| line.starts_with("rav1e"))?;
                Some(version_line.to_string())
            },
            Encoder::VPX { .. } => {
                let result =
                    Command::new(&executable).arg("--help").stdin(Stdio::null()).output().ok()?;
                let stdout = String::from_utf8_lossy(&result.stdout);
                let version_line = stdout.lines().find(|line| line.starts_with("    vp9"))?;
                Some(
                    version_line
                        .split_once('-')
                        .expect("unexpected vpx version string format")
                        .1
                        .replace("(default)", "")
                        .trim()
                        .to_string(),
                )
            },
            Encoder::SVTAV1 { .. } => {
                let result = Command::new(&executable)
                    .arg("--version")
                    .stdin(Stdio::null())
                    .output()
                    .ok()?;
                let stdout = String::from_utf8_lossy(&result.stdout);
                let version_line = stdout.lines().find(|line| line.starts_with("SVT-AV1"))?;
                Some(version_line.to_string())
            },
            Encoder::AVM { .. } => {
                let result =
                    Command::new(&executable).arg("--help").stdin(Stdio::null()).output().ok()?;
                let stdout = String::from_utf8_lossy(&result.stdout);
                let version_line = stdout.lines().find(|line| line.starts_with("    av2"))?;
                Some(
                    version_line
                        .split_once('-')
                        .expect("unexpected avm version string format")
                        .1
                        .replace("(default)", "")
                        .trim()
                        .to_string(),
                )
            },
            Encoder::X264 { .. } => {
                let result = Command::new(&executable)
                    .arg("--version")
                    .stdin(Stdio::null())
                    .output()
                    .ok()?;
                let stdout = String::from_utf8_lossy(&result.stdout);
                let version_line = stdout.lines().find(|line| line.starts_with("x264"))?;
                Some(
                    version_line
                        .split_once(" ")
                        .expect("unexpected x264 version string format")
                        .1
                        .trim()
                        .to_string(),
                )
            },
            Encoder::X265 { .. } => {
                let result = Command::new(&executable)
                    .arg("--version")
                    .stdin(Stdio::null())
                    .output()
                    .ok()?;
                let stderr = String::from_utf8_lossy(&result.stderr);
                let version_line = stderr.lines().find(|line| line.starts_with("x265"))?;
                Some(
                    version_line
                        .split_once(':')
                        .expect("unexpected x265 version string format")
                        .1
                        .trim()
                        .to_string(),
                )
            },
            Encoder::VVenC { .. } => {
                let result = Command::new(&executable)
                    .arg("--version")
                    .stdin(Stdio::null())
                    .output()
                    .ok()?;
                let stdout = String::from_utf8_lossy(&result.stdout);
                let version_line = stdout.lines().find(|line| line.starts_with("vvencapp"))?;
                Some(version_line.to_string())
            },
            Encoder::FFmpeg { .. } => {
                let result = Command::new(&executable).stdin(Stdio::null()).output().ok()?;
                let stderr = String::from_utf8_lossy(&result.stderr);
                let version_line = stderr.lines().find(|line| line.starts_with("ffmpeg"))?;
                version_line.split(' ').nth(2).map(String::from)
            },
        }
    }
}

impl Default for Encoder {
    #[inline]
    fn default() -> Self {
        Self::SVTAV1 {
            executable: None,
            pass: EncoderPasses::All(1),
            options: HashMap::new(),
            photon_noise: None,
            ffmpeg_filter: None,
            append_color_data: true,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct EncodeProgress {
    pub pass: (u8, u8),
    pub frame: usize,
    pub usage: EncodeResourceUsage,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct EncodeResourceUsage {
    pub cpu: f32,
    pub memory: u64,
}

#[derive(Debug, Error, Clone)]
pub struct EncoderResult {
    pub encoder: EncoderBase,
    pub parameters: Vec<String>,
    pub status: std::process::ExitStatus,
    pub stdout: StringOrBytes,
    pub stderr: StringOrBytes,
}

impl Display for EncoderResult {
    #[inline]
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Encoder ({} {}) exited with {}\nSTDOUT:\n{:#?}\nSTDERR:\n{:#?}",
            self.encoder,
            self.parameters.join(" "),
            self.status,
            self.stdout,
            self.stderr
        )
    }
}

#[derive(Debug, Clone, Error)]
pub enum EncoderError {
    #[error("Encoder failed: {0}")]
    EncoderFailed(EncoderResult),
    // #[error("encoder failed to start")]
    // EncoderFailedToStart,
    // #[error("encoder failed to finish")]
    // EncoderFailedToFinish,
    // #[error("encoder failed to encode frame")]
    // EncoderFailedToEncodeFrame,
    #[error("Encoder {0} does not support photon noise")]
    PhotonNoiseNotSupported(EncoderBase),
}

#[cfg(test)]
mod e01_tests;

#[cfg(test)]
mod e02_tests;

#[cfg(test)]
mod e03_tests;

#[cfg(test)]
mod e04_tests;

#[cfg(test)]
mod v01_tests;

#[cfg(test)]
mod e25_tests;

#[cfg(test)]
#[cfg(unix)]
mod e06_tests;

#[cfg(test)]
#[cfg(target_os = "linux")]
mod e07_tests;

#[cfg(test)]
#[cfg(target_os = "linux")]
mod e08_tests;

#[cfg(test)]
#[cfg(target_os = "linux")]
mod e27_tests;

#[cfg(test)]
#[cfg(target_os = "linux")]
mod e09_tests;

#[cfg(test)]
mod tests {
    use av_decoders::{ColorPrimaries, ColorSpace, TransferCharacteristic};

    use super::*;

    #[cfg(unix)]
    #[test]
    fn c04_public_preflight_before_spawn_or_receive() {
        use std::{os::unix::fs::PermissionsExt, time::Duration};
        let temp = tempfile::tempdir().expect("fixture");
        let fake = temp.path().join("encoder");
        std::fs::write(
            &fake,
            "#!/bin/sh\nprintf spawned > \"$C04_DIR/spawned\"\nexit 42\n",
        )
        .expect("fake");
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755))
            .expect("executable");
        std::fs::write(temp.path().join("blank.vpy"), "from vapoursynth import core, YUV420P8\ncore.std.BlankClip(width=64,height=64,length=1,format=YUV420P8).set_output()\n").expect("script");
        for route in [
            "input",
            "input-pass",
            "stream",
            "stream-pass",
            "tuple-input",
            "tuple-stream",
            "vvenc-stream",
        ] {
            let mut child = Command::new(std::env::current_exe().expect("test binary"))
                .args(["--exact", "core::encoder::tests::c04_preflight_child", "--nocapture"])
                .env("C04_DIR", temp.path())
                .env("C04_ROUTE", route)
                .spawn()
                .expect("watchdog child");
            let start = Instant::now();
            let status = loop {
                if let Some(status) = child.try_wait().expect("wait") {
                    break status;
                }
                if start.elapsed() > Duration::from_secs(5) {
                    child.kill().expect("kill blocked child");
                    child.wait().expect("reap blocked child");
                    panic!("{route} blocked before validation");
                }
                thread::sleep(Duration::from_millis(10));
            };
            assert!(status.success(), "{route}: child failed");
            assert!(
                !temp.path().join("spawned").exists(),
                "{route}: encoder spawned"
            );
            assert!(
                !temp.path().join("out.ivf").exists(),
                "{route}: output created"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn c04_preflight_child() {
        let Some(dir) = std::env::var_os("C04_DIR") else {
            return;
        };
        let dir = PathBuf::from(dir);
        let route = std::env::var("C04_ROUTE").expect("route");
        let mut encoder = Encoder::default_from_base(
            &if route == "vvenc-stream" {
                EncoderBase::VVenC
            } else {
                EncoderBase::AOM
            },
            false,
        );
        encoder.set_executable(Some(dir.join("encoder")));
        let pass = if route.starts_with("tuple") {
            (3, 3)
        } else {
            (1, 1)
        };
        *encoder.passes_mut().expect("passes") =
            EncoderPasses::All(if route.starts_with("tuple") { 1 } else { 0 });
        if route == "vvenc-stream" {
            *encoder.passes_mut().expect("passes") = EncoderPasses::All(2);
            encoder.parameters_mut().extend(CLIParameter::new_numbers(
                "--",
                " ",
                &[("passes", 1.0), ("bitrate", 1000.0)],
            ));
        }
        let (tx, _rx) = mpsc::channel();
        let output = dir.join("out.ivf");
        let result = if route.contains("input") {
            let model = crate::models::input::Input::VapourSynthScript {
                source: crate::models::input::VapourSynthScriptSource::Path(dir.join("blank.vpy")),
                variables: HashMap::new(),
                index: 0,
                filters: vec![],
                stream_concurrently: false,
            };
            let mut input = Input::from_data(&model).expect("blank input");
            if route == "input" {
                encoder.encode_with_input(&mut input, &output, 0, 1, tx)
            } else {
                encoder.encode_pass_with_input(&mut input, &output, 0, 1, pass, tx, None)
            }
        } else {
            let (_sender, receiver) = crossbeam_channel::unbounded();
            // Keep the sender alive: a misplaced receive must trip the outer watchdog.
            let result = if route == "stream" || route == "vvenc-stream" {
                encoder.encode_with_stream(receiver, &output, tx, &clip_info())
            } else {
                encoder.encode_pass_with_stream(pass, receiver, &output, tx, &clip_info(), None)
            };
            drop(_sender);
            result
        };
        let error = result.expect_err("invalid settings must fail before encoding");
        if route == "vvenc-stream" {
            use crate::models::encoder::EncoderSettingsError;
            assert!(
                matches!(error.downcast_ref::<EncoderSettingsError>(), Some(EncoderSettingsError::UnsupportedMode(EncoderBase::VVenC, reason)) if reason.contains("frontend")),
                "{error:#}"
            );
            let error = encoder
                .parameters_with_output((1, 2), &output, &clip_info())
                .expect_err("first-pass argv must reject the whole-job conflict");
            assert!(
                matches!(
                    error.downcast_ref::<EncoderSettingsError>(),
                    Some(EncoderSettingsError::UnsupportedMode(EncoderBase::VVenC, _))
                ),
                "{error:#}"
            );
            return;
        }
        assert!(error.to_string().contains("pass"), "{error:#}");
    }

    pub(super) fn clip_info() -> ClipInfo {
        ClipInfo {
            num_frames: 24,
            format_info: crate::core::input::pixel_format::PixelFormat::VapourSynth {
                bit_depth: 10,
            },
            frame_rate: av_format::rational::Rational64::new(24, 1),
            resolution: (1920, 1080),
            color_range: None,
            color_primaries: Some(ColorPrimaries::BT2020),
            color_transfer_characteristic: Some(TransferCharacteristic::SMPTE2084),
            color_space: Some(ColorSpace::BT2020NCL),
            chroma_location: None,
            mastering_display: None,
            content_light_level: None,
        }
    }

    fn parameters(encoder: &Encoder) -> Vec<String> {
        encoder
            .parameters_with_output((1, 1), &PathBuf::from("output.ivf"), &clip_info())
            .expect("valid settings")
    }

    /// The value of `flag` in an argv vector built with a separate-value
    /// delimiter.
    fn value_of<'a>(parameters: &'a [String], flag: &str) -> Option<&'a str> {
        parameters
            .iter()
            .position(|argument| argument == flag)
            .and_then(|index| parameters.get(index + 1))
            .map(String::as_str)
    }

    #[test]
    fn e05_vvenc_default_parameters_inherit_y4m() {
        let expected = HashMap::from([(
            "y4m".to_owned(),
            CLIParameter::Bool {
                prefix: "--".to_owned(),
                value: true,
            },
        )]);
        assert_eq!(EncoderBase::VVenC.default_parameters(), expected);
        let encoder = Encoder::default_from_base(&EncoderBase::VVenC, false);
        assert_eq!(encoder.parameters(), &expected);
    }

    #[test]
    fn e05_vvenc_default_one_pass_argv_declares_y4m_pipe() {
        let output = "job space 映像/scene0.vvc";
        let mut encoder = Encoder::default_from_base(&EncoderBase::VVenC, false);
        encoder.set_append_color_data(false);
        let argv = encoder
            .parameters_with_output((1, 1), Path::new(output), &clip_info())
            .expect("argv");
        // HashMap option order can vary; input/output pairs must remain intact.
        assert!(
            argv == ["-i", "-", "--y4m", "-o", output]
                || argv == ["-i", "-", "-o", output, "--y4m"],
            "{argv:?}"
        );
    }

    #[test]
    fn e05_vvenc_y4m_remains_caller_controlled() {
        let output = "job space 映像/scene0.vvc";
        for exclude_defaults in [false, true] {
            let mut encoder = Encoder::default_from_base(&EncoderBase::VVenC, exclude_defaults);
            encoder.set_append_color_data(false);
            if !exclude_defaults {
                encoder
                    .parameters_mut()
                    .insert("y4m".to_owned(), CLIParameter::new_bool("--", false));
            }
            assert_eq!(
                encoder
                    .parameters_with_output((1, 1), Path::new(output), &clip_info())
                    .expect("argv"),
                ["-i", "-", "-o", output],
                "exclude_defaults={exclude_defaults}"
            );
        }
    }

    #[test]
    fn colour_data_is_appended_by_default() {
        let encoder = Encoder::default_from_base(&EncoderBase::SVTAV1, true);
        let parameters = parameters(&encoder);

        assert_eq!(value_of(&parameters, "--color-primaries"), Some("bt2020"));
        assert_eq!(
            value_of(&parameters, "--transfer-characteristics"),
            Some("smpte2084")
        );
        assert_eq!(
            value_of(&parameters, "--matrix-coefficients"),
            Some("bt2020-ncl")
        );
    }

    #[test]
    fn disabling_colour_data_appends_nothing() {
        let mut encoder = Encoder::default_from_base(&EncoderBase::SVTAV1, true);
        encoder.set_append_color_data(false);
        let parameters = parameters(&encoder);

        assert_eq!(value_of(&parameters, "--color-primaries"), None);
        assert_eq!(value_of(&parameters, "--transfer-characteristics"), None);
        assert_eq!(value_of(&parameters, "--matrix-coefficients"), None);
    }

    #[test]
    fn a_user_option_overrides_the_appended_colour_data() {
        let mut encoder = Encoder::default_from_base(&EncoderBase::SVTAV1, true);
        encoder.parameters_mut().insert(
            "color-primaries".to_owned(),
            CLIParameter::new_string("--", " ", "bt709"),
        );

        let parameters = parameters(&encoder);
        assert_eq!(value_of(&parameters, "--color-primaries"), Some("bt709"));
        assert!(
            !parameters.iter().any(|argument| argument == "bt2020"),
            "the appended value must be replaced rather than duplicated"
        );
    }

    #[test]
    fn a_user_option_overrides_the_appended_hdr_metadata() {
        let mut encoder = Encoder::default_from_base(&EncoderBase::X265, true);
        encoder.parameters_mut().insert(
            "max-cll".to_owned(),
            CLIParameter::new_string("--", " ", "400,200"),
        );
        let clip_info = ClipInfo {
            content_light_level: Some(av_decoders::ContentLightLevel {
                max_cll: 1000,
                max_fall: 400,
            }),
            ..clip_info()
        };

        let parameters = encoder
            .parameters_with_output((1, 1), &PathBuf::from("output.hevc"), &clip_info)
            .expect("valid settings");
        assert_eq!(value_of(&parameters, "--max-cll"), Some("400,200"));
    }

    #[test]
    fn colour_data_follows_the_pass_parameters() {
        let mut encoder = Encoder::default_from_base(&EncoderBase::SVTAV1, true);
        encoder
            .passes_mut()
            .expect("svt-av1 takes passes")
            .clone_from(&EncoderPasses::All(2));
        encoder
            .parameters_mut()
            .insert("rc".into(), CLIParameter::new_number("--", " ", 1.0));

        let parameters = encoder
            .parameters_with_output((1, 2), &PathBuf::from("output.ivf"), &clip_info())
            .expect("VBR two-pass");

        // A two-pass SVT-AV1 command carries `--pass 1`; the colour data is
        // merged after it, and both are present.
        assert_eq!(value_of(&parameters, "--pass"), Some("1"));
        assert_eq!(value_of(&parameters, "--color-primaries"), Some("bt2020"));
    }

    #[test]
    fn ffmpeg_colour_data_precedes_the_input() {
        let encoder = Encoder::default_from_base(&EncoderBase::FFmpeg, true);
        let parameters = encoder
            .parameters_with_output(
                (1, 1),
                &PathBuf::from("output.mkv"),
                &ClipInfo {
                    chroma_location: Some(av_decoders::ChromaLocation::Left),
                    ..clip_info()
                },
            )
            .expect("valid settings");
        let input = parameters
            .iter()
            .position(|argument| argument == "-i")
            .expect("ffmpeg reads the Y4M stream from stdin");

        // `fftools/ffmpeg_enc.c` copies the colour properties out of every
        // frame, so these only survive as input options.
        for flag in ["-color_primaries", "-color_trc", "-colorspace", "-chroma_sample_location"] {
            let index = parameters
                .iter()
                .position(|argument| argument == flag)
                .unwrap_or_else(|| panic!("{flag} is missing"));
            assert!(index < input, "{flag} must precede -i");
        }
        assert_eq!(value_of(&parameters, "-color_primaries"), Some("bt2020"));
        assert_eq!(value_of(&parameters, "-color_trc"), Some("smpte2084"));
        assert_eq!(value_of(&parameters, "-colorspace"), Some("bt2020nc"));
    }

    #[test]
    fn a_user_option_overrides_the_appended_ffmpeg_colour_data() {
        let mut encoder = Encoder::default_from_base(&EncoderBase::FFmpeg, true);
        encoder.parameters_mut().insert(
            "color_primaries".to_owned(),
            CLIParameter::new_string("-", " ", "bt709"),
        );

        let parameters = parameters(&encoder);
        assert_eq!(value_of(&parameters, "-color_primaries"), Some("bt709"));
        assert!(
            !parameters.iter().any(|argument| argument == "bt2020"),
            "the appended value must be replaced rather than duplicated"
        );
    }
}
