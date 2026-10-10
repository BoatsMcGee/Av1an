use super::*;

impl SceneConcatenator {
    #[inline]
    pub fn ivf(
        output: &Path,
        scene_paths: &[PathBuf],
        progress_tx: &sync::mpsc::Sender<SequenceStatus>,
        cancelled: &Arc<AtomicBool>,
    ) -> Result<()> {
        let output = File::create(output)?;
        let mut muxer = MuxerContext::new(IvfMuxer::new(), Writer::new(output));
        let global_info = {
            let acc = AccReader::new(std::fs::File::open(&scene_paths[0])?);
            let mut demuxer = DemuxerContext::new(IvfDemuxer::new(), acc);
            demuxer.read_headers()?;
            // attempt to set the duration correctly
            let duration = demuxer.info.duration.unwrap_or(0)
                + scene_paths.iter().skip(1).try_fold(0u64, |sum, file| -> anyhow::Result<_> {
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
                        Event::MoreDataNeeded(sz) => panic!("needed more data: {sz} bytes"),
                        Event::NewStream(s) => panic!("new stream: {s:?}"),
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
                        _ => unimplemented!(),
                    },
                    Err(e) => {
                        error!("{:?}", e);
                        break;
                    },
                }
            }
            pos_offset += last_pos + 1;
        }

        muxer.write_trailer()?;

        Ok(())
    }
}

