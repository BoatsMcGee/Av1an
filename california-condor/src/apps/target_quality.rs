use std::{
    collections::BTreeMap,
    io::IsTerminal,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError},
    },
    thread,
    time::Duration,
};

use andean_condor::{
    core::{
        input::clip_info::ClipInfo,
        sequence::{SequenceCompletion, SequenceStatus, Status},
    },
    models::{encoder::Encoder, scene::Scene, sequence::target_quality::types::ProbeStatistic},
};
use anyhow::Result;
use ratatui::{
    Frame,
    crossterm::event::{self, Event as TermEvent, KeyCode, KeyModifiers},
    layout::{Constraint, Layout},
    style::Color,
    text::Line,
    widgets::{Axis, Block, Chart, Dataset},
};
use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::{
    apps::{TuiApp, shared_progress::SharedProgress},
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
    attempted_cancel:               bool,
    shared_progress:                SharedProgress<TargetQualityState>,
    cached_state:                   TargetQualityState,
}

impl TuiApp for TargetQualityApp {
    fn original_panic_hook(&mut self) -> &mut Option<super::PanicHook> {
        &mut self.original_panic_hook
    }

    fn run(
        &mut self,
        progress_rx: Receiver<SequenceStatus>,
        cancelled: Arc<AtomicBool>,
    ) -> Result<()> {
        let (event_tx, event_rx) = mpsc::channel();
        if !crate::apps::is_test_mode() {
            let input_tx = event_tx.clone();
            thread::spawn(move || {
                loop {
                    if let Ok(TermEvent::Key(key)) = event::read()
                        && input_tx.send(TargetQualityAppEvent::Input(key)).is_err()
                    {
                        break;
                    }
                }
            });
        }
        if !crate::apps::is_test_mode() {
            let tick_tx = event_tx.clone();
            thread::spawn(move || {
                loop {
                    if tick_tx.send(TargetQualityAppEvent::Tick).is_err() {
                        break;
                    }
                    thread::sleep(Duration::from_millis(33)); // ~30 FPS
                }
            });
        }
        let shared_progress = self.shared_progress.clone();
        let quit = Arc::new(AtomicBool::new(false));
        let quit_flag = Arc::clone(&quit);
        thread::spawn(move || {
            for progress in progress_rx {
                match progress {
                    SequenceStatus::Whole(status) => match status {
                        Status::Processing {
                            completion:
                                SequenceCompletion::Passes {
                                    total, ..
                                },
                            ..
                        } => {
                            shared_progress.apply(|state| {
                                state.current_pass = total;
                                true
                            });
                            if !std::io::stdout().is_terminal() {
                                let event = TargetQualityConsoleEvent::Pass(total);
                                println!(
                                    "[Target Quality][Pass] {}",
                                    serde_json::to_string(&event).unwrap()
                                );
                            }
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
                                shared_progress.apply(|state| {
                                    state.frames_compared = 0;
                                    state.frames_encoded = completed;
                                    state.total_frames = total;
                                    true
                                });
                            } else {
                                shared_progress.apply(|state| {
                                    if completed == 0 {
                                        state.frames_encoded = total;
                                    }
                                    state.frames_compared = completed;
                                    state.total_frames = total;
                                    true
                                });
                            }
                        },
                        _ => {},
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
                            shared_progress.apply(|state| {
                                state.frames_compared = 0;
                                state.frames_encoded = completed;
                                state.total_frames = total;
                                true
                            });
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
                            shared_progress.apply(|state| {
                                if completed == 0 {
                                    state.frames_encoded = total;
                                }
                                state.frames_compared = completed;
                                state.total_frames = total;
                                true
                            });
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
                            shared_progress.apply(|state| {
                                state.quality_passes.entry(index).or_default().push(pass.clone());
                                true
                            });
                            if !std::io::stdout().is_terminal() {
                                let event = TargetQualityConsoleEvent::QualityPass(pass);
                                println!(
                                    "[Target Quality][Quality] {}",
                                    serde_json::to_string(&event).unwrap()
                                );
                            }
                        },
                        _ => {},
                    },
                }
            }
            let _ = event_tx.send(TargetQualityAppEvent::Quit);
            quit_flag.store(true, Ordering::Release);
        });

        if crate::apps::is_test_mode() {
            while !quit.load(Ordering::Acquire) {
                std::thread::sleep(Duration::from_millis(10));
                if let Some(snapshot) = self.shared_progress.read_if_dirty() {
                    self.cached_state = snapshot;
                }
                while event_rx.try_recv().is_ok() {}
            }
            self.cached_state = self.shared_progress.read();
            return Ok(());
        }

        let stdout_is_terminal = std::io::stdout().is_terminal();
        let mut terminal = self.init()?;
        'event_loop: loop {
            while let Ok(TargetQualityAppEvent::Input(key)) = event_rx.try_recv() {
                if Self::handle_ctrl_c(
                    key,
                    &mut self.attempted_cancel,
                    &cancelled,
                    &mut terminal,
                    stdout_is_terminal,
                )? {
                    if let Some(snapshot) = self.shared_progress.read_if_dirty() {
                        self.cached_state = snapshot;
                    }
                    terminal.draw(|f| self.render(f))?;
                    self.restore(terminal)?;
                    break 'event_loop;
                }
            }

            if let Some(snapshot) = self.shared_progress.read_if_dirty() {
                // Reset timer
                let new_encode_phase =
                    snapshot.frames_encoded == 0 && snapshot.frames_compared == 0;
                let new_compare_phase =
                    snapshot.frames_compared > 0 && self.cached_state.frames_compared == 0;
                let pass_changed = snapshot.current_pass != self.cached_state.current_pass;
                if new_encode_phase || new_compare_phase || pass_changed {
                    self.pass_started = std::time::Instant::now();
                }
                self.cached_state = snapshot;
            }

            if quit.load(Ordering::Acquire) {
                self.cached_state = self.shared_progress.read();
                terminal.draw(|f| self.render(f))?;
                self.restore(terminal)?;
                break;
            }

            match event_rx.recv_timeout(Duration::from_millis(33)) {
                Ok(TargetQualityAppEvent::Tick) => {
                    terminal.draw(|f| self.render(f))?;
                },
                Ok(TargetQualityAppEvent::Input(key)) => {
                    if Self::handle_ctrl_c(
                        key,
                        &mut self.attempted_cancel,
                        &cancelled,
                        &mut terminal,
                        stdout_is_terminal,
                    )? {
                        if let Some(snapshot) = self.shared_progress.read_if_dirty() {
                            self.cached_state = snapshot;
                        }
                        terminal.draw(|f| self.render(f))?;
                        self.restore(terminal)?;
                        break 'event_loop;
                    }
                },
                Ok(TargetQualityAppEvent::Quit) => {
                    if let Some(snapshot) = self.shared_progress.read_if_dirty() {
                        self.cached_state = snapshot;
                    }
                    terminal.draw(|f| self.render(f))?;
                    self.restore(terminal)?;
                    break;
                },
                Err(RecvTimeoutError::Timeout) => {
                    terminal.draw(|f| self.render(f))?;
                },
                Err(RecvTimeoutError::Disconnected) => {
                    if let Some(snapshot) = self.shared_progress.read_if_dirty() {
                        self.cached_state = snapshot;
                    }
                    terminal.draw(|f| self.render(f))?;
                    self.restore(terminal)?;
                    break;
                },
            }
        }
        Ok(())
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
                .style(Color::Blue)
                .graph_type(ratatui::widgets::GraphType::Scatter)
                .data(&quantizers),
            Dataset::default()
                .name("Score")
                .style(Color::Green)
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
            color:               MAIN_COLOR,
            processing_title:    if self.attempted_cancel {
                "Shutting down...".to_owned()
            } else if state.frames_encoded < state.total_frames {
                format!("Encoding Pass {}", state.current_pass)
            } else {
                format!("Comparing Pass {}", state.current_pass)
            },
            completed_title:     if self.attempted_cancel {
                "Target Quality Aborted".to_owned()
            } else {
                "Target Quality Completed".to_owned()
            },
            top_right_title:     String::new(),
            bottom_center_title: String::new(),
            unit_per_second:     "FPS".to_owned(),
            unit:                "Frame".to_owned(),
            initial_completed:   0,
            completed:           if state.frames_encoded < state.total_frames {
                state.frames_encoded
            } else {
                state.frames_compared
            },
            total:               state.total_frames,
            show_label:          true,
        };
        let progress_bar = progress_bar.generate(Some(self.pass_started));
        frame.render_widget(progress_bar, layout[2]);
    }
}

