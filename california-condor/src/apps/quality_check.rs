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
    models::{
        scene::Scene,
        sequence::target_quality::types::{ProbeStatistic, QualityMetric},
    },
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
    components::{input_info::InputInfo, progress_bar::ProgressBar},
    configuration::CliSequenceData,
};

#[derive(Clone)]
pub struct SceneReportDetails {
    pub quantizer: Option<f64>,
    pub speed:     Option<String>,
}

#[derive(Clone)]
pub struct QualityCheckState {
    pub scene_scores:    BTreeMap<u64, f64>,
    pub frame_scores:    Vec<f64>,
    pub scene_details:   BTreeMap<u64, SceneReportDetails>,
    pub frames_compared: u64,
    pub total_frames:    u64,
}

pub struct QualityCheckApp {
    pub(crate) original_panic_hook: Option<super::PanicHook>,
    pub clip_info:                  ClipInfo,
    pub metric:                     QualityMetric,
    pub pass_started:               std::time::Instant,
    attempted_cancel:               bool,
    shared_progress:                SharedProgress<QualityCheckState>,
    cached_state:                   QualityCheckState,
}

impl TuiApp for QualityCheckApp {
    fn original_panic_hook(&mut self) -> &mut Option<super::PanicHook> {
        &mut self.original_panic_hook
    }

