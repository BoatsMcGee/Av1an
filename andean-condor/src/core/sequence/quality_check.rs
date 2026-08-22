use std::{
    collections::HashMap,
    sync::{self, Arc, atomic::AtomicBool},
    thread,
    time::SystemTime,
};

use anyhow::{Result, bail};
use thiserror::Error;
use tracing::error;

use crate::{
    core::{
        Condor,
        input::Input,
        sequence::{Sequence, SequenceCompletion, SequenceDetails, SequenceStatus, Status},
    },
    models::{
        input::{Input as InputModel, VapourSynthScriptSource},
        sequence::{
            SequenceConfigHandler,
            SequenceDataHandler,
            quality_check::{
                QualityCheckConfig,
                QualityCheckConfigHandler,
                QualityCheckDataHandler,
            },
            target_quality::types::{QualityMetric, QualityPass},
        },
    },
    vapoursynth::{
        VapourSynthError,
        get_core,
        plugins::{
            MetricPluginFunction,
            PluginFunction,
            ffms2::Source,
            resize::bicubic::Bicubic,
            standard::{splice::Splice, trim::Trim},
            vship::{
                butteraugli::BUTTERAUGLI,
                cvvdp::CVVDP,
                ssimulacra2::SSIMULACRA2 as VSHIPSSIMULACRA2,
            },
            vszip::{ssimulacra2::SSIMULACRA2, xpsnr::XPSNR},
        },
        script_builder::{VapourSynthPluginScript, script::VapourSynthScript},
    },
};

static DETAILS: SequenceDetails = SequenceDetails {
    name:        "Quality Check",
    description: "Measure the quality of the video per scene",
    version:     "0.0.1",
};

pub struct QualityCheck {
    pub input: Option<Input>,
}

impl<DataHandler, ConfigHandler> Sequence<DataHandler, ConfigHandler> for QualityCheck
where
    DataHandler: SequenceDataHandler + QualityCheckDataHandler,
    ConfigHandler: SequenceConfigHandler + QualityCheckConfigHandler,
{
    #[inline]
    fn details(&self) -> SequenceDetails {
        DETAILS
    }

    #[inline]
    fn validate(
        &mut self,
        condor: &mut Condor<DataHandler, ConfigHandler>,
    ) -> Result<((), Vec<anyhow::Error>)> {
        let mut warnings = vec![];

        if condor.scenes.is_empty() {
            warnings.push(anyhow::Error::new(QualityCheckError::ScenesEmpty));
        }
        if !condor.output.path.exists() {
            warnings.push(anyhow::Error::new(QualityCheckError::OutputMissing {
                path: condor.output.path.clone(),
            }));
        }

        Ok(((), warnings))
    }

    #[inline]
    fn initialize(
        &mut self,
        condor: &mut Condor<DataHandler, ConfigHandler>,
        progress_tx: sync::mpsc::Sender<SequenceStatus>,
    ) -> Result<((), Vec<anyhow::Error>)> {
        let warnings = vec![];

        let config = condor.sequence_config.quality_check()?;
        let input = if let Some(input) = self.input.as_mut() {
            input
        } else if let Some(config) = config
            && let Some(input_model) = &config.input
        {
            &mut Input::from_data(input_model)?
        } else {
            &mut condor.input
        };
        // Initialize input by getting clip_info. For VapourSynth inputs, this may begin
        // a lengthy caching process, hence the separation between validate and
        // initialize.
        progress_tx.send(SequenceStatus::Whole(Status::Processing {
            id:         DETAILS.name.to_owned(),
            completion: SequenceCompletion::Custom {
                name:      DETAILS.name.to_owned(),
                completed: 0.0,
                total:     1.0,
            },
        }))?;
        input.clip_info()?;
        progress_tx.send(SequenceStatus::Whole(Status::Completed {
            id: DETAILS.name.to_owned(),
        }))?;

        Ok(((), warnings))
    }

    #[inline]
    fn execute(
        &mut self,
        condor: &mut Condor<DataHandler, ConfigHandler>,
        progress_tx: sync::mpsc::Sender<SequenceStatus>,
        cancelled: Arc<AtomicBool>,
    ) -> Result<((), Vec<anyhow::Error>)> {
        let mut warnings = vec![];

        let Some(config) = condor.sequence_config.quality_check()? else {
            return Ok(((), warnings));
        };

        if condor.scenes.is_empty() {
            warnings.push(anyhow::Error::new(QualityCheckError::ScenesEmpty));
            return Ok(((), warnings));
        }
        if !condor.output.path.exists() {
            warnings.push(anyhow::Error::new(QualityCheckError::OutputMissing {
                path: condor.output.path.clone(),
            }));
            return Ok(((), warnings));
        }

        let input = if let Some(input) = self.input.as_mut() {
            input
        } else if let Some(input_model) = &config.input {
            &mut Input::from_data(input_model)?
        } else {
            &mut condor.input
        };

        let scene_frame_indices = condor
            .scenes
            .iter()
            .enumerate()
            .filter_map(|(index, scene)| {
                let indices = config.strategy.frame_indices(scene.start_frame, scene.end_frame);
                let complete = scene
                    .sequence_data
                    .get_quality_check()
                    .is_ok_and(|data| data.quality.scores.len() == indices.len());
                if complete {
                    None
                } else {
                    Some((index, indices))
                }
            })
            .collect::<Vec<_>>();

        if scene_frame_indices.is_empty() {
            progress_tx.send(SequenceStatus::Whole(Status::Completed {
                id: DETAILS.name.to_owned(),
            }))?;
            return Ok(((), warnings));
        }

        let measurements = Self::measure(
            input,
            &condor.output.path,
            config,
            &scene_frame_indices,
            progress_tx,
            &cancelled,
        )?;

        for ((index, _), measurement) in scene_frame_indices.iter().zip(measurements) {
            let quality_pass = QualityPass {
                quantizer:    0.0,
                scores:       measurement.scores,
                bitrate:      0.0,
                started_on:   measurement.started_on,
                completed_on: measurement.completed_on,
            };
            condor.scenes[*index].sequence_data.get_quality_check_mut()?.quality = quality_pass;
        }

        let mut data = condor.as_data();
        data.scenes = condor.scenes.clone();
        (condor.save_callback)(data).expect("failed to save data");

        Ok(((), warnings))
    }
}

