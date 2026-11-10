use anyhow::bail;

use super::*;

impl SceneConcatenator {
    #[inline]
    pub fn ivf(
        output: &Path,
        scene_paths: &[PathBuf],
        progress_tx: &sync::mpsc::Sender<SequenceStatus>,
        cancelled: &Arc<AtomicBool>,
    ) -> Result<()> {
        let Some((first, rest)) = scene_paths.split_first() else {
            bail!(SceneConcatenatorError::NoSceneFiles);
        };

        let output = File::create(output)?;
        let mut muxer = MuxerContext::new(IvfMuxer::new(), Writer::new(output));
        let global_info = {
            let acc = AccReader::new(std::fs::File::open(first)?);
            let mut demuxer = DemuxerContext::new(IvfDemuxer::new(), acc);
            demuxer.read_headers()?;
            // attempt to set the duration correctly
            let duration = demuxer.info.duration.unwrap_or(0)
                + rest.iter().try_fold(0u64, |sum, file| -> anyhow::Result<_> {
                    let acc = AccReader::new(std::fs::File::open(file)?);
                    let mut demuxer = DemuxerContext::new(IvfDemuxer::new(), acc);

                    demuxer.read_headers()?;
                    Ok(sum + demuxer.info.duration.unwrap_or(0))
                })?;

            let mut info = demuxer.info;
            info.duration = Some(duration);
            info
        };

        muxer.set_global_info(global_info)?;
        muxer.configure()?;
        muxer.write_header()?;

        let total_scenes = scene_paths.len();
        let mut pos_offset: usize = 0;
        for (index, file) in scene_paths.iter().enumerate() {
            if cancelled.load(sync::atomic::Ordering::Relaxed) {
                return Ok(());
            }

            Self::send_progress(
                progress_tx,
                (index + 1) as f64 / total_scenes as f64 * 100.0,
            );

            let mut last_pos: usize = 0;
            let input = std::fs::File::open(file)?;

            let acc = AccReader::new(input);

            let mut demuxer = DemuxerContext::new(IvfDemuxer::new(), acc);
            demuxer.read_headers()?;

            trace!("global info: {:#?}", demuxer.info);

            loop {
                match demuxer.read_event() {
                    Ok(event) => match event {
                        // A demuxer asking for more data has already consumed
                        // everything the reader has, so the scene is truncated.
                        Event::MoreDataNeeded(sz) => {
                            bail!(SceneConcatenatorError::TruncatedScene {
                                scene:  index,
                                needed: sz,
                            });
                        },
                        // IVF carries one stream per file; a second one means
                        // the input is not what this concatenator expects.
                        Event::NewStream(s) => {
                            bail!(SceneConcatenatorError::UnexpectedStream {
                                scene:  index,
                                stream: format!("{s:?}"),
                            });
                        },
                        Event::NewPacket(mut packet) => {
                            if let Some(p) = packet.pos.as_mut() {
                                last_pos = *p;
                                *p += pos_offset;
                            }

                            trace!("received packet with pos: {:?}", packet.pos);
                            muxer.write_packet(Arc::new(packet))?;
                        },
                        Event::Continue => {
                            // do nothing
                        },
                        Event::Eof => {
                            trace!("EOF received.");
                            break;
                        },
                        // `Event` is non-exhaustive; an unhandled variant means
                        // this demuxer emitted something the concatenator does
                        // not understand, so the output would be wrong.
                        other => {
                            bail!(SceneConcatenatorError::UnexpectedEvent {
                                scene: index,
                                event: format!("{other:?}"),
                            });
                        },
                    },
                    // A demux error means the scene is unreadable, so the
                    // concatenated output would be silently short.
                    Err(e) => {
                        bail!(SceneConcatenatorError::DemuxFailed {
                            scene:  index,
                            source: e,
                        });
                    },
                }
            }
            pos_offset += last_pos + 1;
        }

        muxer.write_trailer()?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_directory(name: &str) -> std::path::PathBuf {
        let directory = std::env::temp_dir().join(format!(
            "av1an-scene-concatenator-ivf-{}-{name}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).expect("temp directory created");
        directory
    }

    /// An empty scene list used to index `scene_paths[0]` and panic.
    #[test]
    fn an_empty_scene_list_is_an_error_not_a_panic() {
        let directory = temp_directory("empty");
        let (progress_tx, _progress_rx) = sync::mpsc::channel();

        let result = SceneConcatenator::ivf(
            &directory.join("output.ivf"),
            &[],
            &progress_tx,
            &Arc::new(AtomicBool::new(false)),
        );

        assert!(result.is_err(), "an empty scene list must be an error");
        let _ = std::fs::remove_dir_all(&directory);
    }

    /// A missing scene file used to surface as a bare I/O error from deep in
    /// the duration fold, with no indication of which scene was at fault.
    #[test]
    fn a_missing_scene_file_names_the_scene() {
        let directory = temp_directory("missing");
        let present = directory.join("0.ivf");
        std::fs::write(&present, b"not an ivf file").expect("file written");
        let (progress_tx, _progress_rx) = sync::mpsc::channel();

        let result = SceneConcatenator::ivf(
            &directory.join("output.ivf"),
            &[present, directory.join("1.ivf")],
            &progress_tx,
            &Arc::new(AtomicBool::new(false)),
        );

        assert!(result.is_err(), "a missing scene file must be an error");
        let _ = std::fs::remove_dir_all(&directory);
    }
}
