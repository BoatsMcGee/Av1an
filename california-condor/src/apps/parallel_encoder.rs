use std::{
    collections::BTreeMap,
    io::IsTerminal,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError},
        Arc,
    },
    thread,
    time::Duration,
};

use andean_condor::{
    core::{
        input::clip_info::ClipInfo,
        sequence::{SequenceCompletion, SequenceStatus, Status},
    },
    models::{encoder::Encoder, scene::Scene},
};
use anyhow::Result;
use ratatui::{
    crossterm::event::{self, Event as TermEvent, KeyCode, KeyModifiers},
    layout::{Constraint, Layout},
    style::Color,
    text::Line,
    widgets::Block,
    Frame,
};
use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::{
    apps::{shared_progress::SharedProgress, TuiApp},
    components::{
        active_encoders::ActiveEncoders,
        encoder_info::EncoderInfo,
        input_info::InputInfo,
        progress_bar::ProgressBar,
    },
    configuration::CliSequenceData,
};

#[derive(Debug, Clone)]
pub struct SceneProgressInfo {
    pub scene_index:      u64,
    pub current_pass:     u8,
    pub total_passes:     u8,
    pub frames_processed: u64,
    pub total_frames:     u64,
    pub started:          std::time::Instant,
}

#[derive(Clone)]
pub struct ParallelEncoderState {
    pub active_scenes:          BTreeMap<u64, SceneProgressInfo>,
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
    pub scenes:                     BTreeMap<u64, (u64, Scene<CliSequenceData>)>,
    pub active_scenes:              BTreeMap<u64, SceneEncoder>,
    pub total_frames:               u64,
    pub clip_info:                  ClipInfo,
    attempted_cancel:               bool,
    shared_progress:                SharedProgress<ParallelEncoderState>,
    cached_state:                   ParallelEncoderState,
}

