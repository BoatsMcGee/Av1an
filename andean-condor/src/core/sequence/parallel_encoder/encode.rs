use std::{
    collections::{BTreeMap, VecDeque},
    fs,
    io::Cursor,
    sync::{
        Arc,
        Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread,
};

use anyhow::Result;
use crossbeam_channel::Receiver;
use tracing::{debug, trace};

use super::{
    ParallelEncoder, STREAM_WINDOW,
    error::ParallelEncoderError,
    task::{ParallelEncoderResult, ResultStream, Task},
};
use crate::{
    core::{
        encoder::EncodeProgress,
        input::Input,
        sequence::{SequenceCompletion, SequenceStatus, Status, ffmpeg_filter::FfmpegFilter},
    },
    utils::semaphore::Semaphore,
};

use super::progress::{drain_finished_results, first_filter_failure, relay_progress};

/// The frame channel an encoder reads, plus the filter stage feeding it.
pub(super) type FilteredFrames = (Receiver<Cursor<Vec<u8>>>, Option<FfmpegFilter>);

/// Inserts the scene's FFmpeg filter between the frame producer and the
/// encoder. The encoder consumes the Y4M header FFmpeg regenerates.
///
/// `filter_failures` is `None` for callers that report failures through
/// [`FfmpegFilter::check`] instead of a shared cell.
pub(super) fn spawn_filter(
    frames_rx: Receiver<Cursor<Vec<u8>>>,
    task: &Task,
    filter_failures: Option<&BTreeMap<usize, Arc<Mutex<Option<String>>>>>,
    window: usize,
) -> Result<FilteredFrames> {
    match task.encoder.ffmpeg_filter() {
        Some(graph) => {
            let failure = filter_failures
                .and_then(|cells| cells.get(&task.original_index))
                .cloned()
                .unwrap_or_else(|| Arc::new(Mutex::new(None)));
            let stage = FfmpegFilter::spawn(frames_rx, window, graph, failure)?;
            Ok((stage.receiver(), Some(stage)))
        },
        None => Ok((frames_rx, None)),
    }
}

/// A finished scene encode, and whether it is usable.
struct SceneOutcome {
    temp_output: std::path::PathBuf,
    bytes:       u64,
    usable:      bool,
}

/// Renames a usable encode to its final path, or discards it and flags the
/// error. A failed filter truncates the stream, so a short encode must not
/// survive as the scene's output.
fn finalize_scene(
    outcome: SceneOutcome,
    output: &std::path::Path,
    scene: usize,
    size_tx: &std::sync::mpsc::Sender<SequenceStatus>,
    encoder_errored: &AtomicBool,
) -> Result<()> {
    if outcome.usable {
        size_tx.send(SequenceStatus::Whole(Status::Processing {
            id:         scene.to_string(),
            completion: SequenceCompletion::Custom {
                name:      "size".to_owned(),
                completed: outcome.bytes as f64,
                total:     outcome.bytes as f64,
            },
        }))?;
        fs::rename(outcome.temp_output, output)?;
    } else {
        let _ = fs::remove_file(&outcome.temp_output);
        encoder_errored.store(true, Ordering::Relaxed);
    }
    Ok(())
}

/// Sends a scene's result to the persistence channel and returns it.
fn report_result(
    results_tx: &crossbeam_channel::Sender<ParallelEncoderResult>,
    finished_scenes: &Semaphore,
    result: ParallelEncoderResult,
) -> Result<ParallelEncoderResult> {
    results_tx.send(result.clone())?;
    finished_scenes.release();
    Ok(result)
}

impl ParallelEncoder {
    #[inline]
    pub fn encode_tasks(
        input: &mut Input,
        workers: u8,
        tasks: VecDeque<Task>,
        progress_tx: std::sync::mpsc::Sender<SequenceStatus>,
        cancelled: Arc<AtomicBool>,
    ) -> Result<Vec<Option<ParallelEncoderResult>>> {
        let (results_tx, results_rx) = crossbeam_channel::unbounded();
        let mut on_results = |_results: Vec<ParallelEncoderResult>| -> Result<()> { Ok(()) };
        let mut stream = ResultStream {
            results_tx,
            results_rx: &results_rx,
            finished_scenes: Arc::new(Semaphore::new(0)),
            on_results: &mut on_results,
        };
        let results = Self::encode_tasks_with_sender(
            input,
            workers,
            tasks,
            progress_tx,
            cancelled,
            &mut stream,
        )?;
        Ok(results)
    }

    #[inline]
    pub(super) fn encode_tasks_with_sender(
        input: &mut Input,
        workers: u8,
        tasks: VecDeque<Task>,
        progress_tx: std::sync::mpsc::Sender<SequenceStatus>,
        cancelled: Arc<AtomicBool>,
        stream: &mut ResultStream<'_, impl FnMut(Vec<ParallelEncoderResult>) -> Result<()>>,
    ) -> Result<Vec<Option<ParallelEncoderResult>>> {
        // Streaming inputs take the bounded-memory path; the rest decode one
        // scene ahead of the workers.
        if input.streams_concurrently() {
            return Self::encode_tasks_streaming(
                input,
                workers,
                STREAM_WINDOW,
                tasks,
                &progress_tx,
                &cancelled,
                stream,
            );
        }
        let (task_tx, task_rx) = crossbeam_channel::unbounded();
        let mut frames_senders = BTreeMap::new();
        let mut frames_receivers = BTreeMap::new();
        let mut encoder_semaphores = BTreeMap::new();
        // One failure cell per scene, filled by that scene's filter subprocess.
        let filter_failures: BTreeMap<usize, Arc<Mutex<Option<String>>>> = tasks
            .iter()
            .filter(|task| task.encoder.ffmpeg_filter().is_some())
            .map(|task| (task.original_index, Arc::new(Mutex::new(None))))
            .collect();
        let total_tasks = tasks.len();
        let total_process_frames = tasks.iter().fold(0, |acc, task| acc + task.frame_indices.len());
        for task in tasks.iter() {
            let index = task.index;
            task_tx.send(task.clone())?;
            let (ftx, rtx) = crossbeam_channel::unbounded();
            frames_senders.insert(index, ftx);
            frames_receivers.insert(index, rtx);
            encoder_semaphores.insert(index, Arc::new(Semaphore::new(0)));
        }
        drop(task_tx);
        let encoder_errored = Arc::new(AtomicBool::new(false));
        let clip_info = input.clip_info()?;

        thread::scope(|s| -> Result<_> {
            let total_final_pass_frames_encoded = Arc::new(AtomicUsize::new(0));
            let worker_semaphore = Arc::new(Semaphore::new(workers.into()));
            let decoder_semaphore = Arc::new(Semaphore::new(usize::from(workers) + 1));
            let frame_receivers = Arc::new(Mutex::new(frames_receivers));
            let mut encoder_threads = Vec::new();
            let cancelled = Arc::new(cancelled);

            for _ in 0..total_tasks {
                let total_final_pass_frames_encoded = Arc::clone(&total_final_pass_frames_encoded);
                let cancelled = Arc::clone(&cancelled);
                let filter_failures = &filter_failures;
                let task_rx = task_rx.clone();
                let frame_receivers = Arc::clone(&frame_receivers);
                let task_progress_tx = progress_tx.clone();
                let size_tx = progress_tx.clone();
                let task = task_rx.recv()?;
                let total_passes = task.encoder.total_passes();
                let total_scene_frames = task.frame_indices.len();
                let worker_semaphore = Arc::clone(&worker_semaphore);
                let decoder_semaphore_clone = Arc::clone(&decoder_semaphore);
                let encoder_semaphore =
                    encoder_semaphores.get(&task.index).expect("encoder_semaphore exists");
                let encoder_semaphore_clone: Arc<Semaphore> = Arc::clone(encoder_semaphore);
                let encoder_errored = Arc::clone(&encoder_errored);
                let finished_scenes = Arc::clone(&stream.finished_scenes);
                let results_tx = stream.results_tx.clone();

                let encoder_thread = s.spawn(move || -> Result<Option<ParallelEncoderResult>> {
                    let mut fr_lock =
                        frame_receivers.lock().expect("frame_receivers mutex should acquire lock");
                    let frames_rx = fr_lock.remove(&task.index).expect("should have frames_rx)");
                    drop(fr_lock);
                    let (encode_progress_tx, encode_progress_rx) =
                        std::sync::mpsc::channel::<EncodeProgress>();

                    // Held until the decoder starts, so encoders do not all
                    // launch at creation.
                    encoder_semaphore_clone.acquire();
                    trace!(
                        "Scene {} Encoder waiting for a free Worker",
                        task.original_index
                    );
                    // Caps active encoders at the worker limit.
                    let worker_id = worker_semaphore.acquire();
                    if cancelled.load(Ordering::Relaxed) || encoder_errored.load(Ordering::Relaxed)
                    {
                        // Let the decoder exit.
                        worker_semaphore.release();
                        decoder_semaphore_clone.release();
                        return Ok(None);
                    }
                    debug!(
                        "Encoding Scene {} with Worker {}",
                        task.original_index, worker_id
                    );
                    let started = std::time::SystemTime::now();
                    s.spawn(move || -> Result<()> {
                        relay_progress(
                            encode_progress_rx,
                            &task_progress_tx,
                            total_passes,
                            task.original_index,
                            total_scene_frames,
                            total_process_frames,
                            &total_final_pass_frames_encoded,
                        )
                    });
                    let temp_output = task.output.with_extension(format!(
                        "temp.{}",
                        task.encoder.output_extension()
                    ));
                    trace!(
                        "Encoding Scene {} to {}",
                        task.original_index,
                        temp_output.display()
                    );
                    let (frames_rx, filter_stage) =
                        spawn_filter(frames_rx, &task, Some(filter_failures), STREAM_WINDOW)?;
                    let result = task.encoder.encode_with_stream(
                        frames_rx,
                        &temp_output,
                        encode_progress_tx,
                    )?;
                    let ended = std::time::SystemTime::now();
                    let framerate =
                        *clip_info.frame_rate.numer() as f64 / *clip_info.frame_rate.denom() as f64;
                    let scene_seconds = task.frame_indices.len() as f64 * framerate;
                    let bytes = temp_output.metadata().ok().map_or(0, |meta| meta.len());
                    let bitrate = (bytes * 8) as f64 / scene_seconds;
                    let filter_failed = filter_stage
                        .as_ref()
                        .is_some_and(|stage| stage.is_failed().is_some());
                    finalize_scene(
                        SceneOutcome {
                            temp_output,
                            bytes,
                            usable: result.status.success() && !filter_failed,
                        },
                        &task.output,
                        task.original_index,
                        &size_tx,
                        &encoder_errored,
                    )?;
                    debug!(
                        "Encoded Scene {} in {} seconds yielding {} bytes",
                        task.original_index,
                        ended.duration_since(started)?.as_secs(),
                        bytes
                    );
                    worker_semaphore.release(); // Release for next worker
                    decoder_semaphore_clone.release(); // Release for next decoder
                    let parallel_result = report_result(
                        &results_tx,
                        &finished_scenes,
                        ParallelEncoderResult {
                            scene: task.original_index,
                            started,
                            ended,
                            bytes,
                            bitrate,
                            result,
                        },
                    )?;
                    Ok(Some(parallel_result))
                });

                encoder_threads.push(encoder_thread);
            }

            let mut decode_error: Option<anyhow::Error> = None;
            for task in tasks {
                // Bounds how far ahead the decoder runs.
                decoder_semaphore.acquire();
                if !cancelled.load(Ordering::Relaxed) && !encoder_errored.load(Ordering::Relaxed) {
                    debug!("Decoding Scene {}", task.original_index);
                    let frames_tx =
                        frames_senders.remove(&task.index).expect("should have frames_tx");
                    let y4m_header = input.y4m_header(Some(task.frame_indices.len()))?;
                    let decode_result = (|| -> Result<()> {
                        frames_tx.send(Cursor::new(Vec::from(y4m_header.as_bytes())))?;
                        input.y4m_frames(frames_tx, &task.frame_indices)?;
                        Ok(())
                    })();
                    if let Err(err) = decode_result {
                        // Remaining encoders exit on the flag rather than wait
                        // for frames that will never arrive.
                        encoder_errored.store(true, Ordering::Relaxed);
                        decode_error = Some(err);
                    }
                }
                encoder_semaphores
                    .get(&task.index)
                    .expect("should have encoder_semaphore")
                    .release();

                drain_finished_results(stream)?;
            }

            drain_finished_results(stream)?;

            let encoder_results = encoder_threads
                .into_iter()
                .map(|result| {
                    result
                        .join()
                        .expect("should join encoder thread")
                        .expect("should join encoder_thread")
                })
                .collect::<Vec<_>>();
            drop(progress_tx);

            // The filter's stderr explains the failure better than the encoder
            // or decoder fallout it causes.
            if let Some((scene, message)) = first_filter_failure(&filter_failures) {
                anyhow::bail!("FFmpeg filter failed while encoding scene {scene}: {message}");
            }

            let first_error = encoder_results.iter().find(|maybe_result| {
                maybe_result.as_ref().is_some_and(|result| !result.result.status.success())
            });
            if let Some(Some(first_error)) = first_error {
                let err = ParallelEncoderError::EncoderFailed {
                    scene:  first_error.scene,
                    result: first_error.result.clone(),
                };
                tracing::error!("{}", err);
                anyhow::bail!(err);
            }
            if let Some(decode_error) = decode_error {
                anyhow::bail!(decode_error);
            }

            Ok(encoder_results)
        })
    }
}
