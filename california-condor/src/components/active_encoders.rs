use std::collections::BTreeMap;

use andean_condor::models::encoder::Encoder;
use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Layout, Rect},
    style::Color,
    text::Line,
    widgets::{Block, Widget},
};

use crate::{
    apps::parallel_encoder::SceneEncoder,
    components::{encoder_info::EncoderInfo, progress_bar::ProgressBar},
};

/// Renders the per-worker progress panels inside the parallel encoder TUI.
///
/// Takes a reference to the full `SceneEncoder` map (no clone on render) so
/// that each worker's area can display both the per-scene encoder parameters
/// (top half) and a full progress bar with FPS/elapsed/remaining (bottom half).
pub struct ActiveEncoders<'a> {
    pub color:          Color,
    pub workers:        u8,
    pub parent_encoder: Encoder,
    pub active_scenes:  &'a BTreeMap<u64, SceneEncoder>,
}

impl Widget for ActiveEncoders<'_> {
    fn render(self, area: Rect, buf: &mut Buffer)
    where
        Self: Sized,
    {
        let worker_areas = Layout::default()
            .constraints(std::iter::repeat_n(
                Constraint::Fill(1),
                self.workers.max(1) as usize,
            ))
            .split(area);
        for (worker_area, (scene_index, scene_encoder)) in
            worker_areas.iter().rev().zip(self.active_scenes.iter().rev())
        {
            let bordered_area = Block::bordered()
                .border_type(ratatui::widgets::BorderType::Rounded)
                .title(Line::from(format!(
                    "Scene {} Pass {}/{}",
                    scene_index, scene_encoder.current_pass, scene_encoder.total_passes
                )));
            let worker_area_inner = bordered_area.inner(*worker_area);
            bordered_area.render(*worker_area, buf);

            // Split the bordered area into two halves:
            //   [0] per-scene encoder parameters,
            //   [1] per-scene progress bar (full ProgressBar widget)
            let active_worker_areas = Layout::vertical([Constraint::Fill(1), Constraint::Fill(1)])
                .split(worker_area_inner);

            // Top half: per-scene encoder parameters (only if different from parent)
            let encoder_info = EncoderInfo::new(
                scene_encoder.scene.encoder.clone(),
                Some(self.parent_encoder.clone()),
            );
            encoder_info.generate(false).render(active_worker_areas[0], buf);

            // Bottom half: full progress bar with FPS, elapsed, remaining
            let progress_bar = ProgressBar {
                color:               self.color,
                processing_title:    String::new(),
                completed_title:     String::new(),
                top_right_title:     String::new(),
                bottom_center_title: String::new(),
                initial_completed:   0,
                unit_per_second:     "FPS".to_owned(),
                unit:                "Frame".to_owned(),
                completed:           scene_encoder.frames_processed,
                total:               scene_encoder.total_frames,
                show_label:          true,
            };
            progress_bar
                .generate(Some(scene_encoder.started))
                .render(active_worker_areas[1], buf);
        }
    }
}

impl<'a> ActiveEncoders<'a> {
    #[inline]
    pub fn new(
        color: Color,
        workers: u8,
        parent_encoder: Encoder,
        active_scenes: &'a BTreeMap<u64, SceneEncoder>,
    ) -> Self {
        Self {
            color,
            workers,
            parent_encoder,
            active_scenes,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use andean_condor::{ffmpeg::FFPixelFormat, models::encoder::EncoderBase};

    use super::*;
    use crate::test_helpers::TestVideo;

    /// Forty rows over four workers: one ten-row slot apiece.
    const AREA: Rect = Rect::new(0, 0, 60, 40);
    const SLOT: u16 = AREA.height / 4;

    fn encoder() -> Encoder {
        Encoder::default_from_base(&EncoderBase::SVTAV1, false)
    }

    /// The given scenes mid-encode, keyed the way progress reports them.
    fn active(scenes: &[u64]) -> BTreeMap<u64, SceneEncoder> {
        let video = TestVideo {
            path:         PathBuf::new(),
            width:        1920,
            height:       1080,
            frames:       400,
            fps_rational: (24000, 1001),
            format:       FFPixelFormat::YUV420P,
            scenes:       vec![(0, 100), (100, 200), (200, 300), (300, 400)],
        };
        let scene = video.mock_scenes(&encoder()).remove(0);
        scenes
            .iter()
            .map(|&index| {
                (index, SceneEncoder {
                    scene:            scene.clone(),
                    started:          std::time::Instant::now(),
                    current_pass:     1,
                    total_passes:     2,
                    frames_processed: 40,
                    total_frames:     100,
                })
            })
            .collect()
    }

    fn render(workers: u8, active: &BTreeMap<u64, SceneEncoder>) -> Buffer {
        let mut buf = Buffer::empty(AREA);
        ActiveEncoders::new(Color::Gray, workers, encoder(), active).render(AREA, &mut buf);
        buf
    }

    /// The row a panel's title sits on, or `None` when no panel claims it.
    fn title_row(buf: &Buffer, scene: u64) -> Option<u16> {
        let needle = format!("Scene {scene} Pass");
        (0..AREA.height).find(|&y| {
            let row: String = (0..AREA.width).map(|x| buf[(x, y)].symbol()).collect();
            row.contains(&needle)
        })
    }

    /// Every row holding any part of a panel.
    fn drawn_rows(buf: &Buffer) -> Vec<u16> {
        (0..AREA.height)
            .filter(|&y| (0..AREA.width).any(|x| buf[(x, y)].symbol() != " "))
            .collect()
    }

    /// Fewer active encoders than workers: the panels take the bottom slots
    /// only, the latest worker sits lowest and the earliest directly above it.
    /// Stretching them over the whole area instead would hide how much of the
    /// worker pool is still idle.
    #[test]
    fn panels_fill_the_worker_slots_from_the_bottom_up() {
        let buf = render(4, &active(&[3, 5]));

        assert_eq!(
            title_row(&buf, 3),
            Some(2 * SLOT),
            "the earliest worker sits above the latest"
        );
        assert_eq!(
            title_row(&buf, 5),
            Some(3 * SLOT),
            "the latest worker takes the lowest slot"
        );
        let drawn = drawn_rows(&buf);
        assert!(
            drawn.iter().all(|&y| y >= 2 * SLOT),
            "the slots above the active panels stay blank, got rows {drawn:?}"
        );
    }

    /// Every worker busy: all slots taken, earliest worker at the top.
    #[test]
    fn a_full_pool_of_workers_fills_every_slot() {
        let buf = render(4, &active(&[0, 1, 2, 3]));

        assert_eq!(title_row(&buf, 0), Some(0));
        assert_eq!(title_row(&buf, 3), Some(3 * SLOT));
        assert_eq!(drawn_rows(&buf).len(), AREA.height as usize);
    }

    /// Nothing encoding yet: an empty area, not a stray panel in slot zero.
    #[test]
    fn no_active_encoder_draws_nothing() {
        let buf = render(4, &active(&[]));

        assert!(drawn_rows(&buf).is_empty(), "an idle pool draws no panels");
        assert_eq!(title_row(&buf, 0), None);
    }
}