impl TuiApp for ParallelEncoderApp {
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
                && input_tx.send(ParallelEncoderAppEvent::Input(key)).is_err()
            {
                break;
            }
        });
        let tick_tx = event_tx.clone();
        thread::spawn(move || loop {
            if tick_tx.send(ParallelEncoderAppEvent::Tick).is_err() {
                break;
            }
            thread::sleep(Duration::from_millis(33));
        });
        let shared_progress = self.shared_progress.clone();
        thread::spawn(move || {
            for progress in progress_rx {
                match progress {
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
                            let scene_original_index =
                                id.parse::<u64>().expect("Scene index is a number");
                            let bytes = completed as u64;
                            let _ = event_tx.send(ParallelEncoderAppEvent::SceneBytes(
                                scene_original_index,
                                bytes,
                            ));
                        }
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
                            let scene_original_index =
                                id.parse::<u64>().expect("Scene index is a number");
                            let (current_pass, total_passes) = passes;
                            let (current_frame, _total_frames) = frames;

                            let _ = event_tx.send(ParallelEncoderAppEvent::SceneProgress {
                                scene: scene_original_index,
                                current_pass,
                                total_passes,
                                current_frame,
                                total_frames: _total_frames,
                            });

                            shared_progress.apply(|state| {
                                state
                                    .active_scenes
                                    .entry(scene_original_index)
                                    .and_modify(|info| {
                                        info.current_pass = current_pass;
                                        info.total_passes = total_passes;
                                        info.frames_processed = current_frame;
                                    })
                                    .or_insert_with(|| SceneProgressInfo {
                                        scene_index: scene_original_index,
                                        current_pass,
                                        total_passes,
                                        frames_processed: current_frame,
                                        total_frames: _total_frames,
                                        started: std::time::Instant::now(),
                                    });
                                true // dirty
                            });
                        },
                        Status::Completed {
                            id,
                        } => {
                            let scene_original_index =
                                id.parse::<u64>().expect("Scene index is a number");

                            shared_progress.apply(|state| {
                                state.active_scenes.remove(&scene_original_index);
                                true
                            });
                            let _ = event_tx.send(ParallelEncoderAppEvent::SceneCompleted(
                                scene_original_index,
                            ));
                        },
                        _ => {},
                    },
                }
            }
            let _ = event_tx.send(ParallelEncoderAppEvent::Quit);
        });

        let stdout_is_terminal = std::io::stdout().is_terminal();
        let mut terminal = self.init()?;
        'event_loop: loop {
            while let Ok(ParallelEncoderAppEvent::Input(key)) = event_rx.try_recv() {
                if Self::handle_ctrl_c(
                    key,
                    &mut self.attempted_cancel,
                    &cancelled,
                    &mut terminal,
                    stdout_is_terminal,
                )? {
                    break 'event_loop;
                }
            }

            if let Some(snapshot) = self.shared_progress.read_if_dirty() {
                self.cached_state = snapshot;
            }

            match event_rx.recv_timeout(Duration::from_millis(33)) {
                Ok(ParallelEncoderAppEvent::Tick) => {
                    self.drain_scene_progress(&event_rx);
                    terminal.draw(|f| self.render(f))?;
                },
                Ok(ParallelEncoderAppEvent::Input(key)) => {
                    if Self::handle_ctrl_c(
                        key,
                        &mut self.attempted_cancel,
                        &cancelled,
                        &mut terminal,
                        stdout_is_terminal,
                    )? {
                        break 'event_loop;
                    }
                },
                Ok(ParallelEncoderAppEvent::SceneProgress {
                    scene,
                    current_pass,
                    total_passes,
                    current_frame,
                    total_frames,
                }) => {
                    if current_pass == total_passes && self.active_scenes.contains_key(&scene) {
                        self.scenes.entry(scene).and_modify(|(completed, _)| {
                            *completed = current_frame;
                        });
                    }

                    if let Some(active_scene) = self.active_scenes.get_mut(&scene) {
                        active_scene.current_pass = current_pass;
                        active_scene.total_passes = total_passes;
                        active_scene.frames_processed = current_frame;
                        active_scene.total_frames = total_frames;
                    } else {
                        let scene_encoder = SceneEncoder {
                            scene: self.scenes.get(&scene).expect("Scene exists").1.clone(),
                            started: std::time::Instant::now(),
                            current_pass,
                            total_passes,
                            frames_processed: current_frame,
                            total_frames,
                        };
                        self.active_scenes.insert(scene, scene_encoder);
                    }
                },
                Ok(ParallelEncoderAppEvent::SceneCompleted(scene)) => {
                    self.scenes.entry(scene).and_modify(|(completed, scene)| {
                        *completed = (scene.end_frame - scene.start_frame) as u64;
                    });
                    self.active_scenes.remove(&scene);
                    self.cached_state.completed_scenes_count = self
                        .scenes
                        .iter()
                        .filter(|(_, (_, s))| {
                            s.sequence_data.parallel_encoder.bytes.is_some_and(|b| b > 0)
                        })
                        .count();
                },
                Ok(ParallelEncoderAppEvent::SceneBytes(scene, bytes)) => {
                    self.scenes.entry(scene).and_modify(|(_, scene)| {
                        scene.sequence_data.parallel_encoder.bytes = Some(bytes);
                    });
                    let completed_count = self
                        .scenes
                        .iter()
                        .filter(|(_, (_, s))| {
                            s.sequence_data.parallel_encoder.bytes.is_some_and(|b| b > 0)
                        })
                        .count();
                    self.cached_state.completed_scenes_count = completed_count;
                    let (bitrate, estimated_bytes) =
                        Self::estimate_size(&self.scenes, &self.clip_info);
                    self.cached_state.estimated_bitrate = bitrate;
                    self.cached_state.estimated_bytes = estimated_bytes;
                    if !stdout_is_terminal {
                        let event = ParallelEncoderConsoleEvent::SceneSize {
                            scene_index: scene,
                            bytes,
                        };
                        println!(
                            "[Parallel Encoder][Scene Size] {}",
                            serde_json::to_string(&event)?
                        );
                    }
                },
                Ok(ParallelEncoderAppEvent::Quit) => {
                    self.restore(terminal)?;
                    break;
                },
                Err(RecvTimeoutError::Timeout) => {
                    if let Some(snapshot) = self.shared_progress.read_if_dirty() {
                        self.cached_state = snapshot;
                    }
                    self.drain_scene_progress(&event_rx);
                    terminal.draw(|f| self.render(f))?;
                },
                Err(RecvTimeoutError::Disconnected) => break,
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

        let total_frames_completed: u64 =
            self.scenes.iter().map(|(_, (completed, _))| completed).sum();
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
            &self.active_scenes,
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
            bottom_center_title: format!("{}/{} Scenes", scenes_completed, self.scenes.len()),
            unit_per_second:     "FPS".to_owned(),
            unit:                "Frame".to_owned(),
            initial_completed:   self.initial_frames,
            completed:           total_frames_completed,
            total:               total_frames,
        };
        let progress_bar = progress_bar.generate(Some(self.started));
        frame.render_widget(progress_bar, layout[2]);
    }
}