    fn run(
        &mut self,
        progress_rx: Receiver<SequenceStatus>,
        cancelled: Arc<AtomicBool>,
    ) -> Result<()> {
        let quit = Arc::new(AtomicBool::new(false));
        let (event_tx, event_rx) = mpsc::channel();
        if !crate::apps::is_test_mode() {
            let input_tx = event_tx.clone();
            thread::spawn(move || {
                loop {
                    if let Ok(TermEvent::Key(key)) = event::read()
                        && input_tx.send(QualityCheckAppEvent::Input(key)).is_err()
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
                    if tick_tx.send(QualityCheckAppEvent::Tick).is_err() {
                        break;
                    }
                    thread::sleep(Duration::from_millis(33)); // ~30 FPS
                }
            });
        }
        let shared_progress = self.shared_progress.clone();
        let quit_flag = Arc::clone(&quit);
        thread::spawn(move || {
            for progress in progress_rx {
                match progress {
                    SequenceStatus::Whole(status) => match status {
                        Status::Processing {
                            completion:
                                SequenceCompletion::Frames {
                                    completed,
                                    total,
                                },
                            ..
                        } if !std::io::stdout().is_terminal() => {
                            let event = QualityCheckConsoleEvent::CompareProgress {
                                current_frame: completed,
                                total_frames:  total,
                            };
                            println!(
                                "[Quality Check][Compare] {}",
                                serde_json::to_string(&event).unwrap()
                            );
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
                                    SequenceCompletion::Custom {
                                        name: _,
                                        completed,
                                        total,
                                    },
                                ..
                            },
                            Status::Processing {
                                id,
                                completion:
                                    SequenceCompletion::FrameScore {
                                        frame,
                                        score,
                                    },
                            },
                        ) if id == "Quality" => {
                            shared_progress.apply(|state| {
                                state.frames_compared = completed as u64;
                                state.total_frames = total as u64;
                                state.frame_scores.push(score);
                                true
                            });
                            if !std::io::stdout().is_terminal() {
                                let event = QualityCheckConsoleEvent::FrameScore {
                                    frame,
                                    score,
                                };
                                println!(
                                    "[Quality Check][Frame] {}",
                                    serde_json::to_string(&event).unwrap()
                                );
                            }
                        },
                        (
                            Status::Processing {
                                completion:
                                    SequenceCompletion::Custom {
                                        name: _,
                                        completed,
                                        total,
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
                        ) if id == "Quality" => {
                            shared_progress.apply(|state| {
                                state.frames_compared = completed as u64;
                                state.total_frames = total as u64;
                                state.scene_scores.insert(index, score);
                                true
                            });
                            if !std::io::stdout().is_terminal() {
                                let event = QualityCheckConsoleEvent::SceneScore {
                                    scene: index,
                                    score,
                                };
                                println!(
                                    "[Quality Check][Scene] {}",
                                    serde_json::to_string(&event).unwrap()
                                );
                            }
                        },
                        _ => {},
                    },
                }
            }
            let _ = event_tx.send(QualityCheckAppEvent::Quit);
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
            if self.cached_state.frames_compared == self.cached_state.total_frames
                && !self.cached_state.scene_scores.is_empty()
            {
                self.print_report();
            }
            return Ok(());
        }

        let stdout_is_terminal = std::io::stdout().is_terminal();
        let mut terminal = self.init()?;
        'event_loop: loop {
            while let Ok(QualityCheckAppEvent::Input(key)) = event_rx.try_recv() {
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
                self.cached_state = snapshot;
            }

            if quit.load(Ordering::Acquire) {
                self.cached_state = self.shared_progress.read();
                terminal.draw(|f| self.render(f))?;
                self.restore(terminal)?;
                break;
            }
            match event_rx.recv_timeout(Duration::from_millis(33)) {
                Ok(QualityCheckAppEvent::Tick) => {
                    terminal.draw(|f| self.render(f))?;
                },
                Ok(QualityCheckAppEvent::Input(key)) => {
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
                Ok(QualityCheckAppEvent::Quit) => {
                    self.cached_state = self.shared_progress.read();
                    terminal.draw(|f| self.render(f))?;
                    self.restore(terminal)?;
                    break;
                },
                Err(RecvTimeoutError::Timeout) => {
                    terminal.draw(|f| self.render(f))?;
                },
                Err(RecvTimeoutError::Disconnected) => {
                    self.cached_state = self.shared_progress.read();
                    terminal.draw(|f| self.render(f))?;
                    self.restore(terminal)?;
                    break;
                },
            }
        }

        if self.cached_state.frames_compared == self.cached_state.total_frames
            && !self.cached_state.scene_scores.is_empty()
        {
            self.print_report();
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
            .title_bottom(Line::from("Quality Check").centered());
        let top_info_inner = top_info.inner(layout[0]);
        let top_info_areas =
            Layout::vertical([Constraint::Fill(1), Constraint::Fill(1)]).split(top_info_inner);
        frame.render_widget(top_info, layout[0]);
        let input_info = InputInfo::new(self.clip_info);
        let input_info = input_info.generate(false);
        frame.render_widget(input_info, top_info_areas[0]);

        let state = &self.cached_state;
        let scene_scores = state
            .scene_scores
            .iter()
            .map(|(index, score)| (*index as f64, *score))
            .collect::<Vec<_>>();
        let datasets = vec![
            Dataset::default()
                .name("Scene Score")
                .style(Color::Blue)
                .graph_type(ratatui::widgets::GraphType::Scatter)
                .data(&scene_scores),
        ];
        let max_scenes_label = format!("{}", state.scene_scores.len().saturating_sub(1));
        let max_score = scene_scores.iter().map(|(_, s)| *s).fold(0.0_f64, f64::max);
        let max_score = (max_score / 10.0).ceil() * 10.0; // Round up to nearest 10
        let max_score_label = format!("{}", max_score);
        let chart = Chart::new(datasets)
            .block(
                Block::bordered()
                    .border_type(ratatui::widgets::BorderType::Rounded)
                    .title(Line::from("Quality Score per Scene").centered()),
            )
            .x_axis(
                Axis::default()
                    .title("Scene")
                    .bounds([0.0, state.scene_scores.len() as f64])
                    .labels(["0", &max_scenes_label]),
            )
            .y_axis(
                Axis::default()
                    .title("Score")
                    .bounds([0.0, max_score])
                    .labels(["0", &max_score_label]),
            );
        frame.render_widget(chart, layout[1]);

        let progress_bar = ProgressBar {
            color:               MAIN_COLOR,
            processing_title:    if self.attempted_cancel {
                "Shutting down...".to_owned()
            } else {
                "Comparing".to_owned()
            },
            completed_title:     if self.attempted_cancel {
                "Quality Check Aborted".to_owned()
            } else {
                "Quality Check Completed".to_owned()
            },
            top_right_title:     String::new(),
            bottom_center_title: String::new(),
            unit_per_second:     "FPS".to_owned(),
            unit:                "Frame".to_owned(),
            initial_completed:   0,
            completed:           state.frames_compared,
            total:               state.total_frames,
            show_label:          true,
        };
        let progress_bar = progress_bar.generate(Some(self.pass_started));
        frame.render_widget(progress_bar, layout[2]);
    }
}

impl QualityCheckApp {
    pub fn new(
        clip_info: ClipInfo,
        scenes: &[Scene<CliSequenceData>],
        metric: QualityMetric,
        statistic: ProbeStatistic,
    ) -> QualityCheckApp {
        let scores: BTreeMap<u64, Vec<f64>> = scenes
            .iter()
            .enumerate()
            .map(|(scene_index, scene)| {
                (
                    scene_index as u64,
                    scene.sequence_data.quality_check.quality.scores.clone(),
                )
            })
            .collect();
        let scene_scores: BTreeMap<u64, f64> = scores
            .iter()
            .filter_map(|(index, scores)| {
                if scores.is_empty() {
                    None
                } else {
                    Some((*index, statistic.calculate(scores)))
                }
            })
            .collect();
        let scene_details: BTreeMap<u64, SceneReportDetails> = scenes
            .iter()
            .enumerate()
            .map(|(scene_index, scene)| {
                (scene_index as u64, SceneReportDetails {
                    quantizer: scene.encoder.quantizer(),
                    speed:     scene.encoder.speed(),
                })
            })
            .collect();
        let total_frames = scores.values().fold(0, |acc, scores| acc + scores.len() as u64);
        let frame_scores: Vec<f64> = scores.into_values().flatten().collect();
        let frames_compared = frame_scores.len() as u64;
        let state = QualityCheckState {
            scene_scores,
            frame_scores,
            scene_details,
            frames_compared,
            total_frames,
        };
        QualityCheckApp {
            original_panic_hook: None,
            clip_info,
            metric,
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
                println!("Press Ctrl+C again to exit immediately.");
            }
        }
        Ok(false)
    }

    fn compute_stats(scores: &[f64], is_inverse: bool) -> Option<ReportStats> {
        if scores.is_empty() {
            return None;
        }
        let count = scores.len();
        let average = scores.iter().sum::<f64>() / count as f64;
        let variance =
            scores.iter().map(|s| (s - average) * (s - average)).sum::<f64>() / count as f64;
        let std_dev = variance.sqrt();
        let mut sorted = scores.to_vec();
        sorted.sort_by(f64::total_cmp);
        let minimum = sorted.first().copied().unwrap_or(0.0);
        let maximum = sorted.last().copied().unwrap_or(0.0);
        // 1% worst percentile
        let percentile_index = if is_inverse {
            ((count as f64 * 0.99).ceil() as usize).saturating_sub(1).min(count - 1)
        } else {
            ((count as f64 * 0.01).floor() as usize).min(count - 1)
        };
        let percentile = sorted[percentile_index];
        // Count scores outside 2 standard deviations from the average
        let outside_two_sigma =
            scores.iter().filter(|s| (**s - average).abs() > 2.0 * std_dev).count();
        Some(ReportStats {
            count,
            average,
            minimum,
            maximum,
            percentile,
            std_dev,
            outside_two_sigma,
        })
    }

    /// Scenes scoring outside 2 sigma
    fn poor_scenes(&self) -> Vec<(u64, f64)> {
        let is_inverse = self.metric.is_inverse_metric();
        let scores: Vec<f64> = self.cached_state.scene_scores.values().copied().collect();
        let Some(stats) = Self::compute_stats(&scores, is_inverse) else {
            return Vec::new();
        };
        let threshold_low = 2.0f64.mul_add(-stats.std_dev, stats.average);
        let threshold_high = 2.0f64.mul_add(stats.std_dev, stats.average);
        let mut poor = self
            .cached_state
            .scene_scores
            .iter()
            .filter_map(|(index, score)| {
                let is_poor = if is_inverse {
                    *score > threshold_high
                } else {
                    *score < threshold_low
                };
                is_poor.then_some((*index, *score))
            })
            .collect::<Vec<_>>();
        // Sort by worst score
        poor.sort_by(|a, b| {
            if is_inverse {
                b.1.total_cmp(&a.1)
            } else {
                a.1.total_cmp(&b.1)
            }
        });
        poor
    }

    /// Print the quality report as aligned plain-text tables
    fn print_report(&self) {
        let metric_name = self.metric.friendly_name();
        let is_inverse = self.metric.is_inverse_metric();
        let percentile_label = if is_inverse { "1% High" } else { "1% Low" };

        println!();
        println!("Quality Check - {metric_name}");

        let scene_scores: Vec<f64> = self.cached_state.scene_scores.values().copied().collect();
        let frame_scores = &self.cached_state.frame_scores;
        let scene_stats = Self::compute_stats(&scene_scores, is_inverse);
        let frame_stats = Self::compute_stats(frame_scores, is_inverse);

        // Stats table: header + Per-Scene + Per-Frame rows
        println!(
            "{:<14}{:>12}{:>12}{:>12}{:>12}{:>12}",
            "Scope", "Average", "Minimum", "Maximum", percentile_label, "Std Dev (σ)"
        );
        for (label, stats) in
            [("Per-Scene", scene_stats.as_ref()), ("Per-Frame", frame_stats.as_ref())]
        {
            if let Some(s) = stats {
                println!(
                    "{:<14}{:>12.4}{:>12.4}{:>12.4}{:>12.4}{:>12.4}",
                    label, s.average, s.minimum, s.maximum, s.percentile, s.std_dev
                );
            } else {
                println!("{label:<14}  (no data)");
            }
        }
        println!();

        // Scenes outside 2 sigma
        if let Some(scene_stats) = &scene_stats {
            println!(
                "Scenes outside 2σ: {} of {}",
                scene_stats.outside_two_sigma, scene_stats.count
            );
        }
        println!();

        // Poor scenes table
        let poor = self.poor_scenes();
        if !poor.is_empty() {
            println!("Scenes Outside 2σ ({})", poor.len());
            println!("{}", "─".repeat(72));
            println!(
                "{:<8}{:>12}{:>14}{:>12}",
                "Scene", "Score", "Speed", "Quantizer"
            );
            for (index, score) in &poor {
                let details = self.cached_state.scene_details.get(index);
                let speed = details.and_then(|d| d.speed.as_deref()).unwrap_or("-");
                let quantizer = details
                    .and_then(|d| d.quantizer)
                    .map_or_else(|| "-".to_owned(), |q| format!("{q:.4}"));
                println!("{index:<8}{score:>12.4}{speed:>14}{quantizer:>12}");
            }
        }
    }
}

struct ReportStats {
    count:             usize,
    average:           f64,
    minimum:           f64,
    maximum:           f64,
    /// 1% worst
    percentile:        f64,
    std_dev:           f64,
    outside_two_sigma: usize,
}

enum QualityCheckAppEvent {
    Quit,
    Tick,                   // 30 FPS
    Input(event::KeyEvent), // Keyboard events
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum QualityCheckConsoleEvent {
    FrameScore {
        frame: u64,
        score: f64,
    },
    SceneScore {
        scene: u64,
        score: f64,
    },
    CompareProgress {
        current_frame: u64,
        total_frames:  u64,
    },
}
