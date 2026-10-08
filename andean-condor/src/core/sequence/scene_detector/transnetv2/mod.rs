//! TransNetV2 shot boundary detection via ONNX Runtime.
//!
//! Mirrors the reference implementation (soCzech/TransNetV2): frames are
//! reduced to 48×27 RGB tiles, evaluated in overlapping 100-frame windows with
//! a stride of 50 (each window contributes its middle 50 predictions), and
//! runs of above-threshold predictions become scene cuts.

mod model;
mod preprocess;

use std::{
    collections::VecDeque,
    path::Path,
    sync::{
        self,
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use anyhow::{Result, bail};
use ort::session::Session;

use super::DETAILS;
use crate::core::{
    input::{FrameFeed, Input},
    sequence::{SequenceCompletion, SequenceStatus, Status},
};

/// Frames per inference window; fixed by the exported model's input shape.
const WINDOW: usize = 100;
/// Frames advanced between windows; the 50-frame overlap is discarded.
const STRIDE: usize = 50;
/// Window edge frames borrowed from the start/end of the video.
const START_PAD: usize = 25;
/// Probability above which a frame counts as part of a transition.
const THRESHOLD: f32 = 0.5;

/// Runs the model over `[last_frame, frames)` and returns interior scene
/// starts relative to `last_frame`, honoring minimum and maximum lengths,
/// alongside the per-frame transition probabilities (relative to `last_frame`,
/// truncated to the analyzed prefix when cancelled).
#[allow(clippy::too_many_arguments)]
pub(crate) fn detect_cuts(
    input: &mut Input,
    frames: usize,
    last_frame: usize,
    minimum_length: usize,
    maximum_length: usize,
    model_path: Option<&Path>,
    progress_tx: &sync::mpsc::Sender<SequenceStatus>,
    cancelled: &Arc<AtomicBool>,
) -> Result<(Vec<usize>, Vec<f32>)> {
    let total = frames.saturating_sub(last_frame);
    if total == 0 {
        return Ok((Vec::new(), Vec::new()));
    }

    let mut session = model::session(model_path)?;
    let mut feed = input.frame_feed(last_frame, frames)?;
    let predictions = predict(
        &mut session,
        &mut feed,
        total,
        last_frame,
        frames,
        progress_tx,
        cancelled,
    )?;

    let cuts = enforce_lengths(
        cuts_from_predictions(&predictions),
        predictions.len(),
        minimum_length,
        maximum_length,
    );

    // Interior scenes are finalized here; the caller reports the tail scene.
    let mut previous = 0;
    for &cut in &cuts {
        progress_tx.send(SequenceStatus::Whole(Status::Processing {
            id:         DETAILS.name.to_owned(),
            completion: SequenceCompletion::Custom {
                name:      "new-scene".to_owned(),
                completed: (last_frame + previous) as f64,
                total:     (last_frame + cut) as f64,
            },
        }))?;
        previous = cut;
    }

    Ok((cuts, predictions))
}

/// Streams `feed` through `session`, one window at a time. Windows read their
/// frames sequentially once and reuse the 50-frame overlap from memory.
fn predict(
    session: &mut Session,
    feed: &mut FrameFeed<'_>,
    total: usize,
    last_frame: usize,
    frames: usize,
    progress_tx: &sync::mpsc::Sender<SequenceStatus>,
    cancelled: &Arc<AtomicBool>,
) -> Result<Vec<f32>> {
    let mut predictions = vec![0.0f32; total];
    // Frames whose predictions were actually computed; a cancelled run keeps
    // only this prefix.
    let mut analyzed = 0usize;
    // Most recently read stream tiles, kept for the next window's overlap.
    let mut recent: VecDeque<Vec<f32>> = VecDeque::with_capacity(STRIDE + START_PAD);
    let first = take_tile(feed)?;
    recent.push_back(first.clone());
    let mut latest = first.clone();
    let mut cursor = 1;

    for window in 0..total.div_ceil(STRIDE) {
        if cancelled.load(Ordering::Relaxed) {
            break;
        }

        let base = window * STRIDE;
        let mut tiles = Vec::with_capacity(WINDOW);
        for position in base..base + WINDOW {
            if position < START_PAD {
                tiles.push(first.clone());
                continue;
            }
            let index = position - START_PAD;
            if index >= total {
                tiles.push(latest.clone());
                continue;
            }
            while cursor <= index {
                let tile = take_tile(feed)?;
                latest = tile.clone();
                recent.push_back(tile);
                if recent.len() > STRIDE + START_PAD {
                    recent.pop_front();
                }
                cursor += 1;
            }
            let offset = cursor - recent.len();
            tiles.push(recent[index - offset].clone());
        }

        let mut flat = Vec::with_capacity(WINDOW * preprocess::TILE_LEN);
        for tile in &tiles {
            flat.extend_from_slice(tile);
        }
        let tensor = ort::value::Tensor::from_array((
            [1usize, WINDOW, preprocess::HEIGHT, preprocess::WIDTH, 3],
            flat,
        ))
        .map_err(model::onnx)?;
        let outputs = session.run(ort::inputs! { "input" => tensor }).map_err(model::onnx)?;
        let (_, scores) = outputs[0].try_extract_tensor::<f32>().map_err(model::onnx)?;
        if scores.len() < WINDOW {
            bail!(
                "TransNetV2 returned {} scores for a {WINDOW}-frame window",
                scores.len()
            );
        }

        let start = window * STRIDE;
        let produced = STRIDE.min(total - start);
        // Only the middle STRIDE frames are free of window-edge padding.
        predictions[start..start + produced]
            .copy_from_slice(&scores[START_PAD..START_PAD + produced]);
        analyzed = start + produced;

        progress_tx.send(SequenceStatus::Whole(Status::Processing {
            id:         DETAILS.name.to_owned(),
            completion: SequenceCompletion::Frames {
                completed: (last_frame + start + produced) as u64,
                total:     frames as u64,
            },
        }))?;
    }

    // The zero-filled tail of a cancelled run would otherwise force scene
    // splits across frames that were never analyzed.
    predictions.truncate(analyzed);
    Ok(predictions)
}

/// The feed's next tile, or an error if the input ended before every frame
/// named by `frames` was read.
fn take_tile(feed: &mut FrameFeed<'_>) -> Result<Vec<f32>> {
    let frame = feed
        .next_frame()?
        .ok_or_else(|| anyhow::anyhow!("input ended before every frame was analyzed"))?;
    Ok(preprocess::raw_frame_to_tile(&frame))
}

/// One cut at the end of each above-threshold run, matching the reference
/// `predictions_to_scenes` convention where transition frames close a scene.
fn cuts_from_predictions(predictions: &[f32]) -> Vec<usize> {
    let mut cuts = Vec::new();
    let mut in_transition = false;
    for (index, probability) in predictions.iter().enumerate() {
        if *probability > THRESHOLD {
            in_transition = true;
        } else if in_transition {
            cuts.push(index);
            in_transition = false;
        }
    }
    cuts
}

/// Drops cuts closer than `minimum_length` to the previous scene start, then
/// forces a split every `maximum_length` frames. Forced splits win over the
/// minimum, so every output cut is `< total`.
fn enforce_lengths(
    cuts: Vec<usize>,
    total: usize,
    minimum_length: usize,
    maximum_length: usize,
) -> Vec<usize> {
    let maximum_length = maximum_length.max(1);
    let mut filtered = Vec::with_capacity(cuts.len());
    let mut previous = 0;
    for cut in cuts {
        if cut >= total {
            break;
        }
        if cut - previous >= minimum_length {
            filtered.push(cut);
            previous = cut;
        }
    }

    let mut enforced = Vec::with_capacity(filtered.len());
    let mut previous = 0;
    let mut next = 0;
    while previous < total {
        match filtered.get(next) {
            Some(&cut) if cut <= previous + maximum_length => {
                next += 1;
                if cut - previous >= minimum_length {
                    enforced.push(cut);
                    previous = cut;
                }
            },
            _ => {
                let forced = previous + maximum_length;
                if forced >= total {
                    break;
                }
                enforced.push(forced);
                previous = forced;
            },
        }
    }
    enforced
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn predictions_above_threshold_become_cuts() {
        let predictions = [0.1, 0.9, 0.95, 0.2, 0.6, 0.1];
        assert_eq!(cuts_from_predictions(&predictions), vec![3, 5]);
    }

    #[test]
    fn leading_transition_cuts_after_it() {
        let predictions = [0.9, 0.1, 0.1];
        assert_eq!(cuts_from_predictions(&predictions), vec![1]);
    }

    #[test]
    fn trailing_transition_produces_no_cut() {
        let predictions = [0.1, 0.1, 0.9];
        assert!(cuts_from_predictions(&predictions).is_empty());
    }

    #[test]
    fn threshold_is_exclusive() {
        let predictions = [0.4, 0.5, 0.4];
        assert!(cuts_from_predictions(&predictions).is_empty());
    }

    #[test]
    fn cuts_below_minimum_are_dropped() {
        assert_eq!(enforce_lengths(vec![10, 60], 100, 24, 50), vec![50]);
    }

    #[test]
    fn gaps_beyond_maximum_get_forced_splits() {
        assert_eq!(enforce_lengths(vec![90], 100, 1, 40), vec![40, 80, 90]);
    }

    #[test]
    fn no_cuts_still_covers_the_video_in_chunks() {
        assert_eq!(enforce_lengths(vec![], 100, 1, 50), vec![50]);
    }

    #[test]
    fn forced_splits_stop_at_total() {
        assert!(enforce_lengths(vec![], 90, 1, 50).iter().all(|&cut| cut < 90));
    }
}