impl ParallelEncoderApp {
    fn drain_scene_progress(&mut self, event_rx: &mpsc::Receiver<ParallelEncoderAppEvent>) {
        while let Ok(ParallelEncoderAppEvent::SceneProgress {
            scene,
            current_pass,
            total_passes,
            current_frame,
            total_frames,
        }) = event_rx.try_recv()
        {
            if current_pass == total_passes && self.active_scenes.contains_key(&scene) {
                self.scenes.entry(scene).and_modify(|(completed, _)| {
                    *completed = current_frame;
                });
            }

            if let Some(active_scene) = self.active_scenes.get_mut(&scene) {
                active_scene.current_pass = current_pass;
                active_scene.total_passes = total_passes;
                active_scene.frames_processed = current_frame;
                active_scene.total_frames = total_frames;
            } else {
                let scene_encoder = SceneEncoder {
                    scene: self.scenes.get(&scene).expect("Scene exists").1.clone(),
                    started: std::time::Instant::now(),
                    current_pass,
                    total_passes,
                    frames_processed: current_frame,
                    total_frames,
                };
                self.active_scenes.insert(scene, scene_encoder);
            }
        }
    }

    pub fn new(
        workers: u8,
        encoder: Encoder,
        scenes: BTreeMap<u64, (u64, Scene<CliSequenceData>)>,
        clip_info: ClipInfo,
    ) -> ParallelEncoderApp {
        let total_frames =
            scenes.iter().map(|(_, (_, s))| (s.end_frame - s.start_frame) as u64).sum();

        let initial_state = ParallelEncoderState {
            active_scenes:          BTreeMap::new(),
            completed_scenes_count: scenes
                .iter()
                .filter(|(_, (_, s))| s.sequence_data.parallel_encoder.bytes.is_some_and(|b| b > 0))
                .count(),
            estimated_bitrate:      0.0,
            estimated_bytes:        0,
        };
        let estimate = Self::estimate_size(&scenes, &clip_info);
        let initial_state = ParallelEncoderState {
            estimated_bitrate: estimate.0,
            estimated_bytes: estimate.1,
            ..initial_state
        };

        ParallelEncoderApp {
            original_panic_hook: None,
            started: std::time::Instant::now(),
            workers,
            encoder,
            initial_frames: scenes.iter().fold(0, |acc, (_, (completed, _))| acc + completed),
            scenes,
            active_scenes: BTreeMap::new(),
            total_frames,
            clip_info,
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
                let restore = || -> Result<()> {
                    let _ = ratatui::crossterm::terminal::disable_raw_mode();
                    let _ = ratatui::crossterm::execute!(
                        std::io::stdout(),
                        ratatui::crossterm::terminal::LeaveAlternateScreen
                    );
                    Ok(())
                };
                restore()?;
                debug!("Force quit Condor");
                std::process::exit(0);
            } else if !stdout_is_terminal {
                println!("Waiting for Encoders to finish. Press Ctrl+C again to exit immediately.");
            }
        }
        Ok(false)
    }
}

enum ParallelEncoderAppEvent {
    Quit,
    Tick,                   // 30 FPS
    Input(event::KeyEvent), // Keyboard events
    SceneProgress {
        scene:         u64,
        current_pass:  u8,
        total_passes:  u8,
        current_frame: u64,
        total_frames:  u64,
    },
    SceneCompleted(u64),
    SceneBytes(u64, u64),
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
