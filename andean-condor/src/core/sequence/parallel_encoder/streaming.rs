//! Bounded frame streaming; active finite jobs finish on admission shutdown.
use std::{
    collections::VecDeque,
    io::Cursor,
    sync::atomic::{AtomicBool, AtomicUsize},
    thread,
};

use anyhow::Result;

use super::{
    ParallelEncoder,
    encode::{SceneOutcome, consume_frames, finalize_scene},
    error::ParallelEncoderError,
    lifecycle::{Stop, TempOutput, retain_error},
    task::{ParallelEncoderResult, ResultStream, Task},
};
use crate::core::{input::Input, sequence::SequenceStatus};

impl ParallelEncoder {
    #[allow(clippy::too_many_lines, clippy::too_many_arguments)]
    pub(super) fn encode_tasks_streaming(
        input: &mut Input,
        workers: u8,
        window: usize,
        tasks: VecDeque<Task>,
        progress_tx: &std::sync::mpsc::Sender<SequenceStatus>,
        cancelled: &AtomicBool,
        stream: &mut ResultStream<'_, impl FnMut(Vec<ParallelEncoderResult>) -> Result<()>>,
    ) -> Result<Vec<Option<ParallelEncoderResult>>> {
        if workers == 0 {
            anyhow::bail!(ParallelEncoderError::NoWorkers);
        }
        let total_tasks = tasks.len();
        let workers = usize::from(workers).min(total_tasks.max(1));
        let window = window.max(1);
        let clip_info = input.clip_info()?;
        let framerate = *clip_info.frame_rate.numer() as f64 / *clip_info.frame_rate.denom() as f64;
        let total_frames = tasks.iter().map(|task| task.frame_indices.len()).sum();
        let headers = tasks
            .iter()
            .map(|task| {
                Ok((
                    task.index,
                    input.y4m_header(Some(task.frame_indices.len()))?,
                ))
            })
            .collect::<Result<std::collections::HashMap<_, _>>>()?;
        let sources = input.frame_sources(workers)?;
        let (task_tx, task_rx) = crossbeam_channel::unbounded();
        for task in tasks {
            task_tx.send(task)?;
        }
        drop(task_tx);
        let stop = Stop::new(workers);
        let total_final = AtomicUsize::new(0);
        stop.catch_panic("Streaming scheduling", || {
            thread::scope(|scope| {
                let mut scheduling = stop.guard();
                let mut handles = Vec::new();
                let setup = stop.catch("Streaming worker setup", || {
                    for (worker, source) in sources.into_iter().enumerate() {
                        let task_rx = task_rx.clone();
                        let results_tx = stream.results_tx.clone();
                        let finished = std::sync::Arc::clone(&stream.finished_scenes);
                        let (stop, headers, total_final, clip_info) =
                            (&stop, &headers, &total_final, &clip_info);
                        handles.push(scope.spawn(move || {
                            let mut scene = None;
                            let result = stop.catch(&format!("Streaming worker {worker}"), || {
                                let mut guard = stop.guard();
                                #[cfg(all(test, target_os = "linux"))]
                                super::e11_tests::fault("open", 7)?;
                                let mut reader = source.open()?;
                                let mut done = Vec::new();
                                while !stop.stopped(cancelled) {
                                    let Ok(task) = task_rx.recv() else { break };
                                    if stop.stopped(cancelled) {
                                        break;
                                    }
                                    scene = Some(task.original_index);
                                    let task_result = stop.catch(
                                        &format!("Scene {} worker", task.original_index),
                                        || {
                                            let temp = TempOutput::new(task.output.with_extension(
                                                format!("temp.{}", task.encoder.output_extension()),
                                            ));
                                            let started = std::time::SystemTime::now();
                                            let (tx, rx) =
                                                crossbeam_channel::bounded::<Cursor<Vec<u8>>>(
                                                    window,
                                                );
                                            tx.send(Cursor::new(
                                                headers[&task.index].clone().into_bytes(),
                                            ))?;
                                            let (encoded, produced) = thread::scope(|inner| {
                                                let (task, temp) = (&task, &temp.path);
                                                let encoder = inner.spawn(move || {
                                                    stop.catch(
                                                        &format!(
                                                            "Scene {} consumer",
                                                            task.original_index
                                                        ),
                                                        || {
                                                            consume_frames(
                                                                task,
                                                                rx,
                                                                temp,
                                                                clip_info,
                                                                progress_tx,
                                                                total_frames,
                                                                total_final,
                                                                stop,
                                                                window,
                                                            )
                                                        },
                                                    )
                                                });
                                                let produced = stop.catch(
                                                    &format!(
                                                        "Scene {} frame production",
                                                        task.original_index
                                                    ),
                                                    || {
                                                        #[cfg(all(test, target_os = "linux"))]
                                                        super::e11_tests::fault(
                                                            "produce",
                                                            task.original_index,
                                                        )?;
                                                        reader.y4m_frames(
                                                            &tx,
                                                            &task.frame_indices,
                                                            window,
                                                        )
                                                    },
                                                );
                                                drop(tx); // EOF before joining the consumer and its filter/relays.
                                                let encoded = encoder.join().unwrap_or_else(|_| {
                                                    Err(anyhow::anyhow!(
                                                        "Scene {} encoder panicked",
                                                        task.original_index
                                                    ))
                                                });
                                                (encoded, produced)
                                            });
                                            let mut error = None;
                                            let result = match encoded {
                                                Ok(result) => Some(result),
                                                Err(err) => {
                                                    retain_error(&mut error, err);
                                                    None
                                                },
                                            };
                                            if let Err(err) = produced {
                                                retain_error(&mut error, err);
                                            }
                                            if let Some(err) = error {
                                                return Err(err);
                                            }
                                            let ended = std::time::SystemTime::now();
                                            let bytes = temp.path.metadata()?.len();
                                            finalize_scene(
                                                SceneOutcome {
                                                    temp_output: temp.path.clone(),
                                                    bytes,
                                                    usable: true,
                                                },
                                                &task.output,
                                                task.original_index,
                                                progress_tx,
                                                &stop.stopped,
                                            )?;
                                            let result = ParallelEncoderResult {
                                                scene: task.original_index,
                                                started,
                                                ended,
                                                bytes,
                                                bitrate: (bytes * 8) as f64
                                                    / (task.frame_indices.len() as f64 / framerate),
                                                result: result.expect("successful consumer result"),
                                            };
                                            results_tx.send(result.clone())?;
                                            finished.release();
                                            Ok(result)
                                        },
                                    )?;
                                    done.push((task.index, task_result));
                                }
                                guard.success();
                                Ok(done)
                            });
                            result.map_err(|err| {
                                err.context(scene.map_or_else(
                                    || format!("Streaming worker {worker}"),
                                    |scene| format!("Scene {scene} on streaming worker {worker}"),
                                ))
                            })
                        }));
                    }
                    Ok(())
                });
                // The public stream retains a sender: completion is determined by
                // handles, not channel disconnection. Callback failure stops NOW,
                // before any blocking join, rather than letting workers drain tasks.
                let persisted = stop.catch("Scene persistence", || {
                    loop {
                        super::progress::drain_finished_results(stream)?;
                        if handles.iter().all(thread::ScopedJoinHandle::is_finished) {
                            break;
                        }
                        thread::park_timeout(std::time::Duration::from_millis(10));
                    }
                    super::progress::drain_finished_results(stream)
                });
                let mut error = setup.err();
                if let Err(err) = persisted {
                    retain_error(&mut error, err);
                }
                if stop.stopped(cancelled) {
                    stop.stop();
                }
                let mut ordered = vec![None; total_tasks];
                for handle in handles {
                    match handle.join() {
                        Ok(Ok(results)) => {
                            for (index, result) in results {
                                ordered[index] = Some(result);
                            }
                        },
                        Ok(Err(err)) => {
                            stop.stop();
                            retain_error(&mut error, err);
                        },
                        Err(_) => {
                            stop.stop();
                            retain_error(&mut error, anyhow::anyhow!("Streaming worker panicked"));
                        },
                    }
                }
                #[cfg(all(test, target_os = "linux"))]
                super::e11_tests::verify_pools(&stop, workers);
                scheduling.success();
                error.map_or_else(|| Ok(ordered), Err)
            })
        })
    }
}
