use std::{collections::BTreeMap, io::IsTerminal};

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
    components::{input_info::InputInfo, progress_bar::ProgressBar},
    configuration::CliSequenceData,
};

#[derive(Clone)]
pub struct SceneReportDetails {
    pub quantizer: Option<f64>,
    pub speed: Option<String>,
}

#[derive(Clone)]
pub struct QualityCheckState {
    pub scene_scores: BTreeMap<u64, f64>,
    pub frame_scores: Vec<f64>,
    pub scene_details: BTreeMap<u64, SceneReportDetails>,
    pub frames_compared: u64,
    pub total_frames: u64,
}

pub struct QualityCheckApp {
    pub clip_info: ClipInfo,
    pub metric: QualityMetric,
    pub pass_started: std::time::Instant,
    attempted_cancel: bool,
    shared_progress: SharedProgress<QualityCheckState>,
    cached_state: QualityCheckState,
}

impl TuiApp for QualityCheckApp {
    type State = QualityCheckState;

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
        "Press Ctrl+C again to exit immediately."
    }

    fn map_progress(status: SequenceStatus, state: &mut Self::State) -> bool {
        match status {
            SequenceStatus::Whole(status) => match status {
                Status::Processing {
                    completion:
                        SequenceCompletion::Custom {
                            completed, total, ..
                        },
                    ..
                } => {
                    // VMAF submits progress before its final scores are available.
                    state.frames_compared = completed as u64;
                    state.total_frames = total as u64;
                    if !std::io::stdout().is_terminal() {
                        let event = QualityCheckConsoleEvent::CompareProgress {
                            current_frame: state.frames_compared,
                            total_frames: state.total_frames,
                        };
                        println!(
                            "[Quality Check][Compare] {}",
                            serde_json::to_string(&event).unwrap()
                        );
                    }
                    true
                },
                Status::Processing {
                    completion: SequenceCompletion::Frames { completed, total },
                    ..
                } if !std::io::stdout().is_terminal() => {
                    let event = QualityCheckConsoleEvent::CompareProgress {
                        current_frame: completed,
                        total_frames: total,
                    };
                    println!(
                        "[Quality Check][Compare] {}",
                        serde_json::to_string(&event).unwrap()
                    );
                    false
                },
                _ => false,
            },
            SequenceStatus::Subprocess { parent, child } => match (parent, child) {
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
                        completion: SequenceCompletion::FrameScore { frame, score },
                    },
                ) if id == "Quality" => {
                    state.frames_compared = completed as u64;
                    state.total_frames = total as u64;
                    state.frame_scores.push(score);
                    if !std::io::stdout().is_terminal() {
                        let event = QualityCheckConsoleEvent::FrameScore { frame, score };
                        println!(
                            "[Quality Check][Frame] {}",
                            serde_json::to_string(&event).unwrap()
                        );
                    }
                    true
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
                        completion: SequenceCompletion::SceneQuality { index, score, .. },
                    },
                ) if id == "Quality" => {
                    state.frames_compared = completed as u64;
                    state.total_frames = total as u64;
                    state.scene_scores.insert(index, score);
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
                    true
                },
                _ => false,
            },
        }
    }

    fn after_loop(&mut self) {
        if self.cached_state.frames_compared == self.cached_state.total_frames
            && !self.cached_state.scene_scores.is_empty()
        {
            self.print_report();
        }
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
                .style(theme.accent_blue)
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
            color: main,
            processing_title: if self.attempted_cancel {
                "Shutting down...".to_owned()
            } else {
                "Comparing".to_owned()
            },
            completed_title: if self.attempted_cancel {
                "Quality Check Aborted".to_owned()
            } else {
                "Quality Check Completed".to_owned()
            },
            top_right_title: String::new(),
            bottom_center_title: String::new(),
            unit_per_second: "FPS".to_owned(),
            unit: "Frame".to_owned(),
            initial_completed: 0,
            completed: state.frames_compared,
            total: state.total_frames,
            show_label: true,
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
                    Some((*index, metric.calculate_score(statistic, scores)))
                }
            })
            .collect();
        let scene_details: BTreeMap<u64, SceneReportDetails> = scenes
            .iter()
            .enumerate()
            .map(|(scene_index, scene)| {
                (
                    scene_index as u64,
                    SceneReportDetails {
                        quantizer: scene.encoder.quantizer(),
                        speed: scene.encoder.speed(),
                    },
                )
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
            clip_info,
            metric,
            pass_started: std::time::Instant::now(),
            attempted_cancel: false,
            shared_progress: SharedProgress::new(state.clone()),
            cached_state: state,
        }
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
        let frame_stats = if self.metric.is_cumulative() {
            None
        } else {
            Self::compute_stats(frame_scores, is_inverse)
        };

        // Stats table: header + Per-Scene + Per-Frame rows
        println!(
            "{:<14}{:>12}{:>12}{:>12}{:>12}{:>12}",
            "Scope", "Average", "Minimum", "Maximum", percentile_label, "Std Dev (σ)"
        );
        for (label, stats) in
            [("Per-Scene", scene_stats.as_ref()), ("Per-Frame", frame_stats.as_ref())]
        {
            if label == "Per-Frame" && self.metric.is_cumulative() {
                continue;
            }
            if let Some(s) = stats {
                println!(
                    "{:<14}{:>12.4}{:>12.4}{:>12.4}{:>12.4}{:>12.4}",
                    label, s.average, s.minimum, s.maximum, s.percentile, s.std_dev
                );
            } else {
                println!("{label:<14}  (no data)");
            }
        }
        if self.metric.is_cumulative() {
            println!(
                "Per-Scene statistics describe independent scene results, not a clip-wide CVVDP score."
            );
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
    count: usize,
    average: f64,
    minimum: f64,
    maximum: f64,
    /// 1% worst
    percentile: f64,
    std_dev: f64,
    outside_two_sigma: usize,
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
        total_frames: u64,
    },
}

