use std::{
    collections::BTreeMap,
    io::IsTerminal,
};

use andean_condor::{
    core::{
        input::clip_info::ClipInfo,
        sequence::{SequenceCompletion, SequenceStatus, Status},
    },
    models::{encoder::Encoder, scene::Scene},
};
use ratatui::{
    Frame,
    layout::{Constraint, Layout},
    style::Color,
    text::Line,
    widgets::Block,
};
use serde::{Deserialize, Serialize};

use crate::{
    apps::{SharedProgress, TuiApp},
    components::{
        active_encoders::ActiveEncoders,
        encoder_info::EncoderInfo,
        input_info::InputInfo,
        progress_bar::ProgressBar,
    },
    configuration::CliSequenceData,
};

#[derive(Clone)]
pub struct ParallelEncoderState {
    pub scenes:                 BTreeMap<u64, (u64, Scene<CliSequenceData>)>,
    pub active_encoders:        BTreeMap<u64, SceneEncoder>,
    pub clip_info:              ClipInfo,
    pub completed_scenes_count: usize,
    pub estimated_bitrate:      f64,
    pub estimated_bytes:        u64,
}

pub struct ParallelEncoderApp {
    pub(crate) original_panic_hook: Option<super::PanicHook>,
    pub started:                    std::time::Instant,
    pub workers:                    u8,
    pub encoder:                    Encoder,
    pub initial_frames:             u64,
    pub total_frames:               u64,
    pub clip_info:                  ClipInfo,
    attempted_cancel:               bool,
    shared_progress:                SharedProgress<ParallelEncoderState>,
    cached_state:                   ParallelEncoderState,
}

impl TuiApp for ParallelEncoderApp {
    type State = ParallelEncoderState;

    fn original_panic_hook(&mut self) -> &mut Option<super::PanicHook> {
        &mut self.original_panic_hook
    }

    fn shared_progress(&self) -> &SharedProgress<Self::State> {
        &self.shared_progress
    }

    fn cached_state(&self) -> &Self::State {
        &self.cached_state
    }

    fn cached_state_mut(&mut self) -> &mut Self::State {
        &mut self.cached_state
    }

    fn attempted_cancel(&self) -> bool {
        self.attempted_cancel
    }

    fn attempted_cancel_mut(&mut self) -> &mut bool {
        &mut self.attempted_cancel
    }

