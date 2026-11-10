use std::{
    collections::{BTreeMap, VecDeque},
    fs,
    io::Cursor,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread,
};

use anyhow::Result;
use crossbeam_channel::Receiver;
use tracing::debug;

use super::{
    ParallelEncoder, STREAM_WINDOW,
    error::ParallelEncoderError,
    lifecycle::{Stop, TempOutput, retain_error},
    progress::{drain_finished_results, relay_progress},
    task::{ParallelEncoderResult, ResultStream, Task},
};
use crate::{
    core::{
        encoder::{EncodeProgress, EncoderResult},
        input::{Input, clip_info::ClipInfo},
        sequence::{SequenceCompletion, SequenceStatus, Status, ffmpeg_filter::FfmpegFilter},
    },
    utils::semaphore::Semaphore,
};

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
            let mut stage = FfmpegFilter::spawn(frames_rx, window, graph, failure)?;
            Ok((stage.take_receiver()?, Some(stage)))
        },
        None => Ok((frames_rx, None)),
    }
}

/// A finished scene encode, and whether it is usable.
pub(super) struct SceneOutcome {
    pub(super) temp_output: std::path::PathBuf,
    pub(super) bytes: u64,
    pub(super) usable: bool,
}

/// Renames a usable encode to its final path, or discards it and flags the
/// error. A failed filter truncates the stream, so a short encode must not
/// survive as the scene's output.
pub(super) fn finalize_scene(
    outcome: SceneOutcome,
    output: &std::path::Path,
    scene: usize,
    size_tx: &std::sync::mpsc::Sender<SequenceStatus>,
    encoder_errored: &AtomicBool,
) -> Result<()> {
    if outcome.usable {
        #[cfg(all(test, target_os = "linux"))]
        super::e11_tests::fault("size", scene)?;
        size_tx.send(SequenceStatus::Whole(Status::Processing {
            id: scene.to_string(),
            completion: SequenceCompletion::Custom {
                name: "size".to_owned(),
                completed: outcome.bytes as f64,
                total: outcome.bytes as f64,
            },
        }))?;
        fs::rename(outcome.temp_output, output)?;
    } else {
        let _ = fs::remove_file(&outcome.temp_output);
        encoder_errored.store(true, Ordering::Relaxed);
    }
    Ok(())
}