impl QualityCheck {
    pub const DETAILS: SequenceDetails = DETAILS;

    #[inline]
    pub fn new(input: Option<Input>) -> Self {
        Self {
            input,
        }
    }

    #[inline]
    #[allow(clippy::too_many_arguments)]
    fn measure(
        input: &mut Input,
        output_path: &std::path::Path,
        config: &QualityCheckConfig,
        scene_frame_indices: &[(usize, Vec<usize>)],
        progress_tx: sync::mpsc::Sender<SequenceStatus>,
        cancelled: &Arc<AtomicBool>,
    ) -> Result<Vec<SceneMeasurement>> {
        let v_input = match input.as_data() {
            InputModel::Video {
                path, ..
            } => {
                const SCRIPT_OUTPUT_INDEX: u8 = 0;
                const SCRIPT_NODE_NAME: &str = "clip";
                let mut script = VapourSynthScript::default();
                let script = {
                    let (dec_import_lines, dec_lines) =
                        Source::new(&path).generate_script(SCRIPT_NODE_NAME.to_owned())?;
                    if let Some(dec_import_lines) = dec_import_lines {
                        script.add_imports(dec_import_lines);
                    }
                    script.add_lines(dec_lines);

                    script.outputs.insert(SCRIPT_OUTPUT_INDEX, SCRIPT_NODE_NAME.to_owned());
                    script
                };
                let script_input_data = InputModel::VapourSynthScript {
                    source:    VapourSynthScriptSource::Text(script.to_string()),
                    variables: HashMap::new(),
                    index:     SCRIPT_OUTPUT_INDEX,
                };

                Some(&mut Input::from_vapoursynth(&script_input_data, None)?)
            },
            _ => None,
        };
        let decoder = match input {
            Input::VapourSynth {
                decoder, ..
            }
            | Input::VapourSynthScript {
                decoder, ..
            } => decoder,
            Input::Video {
                ..
            } => v_input.expect("Video Input exists").decoder(),
        };
        let vapoursynth_decoder = decoder.get_vapoursynth_impl().expect("Decoder is VapourSynth");
        let env = &vapoursynth_decoder.env;
        let reference_node = vapoursynth_decoder.get_output(
            vapoursynth_decoder.get_output_index(),
            vapoursynth_decoder.get_node_modifier(),
        )?;
        let core = get_core(env)?;

        let reference_node = {
            let frame_nodes: Vec<_> = scene_frame_indices
                .iter()
                .map(|(_, indices)| {
                    indices
                        .iter()
                        .map(|index| {
                            Trim {
                                first: Some(*index as u32),
                                last: Some(*index as u32),
                                ..Default::default()
                            }
                            .invoke(core, &reference_node)
                        })
                        .collect::<Result<Vec<_>, _>>()
                })
                .collect::<Result<Vec<_>, _>>()?
                .into_iter()
                .flatten()
                .collect();

            Splice::invoke(core, &frame_nodes)?
        };
        let distorted_node = {
            let source_node = Source {
                source: output_path.to_path_buf(),
                ..Default::default()
            }
            .invoke(core)?;
            let frame_nodes: Vec<_> = scene_frame_indices
                .iter()
                .map(|(_, indices)| {
                    indices
                        .iter()
                        .map(|index| {
                            Trim {
                                first: Some(*index as u32),
                                last: Some(*index as u32),
                                ..Default::default()
                            }
                            .invoke(core, &source_node)
                        })
                        .collect::<Result<Vec<_>, _>>()
                })
                .collect::<Result<Vec<_>, _>>()?
                .into_iter()
                .flatten()
                .collect();

            Splice::invoke(core, &frame_nodes)?
        };

        // Map local frame index to the global frame index
        let global_frames = scene_frame_indices
            .iter()
            .flat_map(|(_, indices)| indices.iter().copied())
            .collect::<Vec<_>>();
        let total_frames = global_frames.len();

        // Scene is complete when the last frame is compared
        let mut scene_last_local_index = Vec::with_capacity(scene_frame_indices.len());
        let mut local_index = 0;
        for (_, indices) in scene_frame_indices {
            local_index += indices.len();
            scene_last_local_index.push(local_index - 1);
        }

        let progress_tx_clone = progress_tx.clone();
        let (compare_progress_tx, compare_progress_rx) = sync::mpsc::channel();
        thread::spawn(move || -> Result<()> {
            for progress in compare_progress_rx {
                match progress {
                    SequenceStatus::Whole(Status::Processing {
                        id: _name,
                        completion,
                    }) => {
                        #[allow(clippy::collapsible_match)]
                        if let SequenceCompletion::Frames {
                            completed,
                            total,
                        } = completion
                        {
                            progress_tx_clone.send(SequenceStatus::Whole(Status::Processing {
                                id:         "Compare".to_owned(),
                                completion: SequenceCompletion::Frames {
                                    completed,
                                    total,
                                },
                            }))?;
                        }
                    },
                    SequenceStatus::Whole(Status::Completed {
                        id: _,
                    }) => {
                        progress_tx_clone.send(SequenceStatus::Whole(Status::Completed {
                            id: "Compare".to_owned(),
                        }))?;
                    },
                    _ => (),
                }
            }
            Ok(())
        });

        // Live per-frame score events
        let scene_count = scene_frame_indices.len();
        let scene_state = Arc::new(sync::Mutex::new((
            vec![Vec::new(); scene_count],
            vec![false; scene_count],
            vec![0u128; scene_count],
            vec![0u128; scene_count],
        )));
        let on_frame = {
            let progress_tx = progress_tx.clone();
            let scene_last_local_index = scene_last_local_index.clone();
            let scene_indices =
                scene_frame_indices.iter().map(|(index, _)| *index).collect::<Vec<_>>();
            let statistic = config.statistic;
            let scene_state_clone = Arc::clone(&scene_state);

            move |local_index: usize, score: &f64| -> Result<(), VapourSynthError> {
                let frame = global_frames[local_index];
                let _ = progress_tx.send(SequenceStatus::Subprocess {
                    parent: Status::Processing {
                        id:         DETAILS.name.to_owned(),
                        completion: SequenceCompletion::Custom {
                            name:      DETAILS.name.to_owned(),
                            completed: (local_index + 1) as f64,
                            total:     total_frames as f64,
                        },
                    },
                    child:  Status::Processing {
                        id:         "Quality".to_owned(),
                        completion: SequenceCompletion::FrameScore {
                            frame: frame as u64,
                            score: *score,
                        },
                    },
                });

                let scene_index = scene_last_local_index
                    .iter()
                    .position(|last| *last >= local_index)
                    .expect("local index belongs to a scene");
                let mut scene_state =
                    scene_state_clone.lock().expect("scene state mutex should acquire lock");
                let (scene_scores, scene_completed, scene_started_on, scene_completed_on) =
                    &mut *scene_state;
                if scene_scores[scene_index].is_empty() {
                    scene_started_on[scene_index] = SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .expect("Time is valid")
                        .as_millis();
                }
                scene_scores[scene_index].push(*score);
                if !scene_completed[scene_index]
                    && scene_last_local_index[scene_index] == local_index
                {
                    scene_completed[scene_index] = true;
                    scene_completed_on[scene_index] = SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .expect("Time is valid")
                        .as_millis();
                    let scene_score = statistic.calculate(&scene_scores[scene_index]);
                    let _ = progress_tx.send(SequenceStatus::Subprocess {
                        parent: Status::Processing {
                            id:         DETAILS.name.to_owned(),
                            completion: SequenceCompletion::Custom {
                                name:      DETAILS.name.to_owned(),
                                completed: (local_index + 1) as f64,
                                total:     total_frames as f64,
                            },
                        },
                        child:  Status::Processing {
                            id:         "Quality".to_owned(),
                            completion: SequenceCompletion::SceneQuality {
                                index:     scene_indices[scene_index] as u64,
                                quantizer: 0.0,
                                score:     scene_score,
                                bitrate:   0.0,
                            },
                        },
                    });
                }

                Ok(())
            }
        };

        match &config.metric {
            QualityMetric::VMAF {
                ..
            } => {
                unimplemented!()
            },
            QualityMetric::SSIMULACRA2 {
                resolution,
                threads,
                ..
            } => {
                let (reference_node, distorted_node) = if let Some((width, height)) = *resolution {
                    let resize = Bicubic {
                        width: Some(width),
                        height: Some(height),
                        ..Default::default()
                    };
                    (
                        resize.invoke(core, &reference_node)?,
                        resize.invoke(core, &distorted_node)?,
                    )
                } else {
                    (reference_node, distorted_node)
                };
                if VSHIPSSIMULACRA2::plugin_is_installed(core) {
                    let plugin = VSHIPSSIMULACRA2 {
                        num_stream: threads.map_or(Some(4), |threads| Some(threads as u32)),
                        ..Default::default()
                    };
                    let node = plugin.invoke(core, &reference_node, &distorted_node)?;
                    VSHIPSSIMULACRA2::collect_frame_values(
                        &node,
                        compare_progress_tx,
                        on_frame,
                        |frame| {
                            VSHIPSSIMULACRA2::PROPERTY_NAMES
                                .iter()
                                .find_map(|property_name| {
                                    frame.props().get_float(property_name).ok()
                                })
                                .ok_or_else(|| {
                                    VSHIPSSIMULACRA2::new_error(format!(
                                        "Score not found on any of the following properties: {}",
                                        VSHIPSSIMULACRA2::PROPERTY_NAMES.join(", ")
                                    ))
                                })
                        },
                    )?
                } else if SSIMULACRA2::plugin_is_installed(core) {
                    let node = SSIMULACRA2::invoke(core, &reference_node, &distorted_node)?;
                    SSIMULACRA2::collect_frame_values(
                        &node,
                        compare_progress_tx,
                        on_frame,
                        |frame| {
                            SSIMULACRA2::PROPERTY_NAMES
                                .iter()
                                .find_map(|property_name| {
                                    frame.props().get_float(property_name).ok()
                                })
                                .ok_or_else(|| {
                                    SSIMULACRA2::new_error(format!(
                                        "Score not found on any of the following properties: {}",
                                        SSIMULACRA2::PROPERTY_NAMES.join(", ")
                                    ))
                                })
                        },
                    )?
                } else {
                    error!("No VapourSynth SSIMULACRA2 plugin found");
                    bail!(QualityCheckError::QualityMeasurementFailed);
                }
            },
            QualityMetric::BUTTERAUGLI {
                resolution,
                threads,
                intensity_multiplier,
                norm,
                ..
            } => {
                let (reference_node, distorted_node) = if let Some((width, height)) = *resolution {
                    let resize = Bicubic {
                        width: Some(width),
                        height: Some(height),
                        ..Default::default()
                    };
                    (
                        resize.invoke(core, &reference_node)?,
                        resize.invoke(core, &distorted_node)?,
                    )
                } else {
                    (reference_node, distorted_node)
                };
                let plugin = BUTTERAUGLI {
                    num_stream: threads.map_or(Some(4), |threads| Some(threads as u32)),
                    intensity_multiplier: *intensity_multiplier,
                    q_norm: norm.map(|norm| norm as u32),
                    ..Default::default()
                };
                let node = plugin.invoke(core, &reference_node, &distorted_node)?;
                let property_names = norm.and_then(|_| Some(BUTTERAUGLI::QNORM_PROPERTY_NAMES));
                let property_names = property_names.unwrap_or(BUTTERAUGLI::PROPERTY_NAMES);
                BUTTERAUGLI::collect_frame_values(
                    &node,
                    compare_progress_tx,
                    on_frame,
                    move |frame| {
                        property_names
                            .iter()
                            .find_map(|property_name| frame.props().get_float(property_name).ok())
                            .ok_or_else(|| {
                                BUTTERAUGLI::new_error(format!(
                                    "Score not found on any of the following properties: {}",
                                    property_names.join(", ")
                                ))
                            })
                    },
                )?
            },
            QualityMetric::XPSNR {
                resolution, ..
            } => {
                let (reference_node, distorted_node) = if let Some((width, height)) = *resolution {
                    let resize = Bicubic {
                        width: Some(width),
                        height: Some(height),
                        ..Default::default()
                    };
                    (
                        resize.invoke(core, &reference_node)?,
                        resize.invoke(core, &distorted_node)?,
                    )
                } else {
                    (reference_node, distorted_node)
                };
                let plugin = XPSNR {
                    temporal: Some(false),
                    verbose: Some(false),
                    ..Default::default()
                };
                let node = plugin.invoke(core, &reference_node, &distorted_node)?;
                // XPSNR returns a score per plane, combine them into the weighted XPSNR score.
                XPSNR::collect_frame_values(
                    &node,
                    compare_progress_tx,
                    on_frame,
                    |frame| {
                        let plane_scores = XPSNR::PROPERTY_NAMES
                            .iter()
                            .map(|property_name| {
                                frame.props().get_float(property_name).map_err(|error| {
                                    XPSNR::new_error(format!(
                                        "Score not found on required property \
                                         \"{property_name}\": {error}"
                                    ))
                                })
                            })
                            .collect::<Result<Vec<f64>, _>>()?;
                        match plane_scores.as_slice() {
                            [y, u, v] => Ok(XPSNR::weight_xpsnr(*y, *u, *v)),
                            _ => Err(XPSNR::new_error(
                                "XPSNR should return three plane scores".to_owned(),
                            )),
                        }
                    },
                )?
            },
            QualityMetric::CVVDP {
                resolution,
                display_model,
                resize_to_display,
                disable_temporal,
                ..
            } => {
                let (reference_node, distorted_node) = if let Some((width, height)) = *resolution {
                    let resize = Bicubic {
                        width: Some(width),
                        height: Some(height),
                        ..Default::default()
                    };
                    (
                        resize.invoke(core, &reference_node)?,
                        resize.invoke(core, &distorted_node)?,
                    )
                } else {
                    (reference_node, distorted_node)
                };
                let plugin = CVVDP {
                    model_name: *display_model,
                    resize_to_display: *resize_to_display,
                    disable_temporal: *disable_temporal,
                    ..Default::default()
                };
                let node = plugin.invoke(core, &reference_node, &distorted_node)?;
                CVVDP::collect_frame_values(
                    &node,
                    compare_progress_tx,
                    on_frame,
                    |frame| {
                        CVVDP::PROPERTY_NAMES
                            .iter()
                            .find_map(|property_name| frame.props().get_float(property_name).ok())
                            .ok_or_else(|| {
                                CVVDP::new_error(format!(
                                    "Score not found on any of the following properties: {}",
                                    CVVDP::PROPERTY_NAMES.join(", ")
                                ))
                            })
                    },
                )?
            },
        };

        progress_tx.send(SequenceStatus::Whole(Status::Completed {
            id: "Compare".to_owned(),
        }))?;
        drop(progress_tx);

        if cancelled.load(sync::atomic::Ordering::Relaxed) {
            bail!(QualityCheckError::Cancelled);
        }

        let scene_state = scene_state.lock().expect("scene state mutex should acquire lock");
        let (scene_scores, _scene_completed, scene_started_on, scene_completed_on) = &*scene_state;
        let measurements = (0..scene_frame_indices.len())
            .map(|index| SceneMeasurement {
                scores:       scene_scores[index].clone(),
                started_on:   scene_started_on[index],
                completed_on: scene_completed_on[index],
            })
            .collect::<Vec<_>>();

        Ok(measurements)
    }
}

impl Default for QualityCheck {
    #[inline]
    fn default() -> Self {
        Self {
            input: None
        }
    }
}

struct SceneMeasurement {
    scores:       Vec<f64>,
    started_on:   u128,
    completed_on: u128,
}

#[derive(Debug, Clone, Error)]
pub enum QualityCheckError {
    #[error("No Scenes found")]
    ScenesEmpty,
    #[error("Output file not found: {path}")]
    OutputMissing { path: std::path::PathBuf },
    #[error("Failed to measure quality")]
    QualityMeasurementFailed,
    #[error("Cancelled")]
    Cancelled,
}