    fn cancel_message(&self) -> &'static str {
        "Waiting for Encoders to finish. Press Ctrl+C again to exit immediately."
    }

    fn map_progress(status: SequenceStatus, state: &mut Self::State) -> bool {
        match status {
            SequenceStatus::Whole(status) => {
                if let Status::Processing {
                    id,
                    completion,
                } = status
                    && let SequenceCompletion::Custom {
                        name,
                        completed,
                        ..
                    } = completion
                    && name == "size"
                {
                    let scene = id.parse::<u64>().expect("Scene index is a number");
                    let bytes = completed as u64;
                    state.scenes.entry(scene).and_modify(|(_, scene)| {
                        scene.sequence_data.parallel_encoder.bytes = Some(bytes);
                    });
                    state.completed_scenes_count = state
                        .scenes
                        .iter()
                        .filter(|(_, (_, s))| {
                            s.sequence_data.parallel_encoder.bytes.is_some_and(|b| b > 0)
                        })
                        .count();
                    let (bitrate, estimated_bytes) =
                        Self::estimate_size(&state.scenes, &state.clip_info);
                    state.estimated_bitrate = bitrate;
                    state.estimated_bytes = estimated_bytes;
                    if !std::io::stdout().is_terminal() {
                        let event = ParallelEncoderConsoleEvent::SceneSize {
                            scene_index: scene,
                            bytes,
                        };
                        println!(
                            "[Parallel Encoder][Scene Size] {}",
                            serde_json::to_string(&event).unwrap()
                        );
                    }
                    return true;
                }
                false
            },
            SequenceStatus::Subprocess {
                parent: _,
                child,
            } => match child {
                Status::Processing {
                    id,
                    completion:
                        SequenceCompletion::PassFrames {
                            passes,
                            frames,
                        },
                } => {
                    let scene = id.parse::<u64>().expect("Scene index is a number");
                    let (current_pass, total_passes) = passes;
                    let (current_frame, total_frames) = frames;

                    if current_pass == total_passes
                        && state.active_encoders.contains_key(&scene)
                    {
                        state.scenes.entry(scene).and_modify(|(completed, _)| {
                            *completed = current_frame;
                        });
                    }

                    if let Some(active) = state.active_encoders.get_mut(&scene) {
                        active.current_pass = current_pass;
                        active.total_passes = total_passes;
                        active.frames_processed = current_frame;
                        active.total_frames = total_frames;
                    } else if current_frame < total_frames
                        && let Some((_, scene_data)) = state.scenes.get(&scene)
                    {
                        let scene_encoder = SceneEncoder {
                            scene: scene_data.clone(),
                            started: std::time::Instant::now(),
                            current_pass,
                            total_passes,
                            frames_processed: current_frame,
                            total_frames,
                        };
                        state.active_encoders.insert(scene, scene_encoder);
                    }
                    true
                },
                Status::Completed {
                    id,
                } => {
                    let scene = id.parse::<u64>().expect("Scene index is a number");
                    state.scenes.entry(scene).and_modify(|(completed, scene)| {
                        *completed = (scene.end_frame - scene.start_frame) as u64;
                    });
                    state.active_encoders.remove(&scene);
                    state.completed_scenes_count = state
                        .scenes
                        .iter()
                        .filter(|(_, (_, s))| {
                            s.sequence_data.parallel_encoder.bytes.is_some_and(|b| b > 0)
                        })
                        .count();
                    true
                },
                _ => false,
            },
        }
    }

    fn render(&self, frame: &mut Frame) {
        const MAIN_COLOR: Color = Color::DarkGray;
        let layout = Layout::default()
            .constraints([
                Constraint::Percentage(20),
                Constraint::Percentage(70),
                Constraint::Percentage(10),
            ])
            .split(frame.area());

        let total_frames_completed: u64 = self
            .cached_state
            .scenes
            .iter()
            .map(|(_, (completed, _))| completed)
            .sum();
        let total_frames = self.total_frames;
        let top_info = Block::bordered()
            .border_type(ratatui::widgets::BorderType::Rounded)
            .title(Line::from("Input").centered())
            .title_bottom(Line::from(self.encoder.base().friendly_name()).centered());
        let top_info_inner = top_info.inner(layout[0]);
        let top_info_areas =
            Layout::vertical([Constraint::Fill(1), Constraint::Fill(1)]).split(top_info_inner);
        frame.render_widget(top_info, layout[0]);
        let input_info = InputInfo::new(self.clip_info);
        let input_info = input_info.generate(false);
        frame.render_widget(input_info, top_info_areas[0]);
        let encoder_info = EncoderInfo::new(self.encoder.clone(), None);
        let encoder_info = encoder_info.generate(false);
        frame.render_widget(encoder_info, top_info_areas[1]);

        let active_encoders = ActiveEncoders::new(
            MAIN_COLOR,
            self.workers,
            self.encoder.clone(),
            &self.cached_state.active_encoders,
        );
        frame.render_widget(active_encoders, layout[1]);

        let scenes_completed = self.cached_state.completed_scenes_count;
        let progress_bar = ProgressBar {
            color:               MAIN_COLOR,
            processing_title:    if self.attempted_cancel {
                "Shutting down after Encoders complete...".to_owned()
            } else {
                "Encoding Scenes...".to_owned()
            },
            completed_title:     if self.attempted_cancel {
                "Encoding Aborted".to_owned()
            } else {
                "Encoding Completed".to_owned()
            },
            top_right_title:     if self.cached_state.estimated_bytes > 0 {
                format!(
                    "{:.1}kbps est. {:.1} MB",
                    self.cached_state.estimated_bitrate / 1e3,
                    self.cached_state.estimated_bytes as f64 / 1e6
                )
            } else {
                String::new()
            },
            bottom_center_title: format!(
                "{}/{} Scenes",
                scenes_completed,
                self.cached_state.scenes.len()
            ),
            unit_per_second:     "FPS".to_owned(),
            unit:                "Frame".to_owned(),
            initial_completed:   self.initial_frames,
            completed:           total_frames_completed,
            total:               total_frames,
            show_label:          true,
        };
        let progress_bar = progress_bar.generate(Some(self.started));
        frame.render_widget(progress_bar, layout[2]);
    }
}

