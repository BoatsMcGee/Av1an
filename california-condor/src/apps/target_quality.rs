use std::{collections::BTreeMap, io::IsTerminal};

use andean_condor::{
    core::{
        input::clip_info::ClipInfo,
        sequence::{SequenceCompletion, SequenceStatus, Status},
    },
    models::{encoder::Encoder, scene::Scene, sequence::target_quality::types::ProbeStatistic},
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
    components::{encoder_info::EncoderInfo, input_info::InputInfo, progress_bar::ProgressBar},
    configuration::CliSequenceData,
};
#[derive(Clone)]
pub struct TargetQualityState {
    pub quality_passes:  BTreeMap<u64, Vec<QualityPass>>,
    pub current_pass:    u8,
    pub frames_encoded:  u64,
    pub frames_compared: u64,
    pub total_frames:    u64,
}

pub struct TargetQualityApp {
    pub(crate) original_panic_hook: Option<super::PanicHook>,
    pub encoder:                    Encoder,
    pub clip_info:                  ClipInfo,
    pub pass_started:               std::time::Instant,
    /// The stopped clock of a pass whose frames are all done, held so the
    /// progress bar's FPS stops moving through the gap before the next pass.
    pass_completed:                 Option<std::time::Instant>,
    attempted_cancel:               bool,
    shared_progress:                SharedProgress<TargetQualityState>,
    cached_state:                   TargetQualityState,
}