impl TargetQualityApp {
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
            attempted_cancel: false,
            shared_progress: SharedProgress::new(state.clone()),
            cached_state: state,
        }
    }

    fn handle_ctrl_c(
        key: ratatui::crossterm::event::KeyEvent,
        attempted_cancel: &mut bool,
        cancelled: &Arc<AtomicBool>,
        _terminal: &mut super::StdOutOrErrTerminal,
        stdout_is_terminal: bool,
    ) -> Result<bool> {
        if key.code == KeyCode::Char('c')
            && key.modifiers.contains(KeyModifiers::CONTROL)
            && key.is_press()
        {
            *attempted_cancel = true;
            let already_cancelled = cancelled.swap(true, Ordering::SeqCst);
            if already_cancelled {
                debug!("Force quit Condor");
                return Ok(true);
            } else if !stdout_is_terminal {
                println!("Waiting for Encoders to finish. Press Ctrl+C again to exit immediately.");
            }
        }
        Ok(false)
    }
}

enum TargetQualityAppEvent {
    Quit,
    Tick,                   // 30 FPS
    Input(event::KeyEvent), // Keyboard events
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
    scene:        u64,
    current_pass: u8,
    total_passes: u8,
    quantizer:    f64,
    score:        f64,
    bitrate:      f64,
}
