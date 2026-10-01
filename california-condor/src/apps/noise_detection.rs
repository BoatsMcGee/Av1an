use std::{collections::BTreeMap, io::IsTerminal};

use andean_condor::{
    core::{
        input::clip_info::ClipInfo,
        sequence::{SequenceCompletion, SequenceStatus, Status},
    },
    models::scene::Scene,
};
use ratatui::{
    Frame,
    layout::{Constraint, Layout},
    style::Color,
    text::Line,
    widgets::{Axis, Block, Chart, Dataset},
};
use serde::{Deserialize, Serialize};

use crate::{
    apps::{SharedProgress, TuiApp},
    components::{input_info::InputInfo, progress_bar::ProgressBar},
    configuration::CliSequenceData,
};
#[derive(Clone)]
pub struct NoiseDetectionState {
    pub frames_processed: u64,
    pub total_frames:     u64,
    pub scene_noise:      BTreeMap<u64, f64>,
}

pub struct NoiseDetectionApp {
    pub(crate) original_panic_hook: Option<super::PanicHook>,
    pub started:                    std::time::Instant,
    pub total_frames:               u64,
    pub clip_info:                  ClipInfo,
    attempted_cancel:               bool,
    shared_progress:                SharedProgress<NoiseDetectionState>,
    cached_state:                   NoiseDetectionState,
}

impl TuiApp for NoiseDetectionApp {
    type State = NoiseDetectionState;

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
        "Waiting for Noise Detector to shut down. Press Ctrl+C again to exit immediately."
    }

    fn map_progress(status: SequenceStatus, state: &mut Self::State) -> bool {
        match status {
            SequenceStatus::Whole(Status::Processing {
                completion:
                    SequenceCompletion::Frames {
                        completed, ..
                    },
                ..
            }) => {
                state.frames_processed = completed;
                if !std::io::stdout().is_terminal() {
                    let event = NoiseDetectionConsoleEvent::ProcessedFrame {
                        completed,
                        total: state.total_frames,
                    };
                    let event = serde_json::to_string(&event).unwrap();
                    println!("[Noise Detector][Progress]: {event}");
                }
                true
            },
            SequenceStatus::Subprocess {
                parent,
                child,
            } => match (parent, child) {
                (
                    Status::Processing {
                        completion:
                            SequenceCompletion::Custom {
                                completed,
                                total,
                                ..
                            },
                        ..
                    },
                    Status::Processing {
                        id,
                        completion:
                            SequenceCompletion::SceneQuality {
                                index,
                                score,
                                ..
                            },
                    },
                ) if id == "Noise" => {
                    state.frames_processed = completed as u64;
                    state.total_frames = total as u64;
                    state.scene_noise.insert(index, score);
                    if !std::io::stdout().is_terminal() {
                        let event = NoiseDetectionConsoleEvent::SceneNoise {
                            scene: index,
                            noise: score,
                        };
                        let event = serde_json::to_string(&event).unwrap();
                        println!("[Noise Detector][Scene]: {event}");
                    }
                    true
                },
                _ => false,
            },
            _ => false,
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

        let top_info = Block::bordered()
            .border_type(ratatui::widgets::BorderType::Rounded)
            .title(Line::from("Input").centered())
            .title_bottom(Line::from("Noise Detection").centered());
        let top_info_inner = top_info.inner(layout[0]);
        let top_info_areas =
            Layout::vertical([Constraint::Fill(1), Constraint::Fill(1)]).split(top_info_inner);
        frame.render_widget(top_info, layout[0]);
        let input_info = InputInfo::new(self.clip_info);
        let input_info = input_info.generate(false);
        frame.render_widget(input_info, top_info_areas[0]);

        let state = &self.cached_state;
        let scene_noise = state
            .scene_noise
            .iter()
            .map(|(index, noise)| (*index as f64, *noise))
            .collect::<Vec<_>>();
        let datasets = vec![
            Dataset::default()
                .name("Noise Level")
                .style(Color::Green)
                .graph_type(ratatui::widgets::GraphType::Scatter)
                .data(&scene_noise),
        ];
        let scene_count = state.scene_noise.len().max(state.total_frames as usize);
        let max_scenes_label = format!("{}", scene_count.saturating_sub(1));
        let max_noise = scene_noise.iter().map(|(_, n)| *n).fold(0.0_f64, f64::max);
        let max_noise = if max_noise <= 0.0 {
            1.0
        } else {
            (max_noise * 1000.0).ceil() / 1000.0
        };
        let max_noise_label = format!("{max_noise:.3}");
        let chart = Chart::new(datasets)
            .block(
                Block::bordered()
                    .border_type(ratatui::widgets::BorderType::Rounded)
                    .title(Line::from("Noise Level per Scene").centered()),
            )
            .x_axis(
                Axis::default()
                    .title("Scene")
                    .bounds([0.0, scene_count as f64])
                    .labels(["0", &max_scenes_label]),
            )
            .y_axis(
                Axis::default()
                    .title("Noise")
                    .bounds([0.0, max_noise])
                    .labels(["0", &max_noise_label]),
            );
        frame.render_widget(chart, layout[1]);

        let progress_bar = ProgressBar {
            color:               MAIN_COLOR,
            processing_title:    if self.attempted_cancel {
                "Shutting down...".to_owned()
            } else {
                "Detecting Noisy Scenes...".to_owned()
            },
            completed_title:     if self.attempted_cancel {
                "Noise Detection Aborted".to_owned()
            } else {
                "Noise Detection Completed".to_owned()
            },
            top_right_title:     format!("{} found", state.scene_noise.len()),
            bottom_center_title: String::new(),
            unit_per_second:     "SPS".to_owned(),
            unit:                "Scene".to_owned(),
            initial_completed:   0,
            completed:           self.cached_state.frames_processed,
            total:               self.total_frames,
            show_label:          true,
        };
        let progress_bar = progress_bar.generate(Some(self.started));
        frame.render_widget(progress_bar, layout[2]);
    }
}

impl NoiseDetectionApp {
    pub fn new(scenes: &[Scene<CliSequenceData>], clip_info: ClipInfo) -> NoiseDetectionApp {
        let total_frames = scenes.len() as u64;
        let scene_noise: BTreeMap<u64, f64> = scenes
            .iter()
            .enumerate()
            .filter_map(|(index, scene)| {
                scene
                    .sequence_data
                    .noise_detection
                    .as_ref()
                    .map(|detection| (index as u64, detection.noise))
            })
            .collect();
        let frames_processed = scene_noise.len() as u64;
        let state = NoiseDetectionState {
            frames_processed,
            total_frames,
            scene_noise,
        };
        NoiseDetectionApp {
            original_panic_hook: None,
            started: std::time::Instant::now(),
            total_frames,
            clip_info,
            attempted_cancel: false,
            shared_progress: SharedProgress::new(state.clone()),
            cached_state: state,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum NoiseDetectionConsoleEvent {
    ProcessedFrame { completed: u64, total: u64 },
    SceneNoise { scene: u64, noise: f64 },
}
