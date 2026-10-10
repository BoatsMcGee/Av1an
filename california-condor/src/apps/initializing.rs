use std::{
    io::IsTerminal,
    sync::{Arc, Mutex},
};

use andean_condor::core::{
    input::clip_info::ClipInfo,
    sequence::{SequenceCompletion, SequenceStatus, Status},
};
use ratatui::{
    Frame,
    layout::{Constraint, Layout},
    style::Color,
    text::Line,
    widgets::{Block, Paragraph},
};
use serde::{Deserialize, Serialize};

use crate::{
    apps::{SharedProgress, TuiApp},
    components::{input_info::InputInfo, progress_bar::ProgressBar},
};

/// The [`Status::Processing`] `id` carrying each phase across the progress
/// channel. An `Opening` report carries no percentage of its own, so the phase
/// travels alongside it and the screen can tell "0% indexed" apart from "no
/// percentage exists".
pub const INDEXING_ID: &str = "indexing";
pub const OPENING_ID: &str = "opening";

/// What the startup screen is currently reporting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InitializingPhase {
    /// An index is being built — FFMS2's indexer, or the `dgindexnv`
    /// subprocess — so the bar tracks real progress.
    Indexing,
    /// An index already exists and the input is being opened. Nothing finer
    /// is knowable: VapourSynth indexes inside its own source plugin, where no
    /// callback of ours can reach it, so the bar stays where it is.
    Opening,
}

/// Progress snapshot for the startup "Opening input" screen.
#[derive(Clone)]
pub struct InitializingState {
    pub percent: f64,
    pub phase:   InitializingPhase,
}