#[cfg(test)]
mod tests {
    use std::{
        process::{Command, Stdio},
        time::{Duration, Instant},
    };

    use super::*;

    #[test]
    fn m27_saved_qc_constructor_and_live_mapper_keep_scene_finals_and_prefixes() {
        let (clip, scenes) = crate::test_helpers::quality_scene_fixture();
        for disabled in [None, Some(false), Some(true)] {
            let metric = QualityMetric::CVVDP {
                target_range: (3.9, 4.1),
                resolution: None,
                display_model: None,
                resize_to_display: None,
                disable_temporal: disabled,
                gpu_id: None,
            };
            for (statistic, independent) in [
                (ProbeStatistic::Mean, [5.0, 5.0]),
                (ProbeStatistic::Minimum, [2.0, 1.0]),
                (ProbeStatistic::Maximum, [9.0, 8.0]),
            ] {
                let app = QualityCheckApp::new(clip, &scenes, metric.clone(), statistic);
                let expected = if disabled == Some(true) {
                    independent
                } else {
                    [4.0, 6.0]
                };
                assert_eq!(
                    app.cached_state.scene_scores,
                    BTreeMap::from([(0, expected[0]), (1, expected[1])])
                );
                assert_eq!(
                    app.cached_state.frame_scores,
                    [2.0, 9.0, 4.0, 1.0, 8.0, 6.0]
                );
                assert_eq!(
                    (
                        app.cached_state.frames_compared,
                        app.cached_state.total_frames
                    ),
                    (6, 6)
                );
                let mut live =
                    QualityCheckApp::new(clip, &[], metric.clone(), statistic).cached_state;
                for (index, score) in expected.into_iter().enumerate() {
                    assert!(QualityCheckApp::map_progress(
                        SequenceStatus::Subprocess {
                            parent: Status::Processing {
                                id: "Quality Check".to_owned(),
                                completion: SequenceCompletion::Custom {
                                    name: "Quality Check".to_owned(),
                                    completed: 6.0,
                                    total: 6.0
                                }
                            },
                            child: Status::Processing {
                                id: "Quality".to_owned(),
                                completion: SequenceCompletion::SceneQuality {
                                    index: index as u64,
                                    quantizer: 0.0,
                                    score,
                                    bitrate: 0.0
                                }
                            }
                        },
                        &mut live
                    ));
                }
                assert_eq!(live.scene_scores, app.cached_state.scene_scores);
                let control =
                    QualityCheckApp::new(clip, &scenes, QualityMetric::default(), statistic);
                assert_eq!(
                    control.cached_state.scene_scores.values().copied().collect::<Vec<_>>(),
                    independent
                );
            }
        }
        assert_eq!(
            scenes[0].sequence_data.quality_check.quality.scores,
            [2.0, 9.0, 4.0]
        );
    }

    #[test]
    fn m27_report_does_not_summarize_prefixes_as_independent_frames() {
        if let Ok(mode) = std::env::var("M27_QC_REPORT_CHILD") {
            let (clip, scenes) = crate::test_helpers::quality_scene_fixture();
            let metric = QualityMetric::CVVDP {
                target_range: (3.9, 4.1),
                resolution: None,
                display_model: None,
                resize_to_display: None,
                disable_temporal: Some(mode == "independent"),
                gpu_id: None,
            };
            QualityCheckApp::new(clip, &scenes, metric, ProbeStatistic::Mean).print_report();
            return;
        }
        for mode in ["temporal", "independent"] {
            let output = Command::new(std::env::current_exe().expect("test executable"))
                .args(["--exact", "apps::quality_check::tests::m27_report_does_not_summarize_prefixes_as_independent_frames", "--nocapture"])
                .env("M27_QC_REPORT_CHILD", mode).output().expect("report child");
            let stdout = String::from_utf8_lossy(&output.stdout);
            assert!(output.status.success(), "{stdout}");
            assert!(
                stdout.contains("Per-Scene") && stdout.contains("5.0000"),
                "{stdout}"
            );
            assert_eq!(
                stdout.contains("Per-Frame"),
                mode == "independent",
                "{stdout}"
            );
        }
    }

