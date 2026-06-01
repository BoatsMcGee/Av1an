use std::{
    collections::BTreeMap,
    io::IsTerminal,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver},
        Arc,
    },
    thread,
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
    crossterm::event::{self, Event as TermEvent, KeyCode, KeyModifiers},
    layout::{Constraint, Layout},
    style::Color,
    text::Line,
    widgets::{Axis, Block, Chart, Dataset},
    Frame,
};
use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::{
    apps::TuiApp,
    components::{encoder_info::EncoderInfo, input_info::InputInfo, progress_bar::ProgressBar},
    configuration::CliSequenceData,
};

pub struct TargetQualityApp {
    pub(crate) original_panic_hook: Option<super::PanicHook>,
    pub encoder:                    Encoder,
    pub quality_passes:             BTreeMap<u64, Vec<QualityPass>>,
    pub current_pass:               u8,
    pub pass_started:               std::time::Instant,
    pub frames_encoded:             u64,
    pub frames_compared:            u64,
    pub total_frames:               u64,
    pub clip_info:                  ClipInfo,
    attempted_cancel:               bool,
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
        let input_tx = event_tx.clone();
        thread::spawn(move || loop {
            if let Ok(TermEvent::Key(key)) = event::read()
                && input_tx.send(TargetQualityAppEvent::Input(key)).is_err()
            {
                break;
            }
        });
        let tick_tx = event_tx.clone();
        thread::spawn(move || loop {
            if tick_tx.send(TargetQualityAppEvent::Tick).is_err() {
                break;
            }
            thread::sleep(std::time::Duration::from_millis(16)); // ~60 FPS
        });
        thread::spawn(move || {
            for progress in progress_rx {
                match progress {
                    SequenceStatus::Whole(status) => {
                        if let Status::Processing {
                            id: _id,
                            completion,
                        } = status
                            && let SequenceCompletion::Passes {
                                total, ..
                            } = completion
                        {
                            let _ = event_tx.send(TargetQualityAppEvent::Pass(total));
                        }
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
                            let _ = event_tx.send(TargetQualityAppEvent::EncodeProgress {
                                current_pass,
                                total_passes,
                                current_frame: completed,
                                total_frames: total,
                            });
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
                            let _ = event_tx.send(TargetQualityAppEvent::CompareProgress {
                                current_pass,
                                total_passes,
                                current_frame: completed,
                                total_frames: total,
                            });
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
                            let _ =
                                event_tx.send(TargetQualityAppEvent::QualityPass(QualityPass {
                                    scene: index,
                                    current_pass,
                                    total_passes,
                                    quantizer,
                                    score,
                                    bitrate,
                                }));
                        },
                        _ => (),
                    },
                }
            }
            let _ = event_tx.send(TargetQualityAppEvent::Quit);
        });

        let stdout_is_terminal = std::io::stdout().is_terminal();
        let mut terminal = self.init()?;
        loop {
            match event_rx.recv()? {
                TargetQualityAppEvent::Tick => {
                    terminal.draw(|f| self.render(f))?;
                },
                TargetQualityAppEvent::Pass(current_pass) => {
                    self.current_pass = current_pass;
                    if !stdout_is_terminal {
                        let event = TargetQualityConsoleEvent::Pass(current_pass);
                        println!(
                            "[Target Quality][Pass] {}",
                            serde_json::to_string(&event).unwrap()
                        );
                    }
                },
                TargetQualityAppEvent::EncodeProgress {
                    current_pass,
                    total_passes,
                    current_frame,
                    total_frames,
                } => {
                    if current_frame == 0 {
                        // Reset timer
                        self.pass_started = std::time::Instant::now();
                    }
                    self.frames_compared = 0;
                    self.frames_encoded = current_frame;
                    self.total_frames = total_frames;
                    if !stdout_is_terminal {
                        let event = TargetQualityConsoleEvent::EncodeProgress {
                            current_pass,
                            total_passes,
                            current_frame,
                            total_frames,
                        };
                        println!(
                            "[Target Quality][Encode] {}",
                            serde_json::to_string(&event).unwrap()
                        );
                    }
                },
                TargetQualityAppEvent::CompareProgress {
                    current_pass,
                    total_passes,
                    current_frame,
                    total_frames,
                } => {
                    if current_frame == 0 {
                        // Reset timer
                        self.pass_started = std::time::Instant::now();
                        self.frames_encoded = total_frames;
                    }
                    self.frames_compared = current_frame;
                    self.total_frames = total_frames;
                    if !stdout_is_terminal {
                        let event = TargetQualityConsoleEvent::CompareProgress {
                            current_pass,
                            total_passes,
                            current_frame,
                            total_frames,
                        };
                        println!(
                            "[Target Quality][Compare] {}",
                            serde_json::to_string(&event).unwrap()
                        );
                    }
                },
                TargetQualityAppEvent::QualityPass(quality_pass) => {
                    self.quality_passes
                        .get_mut(&quality_pass.scene)
                        .expect("Quality Pass exists")
                        .push(quality_pass.clone());
                    if !stdout_is_terminal {
                        let event = TargetQualityConsoleEvent::QualityPass(quality_pass);
                        println!(
                            "[Target Quality][Quality] {}",
                            serde_json::to_string(&event).unwrap()
                        );
                    }
                },
                TargetQualityAppEvent::Quit => {
                    self.restore(terminal)?;
                    break;
                },
                TargetQualityAppEvent::Input(key) => {
                    if key.code == KeyCode::Char('c')
                        && key.modifiers.contains(KeyModifiers::CONTROL)
                        // Prevents duplicate event from key release in Windows
                        && key.is_press()
                    {
                        self.attempted_cancel = true;
                        let already_cancelled = cancelled.swap(true, Ordering::SeqCst);
                        if already_cancelled {
                            self.restore(terminal)?;
                            debug!("Force quit Condor");
                            std::process::exit(0);
                        } else if !stdout_is_terminal {
                            println!(
                                "Waiting for Encoders to finish. Press Ctrl+C again to exit \
                                 immediately."
                            );
                        }
                    }
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

        let (quantizers, scores) = self.quality_passes.iter().fold(
            (Vec::new(), Vec::new()),
            |(mut quantizers, mut scores), (index, quality_passes)| {
                if let Some(quality_pass) = quality_passes.iter().last() {
                    quantizers.push((*index as f64, quality_pass.quantizer));
                    scores.push((*index as f64, quality_pass.score));
                    (quantizers, scores)
                } else {
                    quantizers.push((*index as f64, self.encoder.quantizer().unwrap_or(0.0)));
                    scores.push((*index as f64, 0.0));
                    (quantizers, scores)
                }
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
        let max_scenes_label = format!("{}", self.quality_passes.len() - 1);
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
                    .bounds([0.0, self.quality_passes.len() as f64])
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
            } else if self.frames_encoded < self.total_frames {
                format!("Encoding Pass {}", self.current_pass)
            } else {
                format!("Comparing Pass {}", self.current_pass)
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
            completed:           if self.frames_encoded < self.total_frames {
                self.frames_encoded
            } else {
                self.frames_compared
            },
            total:               self.total_frames,
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
        let quality_passes = scenes
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
        TargetQualityApp {
            original_panic_hook: None,
            encoder,
            quality_passes,
            current_pass: 1,
            pass_started: std::time::Instant::now(),
            frames_encoded: 0,
            frames_compared: 0,
            total_frames: 1,
            clip_info,
            attempted_cancel: false,
        }
    }
}

enum TargetQualityAppEvent {
    Quit,
    Tick,                   // 60 FPS
    Input(event::KeyEvent), // Keyboard events
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