impl TuiApp for TargetQualityApp {
    type State = TargetQualityState;

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
            SequenceStatus::Whole(status) => match status {
                Status::Processing {
                    completion:
                        SequenceCompletion::Passes {
                            total, ..
                        },
                    ..
                } => {
                    state.current_pass = total;
                    if !std::io::stdout().is_terminal() {
                        let event = TargetQualityConsoleEvent::Pass(total);
                        println!(
                            "[Target Quality][Pass] {}",
                            serde_json::to_string(&event).unwrap()
                        );
                    }
                    true
                },
                Status::Processing {
                    id,
                    completion:
                        SequenceCompletion::Frames {
                            completed,
                            total,
                        },
                } if id == "Encode" || id == "Compare" => {
                    if id == "Encode" {
                        state.frames_compared = 0;
                        state.frames_encoded = completed;
                        state.total_frames = total;
                    } else {
                        if completed == 0 {
                            state.frames_encoded = total;
                        }
                        state.frames_compared = completed;
                        state.total_frames = total;
                    }
                    true
                },
                _ => false,
            },
            SequenceStatus::Subprocess {
                parent,
                child,
            } => match (parent, child) {
                (
                    Status::Processing {
                        completion:
                            SequenceCompletion::Passes {
                                completed: current_pass,
                                total: total_passes,
                            },
                        ..
                    },
                    Status::Processing {
                        id,
                        completion:
                            SequenceCompletion::Frames {
                                completed,
                                total,
                            },
                    },
                ) if id == "Encode" => {
                    state.frames_compared = 0;
                    state.frames_encoded = completed;
                    state.total_frames = total;
                    if !std::io::stdout().is_terminal() {
                        let event = TargetQualityConsoleEvent::EncodeProgress {
                            current_pass,
                            total_passes,
                            current_frame: completed,
                            total_frames: total,
                        };
                        println!(
                            "[Target Quality][Encode] {}",
                            serde_json::to_string(&event).unwrap()
                        );
                    }
                    true
                },
                (
                    Status::Processing {
                        completion:
                            SequenceCompletion::Passes {
                                completed: current_pass,
                                total: total_passes,
                            },
                        ..
                    },
                    Status::Processing {
                        id,
                        completion:
                            SequenceCompletion::Frames {
                                completed,
                                total,
                            },
                    },
                ) if id == "Compare" => {
                    if completed == 0 {
                        state.frames_encoded = total;
                    }
                    state.frames_compared = completed;
                    state.total_frames = total;
                    if !std::io::stdout().is_terminal() {
                        let event = TargetQualityConsoleEvent::CompareProgress {
                            current_pass,
                            total_passes,
                            current_frame: completed,
                            total_frames: total,
                        };
                        println!(
                            "[Target Quality][Compare] {}",
                            serde_json::to_string(&event).unwrap()
                        );
                    }
                    true
                },
                (
                    Status::Processing {
                        completion:
                            SequenceCompletion::Passes {
                                completed: current_pass,
                                total: total_passes,
                            },
                        ..
                    },
                    Status::Processing {
                        id,
                        completion:
                            SequenceCompletion::SceneQuality {
                                index,
                                quantizer,
                                score,
                                bitrate,
                            },
                    },
                ) if id == "Quality" => {
                    let pass = QualityPass {
                        scene: index,
                        current_pass,
                        total_passes,
                        quantizer,
                        score,
                        bitrate,
                    };
                    state.quality_passes.entry(index).or_default().push(pass.clone());
                    if !std::io::stdout().is_terminal() {
                        let event = TargetQualityConsoleEvent::QualityPass(pass);
                        println!(
                            "[Target Quality][Quality] {}",
                            serde_json::to_string(&event).unwrap()
                        );
                    }
                    true
                },
                _ => false,
            },
        }
    }

    fn on_snapshot(&mut self, snapshot: Self::State) {
        // Reset the pass timer when entering a new encode/compare phase or pass.
        let new_encode_phase = snapshot.frames_encoded == 0 && snapshot.frames_compared == 0;
        let new_compare_phase =
            snapshot.frames_compared > 0 && self.cached_state.frames_compared == 0;
        let pass_changed = snapshot.current_pass != self.cached_state.current_pass;
        if new_encode_phase || new_compare_phase || pass_changed {
            self.pass_started = std::time::Instant::now();
        }
        Self::stop_clock_at_completion(&snapshot, &mut self.pass_completed);
        self.cached_state = snapshot;
    }

    fn render(&self, frame: &mut Frame) {
        let theme = crate::theme::Theme::current();
        let main: Color = theme.main;
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

        let state = &self.cached_state;
        let (quantizers, scores) = state.quality_passes.iter().fold(
            (Vec::new(), Vec::new()),
            |(mut quantizers, mut scores), (index, quality_passes)| {
                if let Some(quality_pass) = quality_passes.iter().last() {
                    quantizers.push((*index as f64, quality_pass.quantizer));
                    scores.push((*index as f64, quality_pass.score));
                } else {
                    quantizers.push((*index as f64, self.encoder.quantizer().unwrap_or(0.0)));
                    scores.push((*index as f64, 0.0));
                }
                (quantizers, scores)
            },
        );
        let datasets = vec![
            Dataset::default()
                .name("Quantizer")
                .style(theme.accent_blue)
                .graph_type(ratatui::widgets::GraphType::Scatter)
                .data(&quantizers),
            Dataset::default()
                .name("Score")
                .style(theme.accent_green)
                .graph_type(ratatui::widgets::GraphType::Scatter)
                .data(&scores),
        ];
        let max_scenes_label = format!("{}", state.quality_passes.len().saturating_sub(1));
        let max_quantizer = quantizers.iter().map(|(_, q)| *q).fold(0.0_f64, f64::max);
        let max_score = scores.iter().map(|(_, s)| *s).fold(0.0_f64, f64::max);
        let max_quantizer_score = (f64::max(max_quantizer, max_score) / 10.0).ceil() * 10.0; // Round up to nearest 10
        let max_quantizer_score_label = format!("{}", max_quantizer_score);
        let chart = Chart::new(datasets)
            .block(
                Block::bordered()
                    .border_type(ratatui::widgets::BorderType::Rounded)
                    .title(Line::from("Quantizer and Score per Scene").centered()),
            )
            .x_axis(
                Axis::default()
                    .title("Scene")
                    .bounds([0.0, state.quality_passes.len() as f64])
                    .labels(["0", &max_scenes_label]),
            )
            .y_axis(
                Axis::default()
                    .title("Quantizer/Score")
                    .bounds([0.0, max_quantizer_score])
                    .labels(["0", &max_quantizer_score_label]),
            );
        frame.render_widget(chart, layout[1]);

        let progress_bar = ProgressBar {
            color:               main,
            processing_title:    if self.attempted_cancel {
                "Shutting down...".to_owned()
            } else if state.frames_encoded < state.total_frames {
                format!("Encoding Pass {}", state.current_pass)
            } else {
                format!("Comparing Pass {}", state.current_pass)
            },
            completed_title:     Self::completed_title(state, self.attempted_cancel),
            top_right_title:     String::new(),
            bottom_center_title: String::new(),
            unit_per_second:     "FPS".to_owned(),
            unit:                "Frame".to_owned(),
            initial_completed:   0,
            completed:           Self::completed(state),
            total:               state.total_frames,
            show_label:          true,
        };
        // A finished pass keeps its clock stopped, so the FPS it measured stays
        // on screen instead of being recomputed against the gap that follows.
        let clock = self.pass_completed.unwrap_or(self.pass_started);
        let progress_bar = progress_bar.generate(Some(clock));
        frame.render_widget(progress_bar, layout[2]);
    }
}