    #[test]
    fn m18_submission_progress_reaches_ui_and_console() {
        if std::env::var_os("M18_QC_UI_CHILD").is_some() {
            let mut state = QualityCheckState {
                scene_scores: BTreeMap::new(),
                frame_scores: Vec::new(),
                scene_details: BTreeMap::new(),
                frames_compared: 0,
                total_frames: 0,
            };
            // Replay initialization, then the exact Whole/Custom submission shape.
            for (completed, total) in [(0, 1), (1, 3), (2, 3), (3, 3)] {
                let redraw = QualityCheckApp::map_progress(
                    SequenceStatus::Whole(Status::Processing {
                        id: "Quality Check".to_owned(),
                        completion: SequenceCompletion::Custom {
                            name: "Quality Check".to_owned(),
                            completed: completed as f64,
                            total: total as f64,
                        },
                    }),
                    &mut state,
                );
                assert!(
                    redraw,
                    "submission {completed}/{total} must reach the real UI mapper"
                );
                assert_eq!(
                    (state.frames_compared, state.total_frames),
                    (completed, total)
                );
                assert!(
                    state.frame_scores.is_empty() && state.scene_scores.is_empty(),
                    "submission progress must not fabricate scores"
                );
            }
            for (frame, score) in [(4, 90.0), (7, 91.0), (9, 92.0)] {
                assert!(
                    QualityCheckApp::map_progress(
                        SequenceStatus::Subprocess {
                            parent: Status::Processing {
                                id: "Quality Check".to_owned(),
                                completion: SequenceCompletion::Custom {
                                    name: "Quality Check".to_owned(),
                                    completed: 3.0,
                                    total: 3.0,
                                },
                            },
                            child: Status::Processing {
                                id: "Quality".to_owned(),
                                completion: SequenceCompletion::FrameScore { frame, score },
                            },
                        },
                        &mut state,
                    ),
                    "final real scores still redraw"
                );
                assert_eq!(
                    (state.frames_compared, state.total_frames),
                    (3, 3),
                    "final scores must not restart or double-count submission progress"
                );
            }
            assert_eq!(state.frame_scores, [90.0, 91.0, 92.0]);
            assert!(state.scene_scores.is_empty());
            return;
        }

        // A piped child exercises actual console output without terminal setup or
        // process-global stdout redirection in the parallel test runner.
        let mut child = Command::new(std::env::current_exe().expect("test executable"))
            .args([
                "--exact",
                "apps::quality_check::tests::m18_submission_progress_reaches_ui_and_console",
                "--nocapture",
            ])
            .env("M18_QC_UI_CHILD", "1")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("mapper child");
        let start = Instant::now();
        while child.try_wait().expect("child status").is_none() {
            if start.elapsed() > Duration::from_secs(30) {
                child.kill().expect("kill timed out child");
                child.wait().expect("reap timed out child");
                panic!("mapper child exceeded 30s");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let output = child.wait_with_output().expect("mapper output");
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        eprintln!("M18 actual QC mapper/console:\n{stdout}\n{stderr}");
        assert!(output.status.success(), "{stdout}\n{stderr}");
        assert!(
            stdout.contains("running 1 test") && stdout.contains("1 passed; 0 failed"),
            "exact child filter must execute one test"
        );
        let events: Vec<serde_json::Value> = stdout
            .lines()
            .filter_map(|line| {
                line.strip_prefix("[Quality Check][Compare] ")
                    .or_else(|| line.strip_prefix("[Quality Check][Frame] "))
                    .map(|json| serde_json::from_str(json).expect("actual console JSON"))
            })
            .collect();
        assert_eq!(
            events,
            [
                serde_json::json!({"CompareProgress":{"current_frame":0,"total_frames":1}}),
                serde_json::json!({"CompareProgress":{"current_frame":1,"total_frames":3}}),
                serde_json::json!({"CompareProgress":{"current_frame":2,"total_frames":3}}),
                serde_json::json!({"CompareProgress":{"current_frame":3,"total_frames":3}}),
                serde_json::json!({"FrameScore":{"frame":4,"score":90.0}}),
                serde_json::json!({"FrameScore":{"frame":7,"score":91.0}}),
                serde_json::json!({"FrameScore":{"frame":9,"score":92.0}}),
            ],
            "accepted progress precedes real source-indexed final scores without extra progress events"
        );
    }
}
