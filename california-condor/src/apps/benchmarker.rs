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
        sequence::{
            SequenceCompletion,
            SequenceDetails,
            SequenceStatus,
            Status,
            benchmarker::Benchmarker,
        },
    },
    models::encoder::Encoder,
};
use anyhow::Result;
use ratatui::{
    Frame,
    crossterm::event::{self, Event as TermEvent, KeyCode, KeyModifiers},
    layout::{Constraint, Layout},
    text::Line,
    widgets::{Block, Cell, Row, Table},
};
use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::{
    apps::{TuiApp, shared_progress::SharedProgress},
    components::{encoder_info::EncoderInfo, input_info::InputInfo},
};
#[derive(Clone)]
pub struct BenchmarkerState {
    pub results: BTreeMap<u8, WorkerStatus>,
}

pub struct BenchmarkerApp {
    pub(crate) original_panic_hook: Option<super::PanicHook>,
    pub encoder:                    Encoder,
    pub clip_info:                  ClipInfo,
    attempted_cancel:               bool,
    shared_progress:                SharedProgress<BenchmarkerState>,
    cached_state:                   BenchmarkerState,
}

impl TuiApp for BenchmarkerApp {
    fn original_panic_hook(&mut self) -> &mut Option<super::PanicHook> {
        &mut self.original_panic_hook
    }

    fn run(
        &mut self,
        progress_rx: Receiver<SequenceStatus>,
        cancelled: Arc<AtomicBool>,
    ) -> Result<()> {
        const DETAILS: SequenceDetails = Benchmarker::DETAILS;
        let (event_tx, event_rx) = mpsc::channel();
        let input_tx = event_tx.clone();
        thread::spawn(move || {
            loop {
                if let Ok(TermEvent::Key(key)) = event::read()
                    && input_tx.send(BenchmarkerAppEvent::Input(key)).is_err()
                {
                    break;
                }
            }
        });
        let tick_tx = event_tx.clone();
        thread::spawn(move || {
            loop {
                if tick_tx.send(BenchmarkerAppEvent::Tick).is_err() {
                    break;
                }
                thread::sleep(Duration::from_millis(33)); // ~30 FPS
            }
        });
        let shared_progress = self.shared_progress.clone();
        let quit = Arc::new(AtomicBool::new(false));
        let quit_flag = Arc::clone(&quit);
        thread::spawn(move || {
            for progress in progress_rx {
                match progress {
                    SequenceStatus::Whole(status) => match status {
                        Status::Completed {
                            id,
                        } => {
                            if id == DETAILS.name {
                                let _ = event_tx.send(BenchmarkerAppEvent::Quit);
                            } else {
                                let workers = id.parse::<u8>().expect("Workers is a number");
                                shared_progress.apply(|state| {
                                    state.results.entry(workers).and_modify(|ws| ws.added = true);
                                    true
                                });
                                if !std::io::stdout().is_terminal() {
                                    let event = BenchmarkerConsoleEvent::WorkerAdded {
                                        worker: workers,
                                    };
                                    println!(
                                        "[Benchmarker][Worker Added] {}",
                                        serde_json::to_string(&event).unwrap()
                                    );
                                }
                            }
                        },
                        Status::Failed {
                            id,
                            error,
                        } => {
                            let workers = id.parse::<u8>().expect("Workers is a number");
                            shared_progress.apply(|state| {
                                state
                                    .results
                                    .entry(workers)
                                    .and_modify(|ws| ws.failed_reason = Some(error.clone()));
                                true
                            });
                            if !std::io::stdout().is_terminal() {
                                let event = BenchmarkerConsoleEvent::WorkerFailed {
                                    worker: workers,
                                    error,
                                };
                                println!(
                                    "[Benchmarker][Worker Failed] {}",
                                    serde_json::to_string(&event).unwrap()
                                );
                            }
                        },
                        _ => {},
                    },
                    SequenceStatus::Subprocess {
                        parent: _,
                        child,
                    } => match child {
                        Status::Processing {
                            id,
                            completion:
                                SequenceCompletion::Frames {
                                    completed,
                                    total,
                                },
                        } => {
                            let workers = id.parse::<u8>().expect("Workers is a number");
                            shared_progress.apply(|state| {
                                state
                                    .results
                                    .entry(workers)
                                    .and_modify(|ws| {
                                        ws.current_frame = completed;
                                        ws.total_frames = total;
                                    })
                                    .or_insert_with(|| WorkerStatus {
                                        started:       std::time::Instant::now(),
                                        added:         false,
                                        finished:      None,
                                        failed_reason: None,
                                        current_frame: completed,
                                        total_frames:  total,
                                    });
                                true
                            });
                            if !std::io::stdout().is_terminal() {
                                let event = BenchmarkerConsoleEvent::Progress {
                                    worker:        workers,
                                    current_frame: completed,
                                    total_frames:  total,
                                };
                                println!(
                                    "[Benchmarker][Progress] {}",
                                    serde_json::to_string(&event).unwrap()
                                );
                            }
                        },
                        Status::Completed {
                            id,
                        } => {
                            let workers = id.parse::<u8>().expect("Workers is a number");
                            shared_progress.apply(|state| {
                                state
                                    .results
                                    .entry(workers)
                                    .and_modify(|ws| ws.finished = Some(std::time::Instant::now()));
                                true
                            });
                            if !std::io::stdout().is_terminal() {
                                let event = BenchmarkerConsoleEvent::WorkerCompleted {
                                    worker: workers,
                                };
                                println!(
                                    "[Benchmarker][Worker Completed] {}",
                                    serde_json::to_string(&event).unwrap()
                                );
                            }
                        },
                        _ => {},
                    },
                }
            }
            let _ = event_tx.send(BenchmarkerAppEvent::Quit);
            quit_flag.store(true, Ordering::Release);
        });

        let stdout_is_terminal = std::io::stdout().is_terminal();
        let mut terminal = self.init()?;
        'event_loop: loop {
            while let Ok(BenchmarkerAppEvent::Input(key)) = event_rx.try_recv() {
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
                    let _ = terminal.draw(|f| self.render(f));
                    self.restore(terminal)?;
                    break 'event_loop;
                }
            }

