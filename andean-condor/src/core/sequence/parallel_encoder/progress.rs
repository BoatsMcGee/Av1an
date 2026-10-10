use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

use anyhow::Result;

use super::task::{ParallelEncoderResult, ResultStream};
use crate::core::{
    encoder::EncodeProgress,
    sequence::{SequenceCompletion, SequenceStatus, Status, parallel_encoder::DETAILS},
};

/// Record one finished final-pass frame and return the running total.
///
/// `fetch_add` returns the pre-increment value, so the count returned here
/// includes the frame just recorded.
#[inline]
pub(super) fn count_frame(count: &AtomicUsize) -> usize {
    count.fetch_add(1, Ordering::Relaxed) + 1
}

/// The first recorded FFmpeg filter failure, keyed by scene, if any.
pub(super) fn first_filter_failure(
    failures: &BTreeMap<usize, Arc<Mutex<Option<String>>>>,
) -> Option<(usize, String)> {
    failures.iter().find_map(|(&scene, cell)| {
        cell.lock()
            .expect("ffmpeg filter failure mutex should acquire lock")
            .clone()
            .map(|message| (scene, message))
    })
}

/// Drains any finished scene results from the channel and forwards them to
/// `on_results`.
pub(super) fn drain_finished_results<F>(stream: &mut ResultStream<'_, F>) -> Result<()>
where
    F: FnMut(Vec<ParallelEncoderResult>) -> Result<()>,
{
    let mut results = Vec::new();
    while stream.finished_scenes.try_acquire().is_some() {
        if let Ok(result) = stream.results_rx.try_recv() {
            results.push(result);
        }
    }
    if !results.is_empty() {
        (stream.on_results)(results)?;
    }
    Ok(())
}

/// Forwards one encoder's progress to the sequence's progress channel.
///
/// The parent status carries the whole encode's final-pass total, so every
/// scene's relay contributes to one shared bar.
#[allow(clippy::too_many_arguments)]
pub(super) fn relay_progress(
    encode_progress_rx: std::sync::mpsc::Receiver<EncodeProgress>,
    task_progress_tx: &std::sync::mpsc::Sender<SequenceStatus>,
    total_passes: u8,
    scene: usize,
    total_scene_frames: usize,
    total_process_frames: usize,
    total_final_pass_frames_encoded: &AtomicUsize,
) -> Result<()> {
    for progress in encode_progress_rx {
        if progress.pass.0 == total_passes && progress.frame > 0 {
            let total_final_encoded = count_frame(total_final_pass_frames_encoded);
            task_progress_tx.send(SequenceStatus::Whole(Status::Processing {
                id:         DETAILS.name.to_owned(),
                completion: SequenceCompletion::Frames {
                    completed: total_final_encoded as u64,
                    total:     total_process_frames as u64,
                },
            }))?;
        }
        task_progress_tx.send(SequenceStatus::Subprocess {
            parent: Status::Processing {
                id:         DETAILS.name.to_owned(),
                completion: SequenceCompletion::Frames {
                    completed: total_final_pass_frames_encoded.load(Ordering::Relaxed) as u64,
                    total:     total_process_frames as u64,
                },
            },
            child:  Status::Processing {
                id:         scene.to_string(),
                completion: SequenceCompletion::PassFrames {
                    passes: progress.pass,
                    frames: (progress.frame as u64, total_scene_frames as u64),
                },
            },
        })?;

        if progress.pass.0 != total_passes {
            continue;
        }
        // The scene's final pass reached its last frame.
        if progress.frame == total_scene_frames {
            task_progress_tx.send(SequenceStatus::Subprocess {
                parent: Status::Processing {
                    id:         DETAILS.name.to_owned(),
                    completion: SequenceCompletion::Frames {
                        completed: total_final_pass_frames_encoded.load(Ordering::Relaxed) as u64,
                        total:     total_process_frames as u64,
                    },
                },
                child:  Status::Completed {
                    id: scene.to_string(),
                },
            })?;
        }
        // Every scene's final pass is done.
        if total_final_pass_frames_encoded.load(Ordering::Relaxed) == total_process_frames {
            task_progress_tx.send(SequenceStatus::Whole(Status::Completed {
                id: DETAILS.name.to_owned(),
            }))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::{
        atomic::AtomicUsize,
        mpsc::{Receiver, Sender, channel},
    };

    use super::{count_frame, relay_progress};
    use crate::core::{encoder::EncodeProgress, sequence::SequenceStatus};

    /// Drives `relay_progress` with one scene's final pass and returns every
    /// status it emitted.
    fn relay_one_scene(frames: usize) -> Vec<SequenceStatus> {
        let (tx, rx): (Sender<EncodeProgress>, Receiver<EncodeProgress>) = channel();
        let (status_tx, status_rx) = std::sync::mpsc::channel();
        let total_final = AtomicUsize::new(0);

        let handle = std::thread::spawn(move || {
            relay_progress(
                rx,
                &status_tx,
                1,
                7,
                frames,
                frames,
                &total_final,
            )
        });

        for frame in 1..=frames {
            tx.send(EncodeProgress {
                pass:  (1, 1),
                frame,
                usage: Default::default(),
            })
            .expect("send progress");
        }
        drop(tx);
        handle.join().expect("relay should join").expect("relay should succeed");

        status_rx.iter().collect()
    }

    /// The TUI clears a scene from its active list and advances the pass on
    /// these events, so dropping them stalls the UI until the sequence ends.
    #[test]
    fn a_finished_scene_and_a_finished_encode_report_completion() {
        let frames = 3_usize;
        let statuses = relay_one_scene(frames);

        let scene_completed = statuses.iter().any(|status| {
            matches!(
                status,
                SequenceStatus::Subprocess {
                    child: crate::core::sequence::Status::Completed { id },
                    ..
                } if id == "7"
            )
        });
        assert!(
            scene_completed,
            "the scene's final frame must report the scene completed"
        );

        let encode_completed = statuses.iter().any(|status| {
            matches!(
                status,
                SequenceStatus::Whole(crate::core::sequence::Status::Completed { .. })
            )
        });
        assert!(
            encode_completed,
            "the last scene's final frame must report the encode completed"
        );
    }

    /// The count must include the frame just recorded, or the bar stops one
    /// short of its total.
    #[test]
    fn the_frame_count_includes_the_frame_just_recorded() {
        let count = AtomicUsize::new(0);
        for expected in 1..=44_usize {
            assert_eq!(
                count_frame(&count),
                expected,
                "frame {expected} should count itself"
            );
        }
        assert_eq!(count.load(std::sync::atomic::Ordering::Relaxed), 44);
    }

    /// Concurrent workers must not lose a count.
    #[test]
    fn concurrent_counts_do_not_lose_frames() {
        let workers = 8_usize;
        let per_worker = 500_usize;
        let total = workers * per_worker;
        let count = AtomicUsize::new(0);

        let counts: Vec<Vec<usize>> = std::thread::scope(|scope| {
            let mut handles = Vec::with_capacity(workers);
            for _ in 0..workers {
                handles.push(scope.spawn(|| {
                    let mut seen = Vec::with_capacity(per_worker);
                    for _ in 0..per_worker {
                        seen.push(count_frame(&count));
                    }
                    seen
                }));
            }
            handles
                .into_iter()
                .map(|handle| handle.join().expect("worker should not panic"))
                .collect()
        });

        assert_eq!(count.load(std::sync::atomic::Ordering::Relaxed), total);

        // Every value from 1 to the total is handed out exactly once.
        let mut all: Vec<usize> = counts.into_iter().flatten().collect();
        all.sort_unstable();
        assert_eq!(all, (1..=total).collect::<Vec<_>>());
    }
}
