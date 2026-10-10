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

/// Progress snapshot for the input-initialization (indexing) screen.
#[derive(Clone)]
pub struct InitializingState {
    pub percent: f64,
}

/// The screen shown while the input is being indexed (clip info probed and
/// frames counted) before any processing phase begins. Indexing can take a
/// while on large or network inputs, so it gets its own progress screen rather
/// than printing a bare log line.
pub struct InitializingApp {
    pub(crate) original_panic_hook: Option<super::PanicHook>,
    pub started:                    std::time::Instant,
    pub clip_info:                  ClipInfo,
    attempted_cancel:               bool,
    shared_progress:                SharedProgress<InitializingState>,
    cached_state:                   InitializingState,
}

impl TuiApp for InitializingApp {
    type State = InitializingState;

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
        "Waiting for Input to finish indexing. Press Ctrl+C again to exit immediately."
    }

    fn map_progress(status: SequenceStatus, state: &mut Self::State) -> bool {
        if let SequenceStatus::Whole(Status::Processing {
            completion, ..
        }) = status
            && let SequenceCompletion::Percentage(percentage) = completion
        {
            state.percent = percentage;
            if !std::io::stdout().is_terminal() {
                println!(
                    "[Initializing Input][Progress]: {}",
                    serde_json::to_string(&InitializingConsoleEvent::Processed(percentage)).unwrap()
                );
            }
            return true;
        }
        false
    }

    fn render(&self, frame: &mut Frame) {
        let theme = crate::theme::Theme::current();
        let main: Color = theme.main;
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
        let input_info = input_info.block(input_block);
        frame.render_widget(input_info, layout[0]);

        let progress_bar = ProgressBar {
            color:               main,
            processing_title:    if self.attempted_cancel {
                "Waiting for Indexing to Finish...".to_owned()
            } else {
                "Indexing Input...".to_owned()
            },
            completed_title:     if self.attempted_cancel {
                "Initialization Aborted".to_owned()
            } else {
                "Input Initialized".to_owned()
            },
            top_right_title:     String::new(),
            bottom_center_title: String::new(),
            unit_per_second:     "%".to_owned(),
            unit:                "Percent".to_owned(),
            initial_completed:   0,
            completed:           self.cached_state.percent.round() as u64,
            total:               100,
            show_label:          false,
        };
        let progress_bar = progress_bar.generate(Some(self.started));
        frame.render_widget(progress_bar, layout[2]);
    }
}

impl InitializingApp {
    pub fn new(clip_info: ClipInfo) -> InitializingApp {
        let state = InitializingState {
            percent: 0.0,
        };
        InitializingApp {
            original_panic_hook: None,
            started:             std::time::Instant::now(),
            clip_info,
            attempted_cancel:    false,
            shared_progress:     SharedProgress::new(state.clone()),
            cached_state:        state,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum InitializingConsoleEvent {
    Processed(f64),
}