            if let Some(snapshot) = self.shared_progress.read_if_dirty() {
                self.cached_state = snapshot;
            }

            if quit.load(Ordering::Acquire) {
                self.cached_state = self.shared_progress.read();
                let _ = terminal.draw(|f| self.render(f));
                self.restore(terminal)?;
                break;
            }

            match event_rx.recv_timeout(Duration::from_millis(33)) {
                Ok(BenchmarkerAppEvent::Tick) => {
                    terminal.draw(|f| self.render(f))?;
                },
                Ok(BenchmarkerAppEvent::Input(key)) => {
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
                        let _ = terminal.draw(|f| self.render(f));
                        self.restore(terminal)?;
                        break 'event_loop;
                    }
                },
                Ok(BenchmarkerAppEvent::Quit) => {
                    if let Some(snapshot) = self.shared_progress.read_if_dirty() {
                        self.cached_state = snapshot;
                    }
                    let _ = terminal.draw(|f| self.render(f));
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
                    let _ = terminal.draw(|f| self.render(f));
                    self.restore(terminal)?;
                    break;
                },
            }
        }
        Ok(())
    }

    fn render(&self, frame: &mut Frame) {
        let layout = Layout::default()
            .constraints([Constraint::Percentage(20), Constraint::Percentage(80)])
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

        let rows = self
            .cached_state
            .results
            .iter()
            .map(|(worker, status)| {
                Row::new(vec![
                    Cell::from(Line::from(worker.to_string()).centered()),
                    Cell::from(Line::from(status.fps_or_error()).centered()),
                    Cell::from(Line::from(status.status()).centered()),
                    Cell::from(
                        Line::from(format!("{}/{}", status.current_frame, status.total_frames))
                            .centered(),
                    ),
                ])
            })
            .collect::<Vec<_>>();
        let table = Table::new(rows, [
            Constraint::Fill(1),
            Constraint::Fill(1),
            Constraint::Fill(1),
            Constraint::Fill(1),
        ])
        .header(Row::new(vec![
            Cell::from(Line::from("Workers").centered()),
            Cell::from(Line::from("FPS").centered()),
            Cell::from(Line::from("Meets Threshold").centered()),
            Cell::from(Line::from("Frames").centered()),
        ]))
        .block(Block::bordered().title_bottom(Line::from("Benchmarking...").centered()));
        frame.render_widget(table, layout[1]);
    }
}

impl BenchmarkerApp {
    pub fn new(encoder: Encoder, clip_info: ClipInfo) -> BenchmarkerApp {
        let state = BenchmarkerState {
            results: BTreeMap::new(),
        };
        BenchmarkerApp {
            original_panic_hook: None,
            encoder,
            clip_info,
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
                println!(
                    "Waiting for Benchmarker to finish. Press Ctrl+C again to exit immediately."
                );
            }
        }
        Ok(false)
    }
}

#[derive(Debug, Clone)]
pub struct WorkerStatus {
    pub started:       std::time::Instant,
    pub finished:      Option<std::time::Instant>,
    pub added:         bool,
    pub failed_reason: Option<String>,
    pub current_frame: u64,
    pub total_frames:  u64,
}

impl WorkerStatus {
    fn fps_or_error(&self) -> String {
        if self.current_frame > 0 {
            let finished = self.finished.unwrap_or_else(std::time::Instant::now);
            let fps =
                self.current_frame as f64 / finished.duration_since(self.started).as_secs_f64();
            format!("{:.2} FPS", fps)
        } else {
            "-".to_owned()
        }
    }

    fn status(&self) -> String {
        if self.failed_reason.is_some() {
            "✕".to_owned()
        } else if self.added {
            "✓".to_owned()
        } else {
            "-".to_owned()
        }
    }
}

enum BenchmarkerAppEvent {
    Quit,
    Tick,                   // 30 FPS
    Input(event::KeyEvent), // Keyboard events
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum BenchmarkerConsoleEvent {
    Progress {
        worker:        u8,
        current_frame: u64,
        total_frames:  u64,
    },
    WorkerCompleted {
        worker: u8,
    },
    WorkerAdded {
        worker: u8,
    },
    WorkerFailed {
        worker: u8,
        error:  String,
    },
}
