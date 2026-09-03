use std::{
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
    models::sequence::scene_concatenator::ConcatMethod,
};
use anyhow::Result;
use ratatui::{
    Frame,
    crossterm::event::{self, Event as TermEvent, KeyCode, KeyModifiers},
    layout::{Constraint, Layout},
    style::Color,
    text::Line,
    widgets::Block,
};
use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::{
    apps::{TuiApp, shared_progress::SharedProgress},
    components::{input_info::InputInfo, progress_bar::ProgressBar},
};

#[derive(Clone)]
pub struct SceneConcatenatorState {
    pub percent: f64,
}

pub struct SceneConcatenatorApp {
    pub(crate) original_panic_hook: Option<super::PanicHook>,
    pub started:                    std::time::Instant,
    pub clip_info:                  ClipInfo,
    pub method:                     ConcatMethod,
    pub scenes_len:                 usize,
    attempted_cancel:               bool,
    shared_progress:                SharedProgress<SceneConcatenatorState>,
    cached_state:                   SceneConcatenatorState,
}

impl TuiApp for SceneConcatenatorApp {
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
                        && input_tx.send(SceneConcatenatorAppEvent::Input(key)).is_err()
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
                    if tick_tx.send(SceneConcatenatorAppEvent::Tick).is_err() {
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
                if let SequenceStatus::Whole(Status::Processing {
                    completion, ..
                }) = progress
                    && let SequenceCompletion::Percentage(percentage) = completion
                {
                    shared_progress.apply(|state| {
                        state.percent = percentage;
                        true
                    });
                    if !std::io::stdout().is_terminal() {
                        let event = SceneConcatenatorConsoleEvent::Processed(percentage);
                        let event = serde_json::to_string(&event).unwrap();
                        println!("[Scene Concatenator][Progress]: {}", event);
                    }
                }
            }
            let _ = event_tx.send(SceneConcatenatorAppEvent::Quit);
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

        let mut terminal = self.init()?;
        let stdout_is_terminal = std::io::stdout().is_terminal();
        'event_loop: loop {
            while let Ok(SceneConcatenatorAppEvent::Input(key)) = event_rx.try_recv() {
                if Self::handle_ctrl_c(
                    key,
                    &mut self.attempted_cancel,
                    &cancelled,
                    stdout_is_terminal,
                ) {
                    if let Some(snapshot) = self.shared_progress.read_if_dirty() {
                        self.cached_state = snapshot;
                    }
                    terminal.draw(|f| self.render(f))?;
                    self.restore(terminal)?;
                    break 'event_loop;
                }
            }

            if let Some(snapshot) = self.shared_progress.read_if_dirty() {
                self.cached_state = snapshot;
            }

            if quit.load(Ordering::Acquire) {
                self.cached_state = self.shared_progress.read();
                terminal.draw(|f| self.render(f))?;
                self.restore(terminal)?;
                break;
            }

            match event_rx.recv_timeout(Duration::from_millis(33)) {
                Ok(SceneConcatenatorAppEvent::Tick) => {
                    terminal.draw(|f| self.render(f))?;
                },
                Ok(SceneConcatenatorAppEvent::Input(key)) => {
                    if Self::handle_ctrl_c(
                        key,
                        &mut self.attempted_cancel,
                        &cancelled,
                        stdout_is_terminal,
                    ) {
                        if let Some(snapshot) = self.shared_progress.read_if_dirty() {
                            self.cached_state = snapshot;
                        }
                        terminal.draw(|f| self.render(f))?;
                        self.restore(terminal)?;
                        break 'event_loop;
                    }
                },
                Ok(SceneConcatenatorAppEvent::Quit) => {
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
                Constraint::Percentage(10),
                Constraint::Percentage(80),
                Constraint::Percentage(10),
            ])
            .split(frame.area());

        let input_info = InputInfo::new(self.clip_info);
        let input_info = input_info.generate(false);
        let input_block = Block::bordered()
            .border_type(ratatui::widgets::BorderType::Rounded)
            .title(Line::from("Input").centered())
            .title_bottom(Line::from(self.method.to_string()).centered());
        let input_info = input_info.block(input_block);
        frame.render_widget(input_info, layout[0]);

        let progress_bar = ProgressBar {
            color:               MAIN_COLOR,
            processing_title:    if self.attempted_cancel {
                "Waiting for Concatenation to Finish...".to_owned()
            } else {
                "Concatenating Scenes...".to_owned()
            },
            completed_title:     if self.attempted_cancel {
                "Concatenation Aborted".to_owned()
            } else {
                "Concatenation Completed".to_owned()
            },
            top_right_title:     format!("{} scenes", self.scenes_len),
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

impl SceneConcatenatorApp {
    pub fn new(
        clip_info: ClipInfo,
        scenes_len: usize,
        method: ConcatMethod,
    ) -> SceneConcatenatorApp {
        let state = SceneConcatenatorState {
            percent: 0.0
        };
        SceneConcatenatorApp {
            original_panic_hook: None,
            started: std::time::Instant::now(),
            clip_info,
            method,
            scenes_len,
            attempted_cancel: false,
            shared_progress: SharedProgress::new(state.clone()),
            cached_state: state,
        }
    }

    fn handle_ctrl_c(
        key: ratatui::crossterm::event::KeyEvent,
        attempted_cancel: &mut bool,
        cancelled: &Arc<AtomicBool>,
        stdout_is_terminal: bool,
    ) -> bool {
        if key.code == KeyCode::Char('c')
            && key.modifiers.contains(KeyModifiers::CONTROL)
            && key.is_press()
        {
            *attempted_cancel = true;
            let already_cancelled = cancelled.swap(true, Ordering::SeqCst);
            if already_cancelled {
                debug!("Force quit Condor");
                return true;
            } else if !stdout_is_terminal {
                println!(
                    "Waiting for Concatenation to finish. Press Ctrl+C again to exit immediately."
                );
            }
        }
        false
    }
}

enum SceneConcatenatorAppEvent {
    Quit,
    Tick,                   // 30 FPS
    Input(event::KeyEvent), // Keyboard events
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum SceneConcatenatorConsoleEvent {
    Processed(f64),
}