impl TargetQualityApp {
    /// The bar's top-left title once every frame it tracks is done.
    #[inline]
    fn completed_title(state: &TargetQualityState, attempted_cancel: bool) -> String {
        if attempted_cancel {
            "Target Quality Aborted".to_owned()
        } else {
            format!("Comparing Pass {} Complete", state.current_pass)
        }
    }

    /// The frames the progress bar measures, and therefore the count a rate
    /// would be derived from: the encode's frames while encoding, the compare's
    /// once every frame is encoded.
    #[inline]
    fn completed(state: &TargetQualityState) -> u64 {
        if state.frames_encoded < state.total_frames {
            state.frames_encoded
        } else {
            state.frames_compared
        }
    }

    /// Stops the clock once a pass has produced every frame the bar shows, and
    /// releases it again when the next pass reports progress.
    fn stop_clock_at_completion(
        snapshot: &TargetQualityState,
        pass_completed: &mut Option<std::time::Instant>,
    ) {
        if Self::completed(snapshot) >= snapshot.total_frames {
            if pass_completed.is_none() {
                *pass_completed = Some(std::time::Instant::now());
            }
        } else {
            *pass_completed = None;
        }
    }

    pub fn new(
        clip_info: ClipInfo,
        scenes: Vec<Scene<CliSequenceData>>,
        encoder: Encoder,
        probe_statistic: ProbeStatistic,
    ) -> TargetQualityApp {
        let quality_passes: BTreeMap<u64, Vec<QualityPass>> = scenes
            .into_iter()
            .enumerate()
            .map(|(scene_index, scene)| {
                (
                    scene_index as u64,
                    scene
                        .sequence_data
                        .target_quality
                        .passes
                        .iter()
                        .enumerate()
                        .map(|(pass_index, pass)| QualityPass {
                            scene:        scene_index as u64,
                            current_pass: (pass_index + 1) as u8,
                            total_passes: (pass_index + 1) as u8,
                            quantizer:    pass.quantizer,
                            score:        probe_statistic.calculate(&pass.scores),
                            bitrate:      pass.bitrate,
                        })
                        .collect(),
                )
            })
            .collect();
        let state = TargetQualityState {
            quality_passes,
            current_pass: 1,
            frames_encoded: 0,
            frames_compared: 0,
            total_frames: 1,
        };
        TargetQualityApp {
            original_panic_hook: None,
            encoder,
            clip_info,
            pass_started: std::time::Instant::now(),
            pass_completed: None,
            attempted_cancel: false,
            shared_progress: SharedProgress::new(state.clone()),
            cached_state: state,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TargetQualityConsoleEvent {
    Pass(u8),
    QualityPass(QualityPass),
    EncodeProgress {
        current_pass:  u8,
        total_passes:  u8,
        current_frame: u64,
        total_frames:  u64,
    },
    CompareProgress {
        current_pass:  u8,
        total_passes:  u8,
        current_frame: u64,
        total_frames:  u64,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QualityPass {
    pub(crate) scene:        u64,
    pub(crate) current_pass: u8,
    pub(crate) total_passes: u8,
    pub(crate) quantizer:    f64,
    pub(crate) score:        f64,
    pub(crate) bitrate:      f64,
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::{TargetQualityApp, TargetQualityState};

    const TOTAL: u64 = 14386;

    fn state(current_pass: u8, frames_encoded: u64, frames_compared: u64) -> TargetQualityState {
        TargetQualityState {
            quality_passes: BTreeMap::new(),
            current_pass,
            frames_encoded,
            frames_compared,
            total_frames: TOTAL,
        }
    }

    /// A pass that has compared every frame is finished, so its clock stops
    /// and the FPS it measured is what stays on screen.
    #[test]
    fn a_finished_pass_stops_the_clock() {
        let mut pass_completed = None;

        TargetQualityApp::stop_clock_at_completion(&state(2, TOTAL, TOTAL), &mut pass_completed);

        assert!(
            pass_completed.is_some(),
            "a pass with every frame compared has finished"
        );
    }

    /// Frames still arriving mean the pass is running: the clock belongs to it
    /// and must keep ticking.
    #[test]
    fn frames_still_coming_in_keep_the_clock_running() {
        let mut pass_completed = None;

        TargetQualityApp::stop_clock_at_completion(&state(2, 6040, 0), &mut pass_completed);
        assert!(
            pass_completed.is_none(),
            "an encoding pass is still running"
        );

        TargetQualityApp::stop_clock_at_completion(&state(2, TOTAL, 7190), &mut pass_completed);
        assert!(
            pass_completed.is_none(),
            "a comparing pass is still running"
        );
    }

    /// The snapshot that announces the next pass arrives before any of its
    /// frames, so the counters still read the finished pass. The clock must
    /// stay stopped through it — this is the snapshot the FPS used to spike on.
    #[test]
    fn the_next_pass_announcement_leaves_the_clock_stopped() {
        let mut pass_completed = None;
        TargetQualityApp::stop_clock_at_completion(&state(2, TOTAL, TOTAL), &mut pass_completed);
        let stopped = pass_completed.expect("the finished pass stops the clock");

        TargetQualityApp::stop_clock_at_completion(&state(3, TOTAL, TOTAL), &mut pass_completed);

        assert_eq!(
            pass_completed,
            Some(stopped),
            "no frames of the next pass have been reported yet"
        );
    }

    /// The first frames of the next pass hand the clock back to it.
    #[test]
    fn the_next_pass_resumes_the_clock() {
        let mut pass_completed = None;
        TargetQualityApp::stop_clock_at_completion(&state(2, TOTAL, TOTAL), &mut pass_completed);

        TargetQualityApp::stop_clock_at_completion(&state(3, 1, 0), &mut pass_completed);

        assert!(
            pass_completed.is_none(),
            "the next pass is encoding, so its own clock runs"
        );
    }

    /// A finished compare ends a probe pass, and another pass usually follows,
    /// so the title names that pass instead of claiming the whole run is done.
    #[test]
    fn a_finished_compare_titles_its_own_pass() {
        assert_eq!(
            TargetQualityApp::completed_title(&state(2, TOTAL, TOTAL), false),
            "Comparing Pass 2 Complete"
        );
    }

    /// Cancelling takes over the title, whatever the pass counters say.
    #[test]
    fn cancelling_titles_the_abort() {
        assert_eq!(
            TargetQualityApp::completed_title(&state(2, TOTAL, TOTAL), true),
            "Target Quality Aborted"
        );
    }
}