/// The screen shown while an input is opened: indexing it if it has no cache,
/// then probing it. Indexing can take a while on large or network inputs, so
/// it gets its own progress screen rather than printing a bare log line.
///
/// This is the only screen that reports input opening — the shared
/// `open_input_with_progress` helper runs it for the startup open and for
/// every later open, so indexing is reported once, at the front, instead of
/// once per phase.
pub struct InitializingApp {
    pub(crate) original_panic_hook: Option<super::PanicHook>,
    pub started:                    std::time::Instant,
    /// The clip being opened, filled in as soon as it can be probed.
    ///
    /// A slot rather than a plain field because the screen has to be built
    /// before the open starts: a VapourSynth input cannot be probed before its
    /// decoder exists, and making that decoder exist is exactly the work being
    /// reported. Shared so the opener thread can publish the answer for the
    /// last frame.
    clip_info:                      Arc<Mutex<Option<ClipInfo>>>,
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
        "Waiting for Input to finish opening. Press Ctrl+C again to exit immediately."
    }

    fn map_progress(status: SequenceStatus, state: &mut Self::State) -> bool {
        let SequenceStatus::Whole(Status::Processing {
            id,
            completion,
        }) = status
        else {
            return false;
        };
        let SequenceCompletion::Percentage(percentage) = completion else {
            return false;
        };
        let phase = if id == OPENING_ID {
            InitializingPhase::Opening
        } else {
            InitializingPhase::Indexing
        };
        let mut changed = state.phase != phase;
        state.phase = phase;
        // `Opening` is the indeterminate phase: it exists only to switch the
        // title, so it leaves the bar at whatever the indexing phase reached.
        if phase == InitializingPhase::Opening {
            return changed;
        }
        // Rounded: the bar can only show a whole percent anyway, and FFMS2
        // calls back per frame — unrounded this prints one console line per
        // indexed frame in a headless run (measured ~600 for a 45s clip).
        let percentage = percentage.round();
        if state.percent != percentage {
            state.percent = percentage;
            changed = true;
            if !std::io::stdout().is_terminal() {
                println!(
                    "[Initializing Input][Progress]: {}",
                    serde_json::to_string(&InitializingConsoleEvent::Processed(percentage)).unwrap()
                );
            }
        }
        changed
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

        let input_block = Block::bordered()
            .border_type(ratatui::widgets::BorderType::Rounded)
            .title(Line::from("Input").centered());
        match self.clip_info() {
            Some(clip_info) => {
                let input_info = InputInfo::new(clip_info);
                let input_info = input_info.generate(false).block(input_block);
                frame.render_widget(input_info, layout[0]);
            },
            // Nothing to describe yet: VapourSynth can only be probed once its
            // decoder exists, and building it is the work being reported.
            None => {
                let placeholder =
                    Paragraph::new(Line::from("Probing input...").centered()).block(input_block);
                frame.render_widget(placeholder, layout[0]);
            },
        }

        let progress_bar = ProgressBar {
            color:               main,
            processing_title:    if self.attempted_cancel {
                "Waiting for the Input to Finish Opening...".to_owned()
            } else {
                match self.cached_state.phase {
                    InitializingPhase::Indexing => "Indexing Input...".to_owned(),
                    InitializingPhase::Opening => "Opening Input...".to_owned(),
                }
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
    /// Builds the screen, optionally with a clip already known (documentation
    /// screenshots and tests probe one up front).
    pub fn new(clip_info: Option<ClipInfo>) -> InitializingApp {
        let state = InitializingState {
            percent: 0.0,
            phase:   InitializingPhase::Opening,
        };
        InitializingApp {
            original_panic_hook: None,
            started:             std::time::Instant::now(),
            clip_info:           Arc::new(Mutex::new(clip_info)),
            attempted_cancel:    false,
            shared_progress:     SharedProgress::new(state.clone()),
            cached_state:        state,
        }
    }

    /// The slot the opener thread writes the probed clip into. Clone it, move
    /// it into the opener, and [`InitializingApp::render`] picks the value up
    /// as soon as it lands.
    #[inline]
    pub fn clip_info_slot(&self) -> Arc<Mutex<Option<ClipInfo>>> {
        Arc::clone(&self.clip_info)
    }

    /// The clip being opened, once it is known.
    #[inline]
    fn clip_info(&self) -> Option<ClipInfo> {
        *self.clip_info.lock().expect("initializing clip_info lock")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum InitializingConsoleEvent {
    Processed(f64),
}

#[cfg(test)]
mod tests {
    use andean_condor::core::sequence::{SequenceCompletion, SequenceStatus, Status};

    use super::*;
    use crate::apps::TuiApp;

    fn status(id: &str, percent: f64) -> SequenceStatus {
        SequenceStatus::Whole(Status::Processing {
            id: id.to_owned(),
            completion: SequenceCompletion::Percentage(percent),
        })
    }

    /// A cache hit reports nothing but `Opening`, so the bar must not claim to
    /// be indexing — there is nothing to index.
    #[test]
    fn an_open_without_indexing_stays_indeterminate() {
        let mut state = InitializingState {
            percent: 0.0,
            phase:   InitializingPhase::Opening,
        };

        assert!(!InitializingApp::map_progress(status(OPENING_ID, 0.0), &mut state));
        assert_eq!(state.phase, InitializingPhase::Opening);
        assert_eq!(state.percent, 0.0);
    }

    /// Indexing really running switches the phase, and the bar tracks it,
    /// rounded to a whole percent — which is all the bar can show.
    #[test]
    fn indexing_reports_its_percentage() {
        let mut state = InitializingState {
            percent: 0.0,
            phase:   InitializingPhase::Opening,
        };

        assert!(InitializingApp::map_progress(status(INDEXING_ID, 37.5), &mut state));
        assert_eq!(state.phase, InitializingPhase::Indexing);
        assert_eq!(state.percent, 38.0);
    }

    /// The final `Opening` keeps the bar where indexing left it, so a finished
    /// open still reads as complete rather than snapping back to zero.
    #[test]
    fn opening_after_indexing_keeps_the_reached_percentage() {
        let mut state = InitializingState {
            percent: 0.0,
            phase:   InitializingPhase::Opening,
        };
        InitializingApp::map_progress(status(INDEXING_ID, 100.0), &mut state);

        assert!(InitializingApp::map_progress(status(OPENING_ID, 0.0), &mut state));
        assert_eq!(state.phase, InitializingPhase::Opening);
        assert_eq!(state.percent, 100.0);
    }
}