/// The consumer owns filter cleanup and joins the progress relay even when
/// encoding fails. A producer may still be blocked on the filter's input.
#[allow(clippy::too_many_arguments)]
pub(super) fn consume_frames(
    task: &Task,
    frames: Receiver<Cursor<Vec<u8>>>,
    temp: &std::path::Path,
    clip_info: &ClipInfo,
    progress_tx: &std::sync::mpsc::Sender<SequenceStatus>,
    total_frames: usize,
    total_final: &AtomicUsize,
    stop: &Stop,
    window: usize,
) -> Result<EncoderResult> {
    #[cfg(all(test, target_os = "linux"))]
    if let Some(result) = super::e28_tests::consume(task, &frames, temp, stop) {
        return result;
    }
    let (frames, mut filter) = spawn_filter(frames, task, None, window)?;
    thread::scope(|scope| {
        let (tx, rx) = std::sync::mpsc::channel::<EncodeProgress>();
        let relay = scope.spawn(move || {
            stop.catch(
                &format!("Scene {} progress relay", task.original_index),
                || {
                    #[cfg(all(test, target_os = "linux"))]
                    super::e11_tests::fault("relay", task.original_index)?;
                    relay_progress(
                        rx,
                        progress_tx,
                        task.encoder.total_passes(),
                        task.original_index,
                        task.frame_indices.len(),
                        total_frames,
                        total_final,
                    )
                },
            )
        });
        let encoded = stop.catch(&format!("Scene {} encoder", task.original_index), || {
            #[cfg(all(test, target_os = "linux"))]
            super::e11_tests::fault("worker", task.original_index)?;
            task.encoder
                .encode_with_stream(frames, temp, tx, clip_info)
                .or_else(anyhow::Error::downcast::<EncoderResult>)
        });
        if !encoded.as_ref().is_ok_and(|result| result.status.success()) {
            stop.stop();
        }
        let filtered = filter.as_mut().map_or(Ok(()), |stage| stage.check()).map_err(|err| {
            err.context(format!(
                "FFmpeg filter failed while encoding scene {}",
                task.original_index
            ))
        });
        if filtered.is_err() {
            stop.stop();
        }
        let relayed = relay.join().unwrap_or_else(|_| {
            Err(anyhow::anyhow!(
                "Scene {} progress relay panicked",
                task.original_index
            ))
        });
        // The filter's diagnostic explains truncated-input fallout best.
        let mut error = filtered.err();
        let result = match encoded {
            Ok(result) if result.status.success() => Some(result),
            Ok(result) => {
                retain_error(
                    &mut error,
                    ParallelEncoderError::EncoderFailed {
                        scene: task.original_index,
                        result,
                    }
                    .into(),
                );
                None
            },
            Err(err) => {
                retain_error(&mut error, err);
                None
            },
        };
        if let Err(err) = relayed {
            retain_error(&mut error, err);
        }
        if let Some(err) = error {
            return Err(err);
        }
        Ok(result.expect("successful encode retained its result"))
    })
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
        if workers == 0 {
            anyhow::bail!(ParallelEncoderError::NoWorkers);
        }
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
        // Nonstream VS decoding stays on this thread; both paths bound workers.
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
        if workers == 0 {
            anyhow::bail!(ParallelEncoderError::NoWorkers);
        }
        let clip_info = input.clip_info()?;
        let stop = Stop::new(workers.into());
        let total_final = AtomicUsize::new(0);
        let total_frames = tasks.iter().map(|task| task.frame_indices.len()).sum();
        let total_tasks = tasks.len();
        let results = stop.catch_panic("Parallel scheduling", || thread::scope(|scope| {
            // Also covers a thread-spawn panic before the scheduling loop:
            // close admission before std::thread::scope implicitly joins.
            let mut scheduling = stop.guard();
            // Rendezvous hands an admitted scene directly to an idle worker.
            // Only admitted scenes own frame channels; queued tasks remain metadata.
            let (jobs_tx, jobs_rx) = crossbeam_channel::bounded::<(
                &Task, Receiver<Cursor<Vec<u8>>>, crate::utils::semaphore::Permit, Arc<AtomicBool>
            )>(0);
            let mut handles = Vec::new();
            let setup = stop.catch("Parallel worker setup", || {
            for worker in 0..usize::from(workers).min(total_tasks) {
                let (stop, total_final, clip_info, cancelled) =
                    (&stop, &total_final, &clip_info, &cancelled);
                let jobs_rx = jobs_rx.clone();
                let progress_tx = progress_tx.clone();
                let results_tx = stream.results_tx.clone();
                let finished = Arc::clone(&stream.finished_scenes);
                handles.push(scope.spawn(move || stop.catch_panic(&format!("Parallel worker {worker}"), || {
                    #[cfg(all(test, target_os = "linux"))]
                    let _worker_count = super::e28_tests::worker_started();
                    let mut done = Vec::new();
                    loop {
                        let (task, frames_rx, _permit, decoded) = crossbeam_channel::select! {
                            recv(stop.receiver) -> _ => break,
                            recv(jobs_rx) -> job => match job { Ok(job) => job, Err(_) => break },
                        };
                        if stop.stopped(cancelled) { break; }
                        let result = stop.catch(&format!("Scene {} worker", task.original_index), || {
                            let mut guard = stop.guard();
                            #[cfg(all(test, target_os = "linux"))]
                            super::e11_tests::fault("gate", task.original_index)?;
                            debug!("Encoding Scene {}", task.original_index);
                            let started = std::time::SystemTime::now();
                            let temp = TempOutput::new(task.output.with_extension(format!("temp.{}", task.encoder.output_extension())));
                            let result = consume_frames(task, frames_rx, &temp.path, clip_info, &progress_tx,
                                total_frames, total_final, stop, STREAM_WINDOW)?;
                            // EOF alone is not success: a failed producer can leave
                            // a codec happily encoding a truncated scene. This is
                            // scene-local so successful active peers still persist.
                            if !decoded.load(Ordering::Acquire) {
                                guard.success();
                                return Ok(None);
                            }
                            let ended = std::time::SystemTime::now();
                            let bytes = temp.path.metadata()?.len();
                            let framerate = *clip_info.frame_rate.numer() as f64 / *clip_info.frame_rate.denom() as f64;
                            // Preserve the existing nonstream bitrate calculation (unrelated to E28).
                            let bitrate = (bytes * 8) as f64 / (task.frame_indices.len() as f64 * framerate);
                            finalize_scene(SceneOutcome { temp_output: temp.path.clone(), bytes, usable: true },
                                &task.output, task.original_index, &progress_tx, &stop.stopped)?;
                            let result = report_result(&results_tx, &finished, ParallelEncoderResult {
                                scene: task.original_index, started, ended, bytes, bitrate, result,
                            })?;
                            guard.success();
                            Ok(Some(result))
                        });
                        #[cfg(all(test, target_os = "linux"))]
                        if result.is_err() { super::e11_tests::collected_error(); }
                        if let Some(result) = result? { done.push((task.index, result)); }
                    }
                    Ok(done)
                })));
            }
            #[cfg(all(test, target_os = "linux"))]
            if tasks.len() > 1 { super::e11_tests::fault("schedule", 7)?; }
            Ok(())
            });
            // Callback errors stop admission immediately, but remain secondary
            // to a worker's original typed failure collected at join below.
            let mut persistence_error = None;
            let mut persist = || {
                match stop.catch("Scene persistence", || drain_finished_results(stream)) {
                    Ok(()) => true,
                    Err(err) => {
                        retain_error(&mut persistence_error, err);
                        false
                    },
                }
            };
            let scheduled = stop.catch("Scene decoding/persistence", || {
                for task in &tasks {
                    #[cfg(all(test, target_os = "linux"))]
                    super::e28_tests::before_admission(&stop, task);
                    // Reserve a real worker before creating/decoding its input.
                    // The worker owns this permit through completion, never reacquires it.
                    let Some(permit) = stop.workers.acquire_owned() else {
                        break;
                    };
                    if stop.stopped(&cancelled) {
                        break;
                    }
                    let mut guard = stop.guard();
                    if !persist() { guard.success(); break; }
                    let (frames_tx, frames_rx) = crossbeam_channel::unbounded();
                    let decoded = Arc::new(AtomicBool::new(false));
                    crossbeam_channel::select! {
                        recv(stop.receiver) -> _ => { guard.success(); break; },
                        send(jobs_tx, (task, frames_rx, permit, Arc::clone(&decoded))) -> sent => {
                            if sent.is_err() {
                                if stop.stopped(&cancelled) { guard.success(); break; }
                                anyhow::bail!("Scene {} worker handoff disconnected", task.original_index);
                            }
                        },
                    }
                    if stop.stopped(&cancelled) { guard.success(); break; }
                    #[cfg(all(test, target_os = "linux"))]
                    super::e11_tests::fault("header", task.original_index)?;
                    #[cfg(all(test, target_os = "linux"))]
                    super::e28_tests::header(task);
                    let header = input.y4m_header(Some(task.frame_indices.len()))?;
                    if let Err(err) = frames_tx.send(Cursor::new(header.into_bytes())) {
                        if stop.stopped(&cancelled) {
                            guard.success();
                            break;
                        }
                        return Err(err.into());
                    }
                    #[cfg(all(test, target_os = "linux"))]
                    super::e11_tests::fault("decode", task.original_index)?;
                    if let Err(err) = input.y4m_frames(frames_tx.clone(), &task.frame_indices) {
                        if stop.stopped(&cancelled) {
                            tracing::error!(
                                "Frame production stopped after worker failure: {err:#}"
                            );
                            guard.success();
                            break;
                        }
                        return Err(
                            err.context(format!("Scene {} frame production", task.original_index))
                        );
                    }
                    // Publish full production before EOF can reach the consumer.
                    decoded.store(true, Ordering::Release);
                    drop(frames_tx);
                    if !persist() { guard.success(); break; }
                    guard.success();
                }
                Ok(())
            });
            // EOF and admission closure precede every join, including decode or
            // callback failure. No pool-owned sender can keep a consumer alive.
            drop(jobs_tx);
            #[cfg(all(test, target_os = "linux"))]
            super::e28_tests::before_joins();
            // The final batch can finish after decoding ends. Observe callback
            // failures before blocking on any still-active peer (as streaming does).
            while !stop.stopped(&cancelled)
                && handles.iter().any(|handle| !handle.is_finished())
            {
                if !persist() { break; }
                thread::park_timeout(std::time::Duration::from_millis(10));
            }
            if stop.stopped(&cancelled) {
                stop.stop();
            }
            let mut error = setup.err();
            if let Err(err) = scheduled { retain_error(&mut error, err); }
            let mut results = vec![None; total_tasks];
            for handle in handles {
                match handle.join() {
                    Ok(Ok(done)) => {
                        for (index, result) in done { results[index] = Some(result); }
                    },
                    Ok(Err(err)) => {
                        stop.stop();
                        retain_error(&mut error, err);
                    },
                    Err(_) => {
                        stop.stop();
                        retain_error(&mut error, anyhow::anyhow!("encoder worker panicked"));
                    },
                }
            }
            if let Some(err) = persistence_error { retain_error(&mut error, err); }
            // Successful active jobs may report after a peer stops scheduling.
            // Persist those results without replacing the original failure.
            if let Err(err) = stop.catch("Scene persistence", || drain_finished_results(stream)) {
                stop.stop();
                retain_error(&mut error, err);
            }
            #[cfg(all(test, target_os = "linux"))]
            super::e11_tests::verify_pools(&stop, workers.into());
            scheduling.success();
            error.map_or_else(|| Ok(results), Err)
        }));
        drop(progress_tx);
        drop(cancelled);
        results
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A usable scene reports its size on the progress channel — the event the
    /// interface turns into the finished-scene counter and the size estimate —
    /// and is renamed to its final output.
    #[test]
    fn a_usable_scene_reports_its_size() {
        let dir = std::env::temp_dir().join(format!("cc-finalize-scene-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir should be created");
        let temp_output = dir.join("0.temp.ivf");
        let output = dir.join("0.ivf");
        std::fs::write(&temp_output, vec![7_u8; 42]).expect("temp file should be written");
        let (tx, rx) = std::sync::mpsc::channel();
        let errored = AtomicBool::new(false);

        finalize_scene(
            SceneOutcome {
                temp_output: temp_output.clone(),
                bytes: 42,
                usable: true,
            },
            &output,
            3,
            &tx,
            &errored,
        )
        .expect("finalize should succeed");

        match rx.try_recv().expect("the size event is sent") {
            SequenceStatus::Whole(Status::Processing {
                id,
                completion:
                    SequenceCompletion::Custom {
                        name,
                        completed,
                        total,
                    },
            }) => {
                assert_eq!(id, "3");
                assert_eq!(name, "size");
                assert_eq!(completed, 42.0);
                assert_eq!(total, 42.0);
            },
            other => panic!("unexpected status: {other:?}"),
        }
        assert!(output.exists(), "the encode is renamed to its final output");
        assert!(!temp_output.exists(), "the temp file is gone");
        assert!(!errored.load(Ordering::Relaxed));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An unusable encode is discarded without reporting a size — an estimate
    /// built on it would claim bytes that do not exist — and flags the error.
    #[test]
    fn an_unusable_scene_is_discarded_without_a_size() {
        let dir = std::env::temp_dir().join(format!("cc-finalize-drop-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir should be created");
        let temp_output = dir.join("1.temp.ivf");
        let output = dir.join("1.ivf");
        std::fs::write(&temp_output, vec![7_u8; 42]).expect("temp file should be written");
        let (tx, rx) = std::sync::mpsc::channel();
        let errored = AtomicBool::new(false);

        finalize_scene(
            SceneOutcome {
                temp_output: temp_output.clone(),
                bytes: 42,
                usable: false,
            },
            &output,
            1,
            &tx,
            &errored,
        )
        .expect("finalize should succeed");

        assert!(rx.try_recv().is_err(), "a discarded scene reports no size");
        assert!(!output.exists(), "a discarded scene produces no output");
        assert!(!temp_output.exists(), "the temp output is removed");
        assert!(errored.load(Ordering::Relaxed), "the error is flagged");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
