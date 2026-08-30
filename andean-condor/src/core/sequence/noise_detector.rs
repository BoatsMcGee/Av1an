use std::{
    collections::HashMap,
    sync::{self, Arc, atomic::AtomicBool},
    thread,
    time::SystemTime,
};

use anyhow::Result;
use thiserror::Error;

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
            noise_detector::{
                NoiseDetectorConfigHandler,
                NoiseDetectorData,
                NoiseDetectorDataHandler,
            },
        },
    },
    vapoursynth::{
        get_core,
        plugins::{
            MetricPluginFunction,
            ffms2::Source,
            standard::{plane_stats::PlaneStats, splice::Splice, trim::Trim},
        },
        script_builder::{VapourSynthPluginScript, script::VapourSynthScript},
    },
};
static DETAILS: SequenceDetails = SequenceDetails {
    name:        "Noise Detector",
    description: "Measure the amount of noise of the video per scene",
    version:     "0.0.1",
};

#[derive(Default)]
pub struct NoiseDetector {
    pub input: Option<Input>,
}

impl<Data, Config> Sequence<Data, Config> for NoiseDetector
where
    Data: SequenceDataHandler + NoiseDetectorDataHandler,
    Config: SequenceConfigHandler + NoiseDetectorConfigHandler,
{
    #[inline]
    fn details(&self) -> SequenceDetails {
        DETAILS
    }

    #[inline]
    fn validate(
        &mut self,
        _condor: &mut Condor<Data, Config>,
    ) -> anyhow::Result<((), Vec<anyhow::Error>)> {
        let warnings = vec![];

        Ok(((), warnings))
    }

    #[inline]
    fn initialize(
        &mut self,
        _condor: &mut Condor<Data, Config>,
        _progress_tx: sync::mpsc::Sender<SequenceStatus>,
    ) -> anyhow::Result<((), Vec<anyhow::Error>)> {
        let warnings = vec![];

        Ok(((), warnings))
    }

    #[inline]
    fn execute(
        &mut self,
        condor: &mut Condor<Data, Config>,
        progress_tx: sync::mpsc::Sender<SequenceStatus>,
        _cancelled: Arc<AtomicBool>,
    ) -> anyhow::Result<((), Vec<anyhow::Error>)> {
        let warnings = vec![];
        let mut condor_data = condor.as_data();
        let input = self.input.as_mut().unwrap_or(&mut condor.input);
        let config = condor.sequence_config.noise_detector()?.clone().unwrap_or_default();

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
        let mut reference_node = vapoursynth_decoder.get_output(
            vapoursynth_decoder.get_output_index(),
            vapoursynth_decoder.get_node_modifier(),
        )?;
        let core = get_core(env)?;

        let mut denoised_node = reference_node.clone();

        for filter in &config.reference_filters {
            reference_node = filter.invoke_plugin_function(core, &reference_node)?;
        }
        for filter in &config.denoised_filters {
            denoised_node = filter.invoke_plugin_function(core, &denoised_node)?;
        }

        // Sample 1 frame in the middle of each scene
        let reference_node = {
            let frame_nodes: Vec<_> = condor
                .scenes
                .iter()
                .map(|scene| {
                    let index = scene.start_frame + (scene.end_frame - scene.start_frame) / 2;
                    Trim {
                        first: Some(index as u32),
                        last: Some(index as u32),
                        ..Default::default()
                    }
                    .invoke(core, &reference_node)
                })
                .collect::<Result<Vec<_>, _>>()?;

            Splice::invoke(core, &frame_nodes)?
        };
        let denoised_node = {
            let frame_nodes: Vec<_> = condor
                .scenes
                .iter()
                .map(|scene| {
                    let index = scene.start_frame + (scene.end_frame - scene.start_frame) / 2;
                    Trim {
                        first: Some(index as u32),
                        last: Some(index as u32),
                        ..Default::default()
                    }
                    .invoke(core, &denoised_node)
                })
                .collect::<Result<Vec<_>, _>>()?;

            Splice::invoke(core, &frame_nodes)?
        };

        let (compare_progress_tx, compare_progress_rx) = sync::mpsc::channel();
        thread::spawn(move || -> Result<()> {
            for progress in compare_progress_rx {
                match progress {
                    SequenceStatus::Whole(Status::Processing {
                        id: _fn_name,
                        completion,
                    }) => {
                        #[allow(clippy::collapsible_match)]
                        if let SequenceCompletion::Frames {
                            completed,
                            total,
                        } = completion
                        {
                            progress_tx.send(SequenceStatus::Whole(Status::Processing {
                                id:         Self::DETAILS.name.to_owned(),
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
                        progress_tx.send(SequenceStatus::Whole(Status::Completed {
                            id: Self::DETAILS.name.to_owned(),
                        }))?;
                    },
                    _ => (),
                }
            }
            Ok(())
        });

        let plane_stats_node = PlaneStats {
            clip_b_name: Some("denoised".to_owned()),
            plane:       Some(0),
            prop:        None,
        }
        .call(core, &reference_node, Some(&denoised_node))?;

        let plane_stats = PlaneStats::get_scores(&plane_stats_node, None, compare_progress_tx)?;

        for (plane_stat, scene) in plane_stats.iter().zip(condor.scenes.iter_mut()) {
            let noise_detection = scene.sequence_data.get_noise_detection_mut()?;
            *noise_detection = Some(NoiseDetectorData {
                noise:      *plane_stat,
                luminance:  0.0,
                created_on: SystemTime::now(),
            });
        }
        condor_data.scenes = condor.scenes.clone();

        (condor.save_callback)(condor_data)?;

        Ok(((), warnings))
    }
}

impl NoiseDetector {
    pub const DETAILS: SequenceDetails = DETAILS;
}

#[derive(Debug, Error)]
pub enum NoiseDetectorError {
    #[error("Input must be VapourSynthScript")]
    InvalidInput,
}