impl ParallelEncoderApp {
    pub fn new(
        workers: u8,
        encoder: Encoder,
        scenes: BTreeMap<u64, (u64, Scene<CliSequenceData>)>,
        clip_info: ClipInfo,
    ) -> ParallelEncoderApp {
        let total_frames =
            scenes.iter().map(|(_, (_, s))| (s.end_frame - s.start_frame) as u64).sum();

        let completed_scenes_count = scenes
            .iter()
            .filter(|(_, (_, s))| s.sequence_data.parallel_encoder.bytes.is_some_and(|b| b > 0))
            .count();
        let (estimated_bitrate, estimated_bytes) = Self::estimate_size(&scenes, &clip_info);
        let initial_frames = scenes.iter().fold(0, |acc, (_, (completed, _))| acc + completed);
        let initial_state = ParallelEncoderState {
            scenes,
            active_encoders: BTreeMap::new(),
            clip_info,
            completed_scenes_count,
            estimated_bitrate,
            estimated_bytes,
        };

        ParallelEncoderApp {
            original_panic_hook: None,
            started: std::time::Instant::now(),
            workers,
            encoder,
            initial_frames,
            total_frames,
            clip_info: initial_state.clip_info,
            attempted_cancel: false,
            shared_progress: SharedProgress::new(initial_state.clone()),
            cached_state: initial_state,
        }
    }

    pub fn estimate_size(
        scenes: &BTreeMap<u64, (u64, Scene<CliSequenceData>)>,
        clip_info: &ClipInfo,
    ) -> (f64, u64) {
        let total_frames = clip_info.num_frames;
        let (frames_completed, bytes_completed) = scenes
            .iter()
            .filter(|(_, (_, scene))| scene.sequence_data.parallel_encoder.bytes.is_some())
            .fold(
                (0, 0),
                |(frames_completed, bytes_completed), (_, (_, scene))| {
                    (
                        frames_completed + (scene.end_frame - scene.start_frame) as u64,
                        bytes_completed + scene.sequence_data.parallel_encoder.bytes.unwrap_or(0),
                    )
                },
            );
        if frames_completed == 0 {
            return (0.0, 0);
        }
        let framerate = *clip_info.frame_rate.numer() as f64 / *clip_info.frame_rate.denom() as f64;
        let seconds = frames_completed as f64 / framerate;
        let total_seconds = total_frames as f64 / framerate;
        let bitrate = (bytes_completed * 8) as f64 / seconds;
        let estimated_bytes = ((bitrate * total_seconds) / 8.0) as u64;
        (bitrate, estimated_bytes)
    }

}

#[derive(Debug, Clone)]
pub struct SceneEncoder {
    pub scene:            Scene<CliSequenceData>,
    pub started:          std::time::Instant,
    pub current_pass:     u8,
    pub total_passes:     u8,
    pub frames_processed: u64,
    pub total_frames:     u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ParallelEncoderConsoleEvent {
    Progress {
        scene:         SceneProgress,
        current_frame: u64,
        total_frames:  u64,
    },
    NewScene {
        scene_index: u64,
        /// Must be milliseconds since UNIX Epoch
        time:        u128,
    },
    SceneSize {
        scene_index: u64,
        bytes:       u64,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SceneProgress {
    pub index:     u64,
    current_pass:  u8,
    total_passes:  u8,
    current_frame: u64,
    total_frames:  u64,
}
