use std::io::IsTerminal;

use andean_condor::core::{
    input::clip_info::ClipInfo,
    sequence::{SequenceCompletion, SequenceStatus, Status},
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
    components::{input_info::InputInfo, progress_bar::ProgressBar},
};
#[derive(Clone)]
pub struct NoiseDetectionState {
    pub frames_processed: u64,
    pub total_frames:     u64,
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
        "Noise Detection does not support cancelling. Press Ctrl+C again to exit."
    }

    fn map_progress(status: SequenceStatus, state: &mut Self::State) -> bool {
        if let SequenceStatus::Whole(Status::Processing {
            completion, ..
        }) = status
            && let SequenceCompletion::Frames {
                completed, ..
            } = completion
        {
            state.frames_processed = completed;
            if !std::io::stdout().is_terminal() {
                let event = NoiseDetectionConsoleEvent::ProcessedFrame {
                    completed,
                    total: state.total_frames,
                };
                let event = serde_json::to_string(&event).unwrap();
                println!("[Noise Detector][Progress]: {}", event);
            }
            return true;
        }
        false
    }

    fn render(&self, frame: &mut Frame) {
        const MAIN_COLOR: Color = Color::DarkGray;
        let layout = Layout::default()
            .constraints([
                Constraint::Percentage(10),
                Constraint::Percentage(80),
                Constraint::Percentage(10),
            ])
            .split(frame.area());

        let input_info = InputInfo::new(self.clip_info);
        let input_info = input_info.generate(false);
        let input_block = Block::bordered()
            .border_type(ratatui::widgets::BorderType::Rounded)
            .title(Line::from("Input").centered());
        let input_block = if self.attempted_cancel {
            input_block.title_bottom(
                Line::from(
                    "Noise Detection does not support cancelling. Press Ctrl+C again to exit.",
                )
                .centered(),
            )
        } else {
            input_block
        };
        let input_info = input_info.block(input_block);
        frame.render_widget(input_info, layout[0]);

        let progress_bar = ProgressBar {
            color:               MAIN_COLOR,
            processing_title:    if self.attempted_cancel {
                "Waiting for Noise Detection to Finish...".to_owned()
            } else {
                "Detecting Noisy Scenes...".to_owned()
            },
            completed_title:     "Noise Detection Completed".to_owned(),
            top_right_title:     String::new(),
            bottom_center_title: String::new(),
            unit_per_second:     "FPS".to_owned(),
            unit:                "Frame".to_owned(),
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
    pub fn new(total_frames: u64, clip_info: ClipInfo) -> NoiseDetectionApp {
        let state = NoiseDetectionState {
            frames_processed: 0,
            total_frames,
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
}
