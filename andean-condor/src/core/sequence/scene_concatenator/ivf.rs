use super::*;
use std::io::{Seek, SeekFrom};

fn count_record(count: u32) -> Result<u32> {
    count.checked_add(1).context("IVF record count exceeds u32")
}

// av-ivf 0.5 discards the header timebase; supplement it without replacing the codec pipeline.
fn read_header(path: &Path) -> Result<([u8; 32], Ratio<i64>)> {
    let mut header = [0; 32];
    File::open(path)
        .with_context(|| format!("opening IVF {}", path.display()))?
        .read_exact(&mut header)
        .with_context(|| format!("reading IVF header {}", path.display()))?;
    if &header[..4] != b"DKIF" || header[4..8] != [0, 0, 32, 0] {
        bail!(
            "{} requires a standard version 0, 32-byte IVF header",
            path.display()
        );
    }
    let rate = u32::from_le_bytes(header[16..20].try_into().expect("4 bytes"));
    let scale = u32::from_le_bytes(header[20..24].try_into().expect("4 bytes"));
    if rate == 0 || scale == 0 {
        bail!("{} has an invalid zero IVF rate/scale", path.display());
    }
    let timebase = Ratio::new(i64::from(scale), i64::from(rate));
    // The pinned muxer narrows its reduced rational to signed 32-bit components.
    if i32::try_from(*timebase.numer()).is_err() || i32::try_from(*timebase.denom()).is_err() {
        bail!(
            "{} IVF timebase exceeds the muxer's i32 range",
            path.display()
        );
    }
    Ok((header, timebase))
}

impl SceneConcatenator {
    /// Concatenates IVF parts using the input/timestamp policy of [`Self::ivf_cancel`].
    /// Scratch writer: the caller must exclusively own `output`; this can overwrite it.
    /// Use the final Sequence boundary for saved overwrite/source protection and publication.
    #[inline]
    pub fn ivf(
        output: &Path,
        scene_paths: &[PathBuf],
        progress_tx: &sync::mpsc::Sender<SequenceStatus>,
        cancelled: &Arc<AtomicBool>,
    ) -> Result<()> {
        Self::ivf_cancel(
            output,
            scene_paths,
            progress_tx,
            &Cancellation::from(Arc::clone(cancelled)),
        )
    }

    /// Concatenates nonempty standard version-0/32-byte-header IVF parts with matching
    /// codec/dimensions and equal rational timebases (reduced components must fit i32).
    /// Input counts are hints: zero means unknown; output counts actual records, including
    /// invisible frames, not decoded visible frames. The total must fit u32.
    /// Timestamps must be nondecreasing and fit usize, as must appended timestamps.
    /// Equal PTS and gaps are preserved; each origin is removed once. The next part starts
    /// one nominal tick (scale/rate seconds) after the previous part's final record.
    /// Caller must exclusively own the scratch output (it may be truncated on error).
    #[inline]
    pub fn ivf_cancel(
        output: &Path,
        scene_paths: &[PathBuf],
        progress_tx: &sync::mpsc::Sender<SequenceStatus>,
        cancellation: &Cancellation,
    ) -> Result<()> {
        cancellation.check_admission()?;
        Self::validate_scene_paths(scene_paths)?;
        let mut partial = InterruptedOutput::new(output, cancellation);
        let output = File::create(output)?;
        let mut muxer = MuxerContext::new(IvfMuxer::new(), Writer::new(output));
        let headers =
            scene_paths.iter().map(|path| read_header(path)).collect::<Result<Vec<_>>>()?;
        let (first_header, timebase) = &headers[0];
        for ((header, tb), path) in headers.iter().zip(scene_paths) {
            if header[8..16] != first_header[8..16] || tb != timebase {
                bail!(
                    "{} has an incompatible IVF codec, dimensions or timebase",
                    path.display()
                );
            }
        }
        let global_info = {
            let acc = AccReader::new(std::fs::File::open(&scene_paths[0])?);
            let mut demuxer = DemuxerContext::new(IvfDemuxer::new(), acc);
            demuxer.read_headers()?;
            let mut info = demuxer.info;
            info.timebase = Some(*timebase);
            info.duration = None;
            // av-ivf 0.5 reads first-stream duration. Leave the provisional count unknown;
            // rav1e's header count is always zero, so patch the actual record count at EOF.
            let stream = info.streams.first_mut().context("IVF has no video stream")?;
            stream.duration = None;
            stream.timebase = *timebase;
            info
        };

        muxer.set_global_info(global_info)?;
        muxer.configure()?;
        muxer.write_header()?;

        let total_scenes = scene_paths.len();
        let mut timestamp_offset = 0u64;
        let mut record_count = 0u32;
        for (index, file) in scene_paths.iter().enumerate() {
            cancellation.check_forced()?;

            Self::send_progress(
                progress_tx,
                (index + 1) as f64 / total_scenes as f64 * 100.0,
            );

            let mut origin = None;
            let mut previous_timestamp = None;
            let mut last_local = 0u64;
            let input = std::fs::File::open(file)?;
            // Reject timestamps the adapter's u64 -> usize conversion would truncate,
            // including on 32-bit targets. Only framing is read, not codec payloads.
            let mut frame_headers = BufReader::new(File::open(file)?);
            frame_headers.seek(SeekFrom::Start(32))?;

            let acc = AccReader::new(input);

            let mut demuxer = DemuxerContext::new(IvfDemuxer::new(), acc);
            demuxer.read_headers()?;

            trace!("global info: {:#?}", demuxer.info);

            loop {
                cancellation.check_forced()?;
                match demuxer.read_event() {
                    Ok(event) => match event {
                        Event::MoreDataNeeded(sz) => panic!("needed more data: {sz} bytes"),
                        Event::NewStream(s) => panic!("new stream: {s:?}"),
                        Event::NewPacket(mut packet) => {
                            let mut frame_header = [0; 12];
                            frame_headers.read_exact(&mut frame_header).with_context(|| {
                                format!("reading IVF frame header {}", file.display())
                            })?;
                            let size =
                                u32::from_le_bytes(frame_header[..4].try_into().expect("4 bytes"));
                            let timestamp = u64::from_le_bytes(
                                frame_header[4..12].try_into().expect("8 bytes"),
                            );
                            frame_headers.seek_relative(i64::from(size))?;
                            let carrier = usize::try_from(timestamp).with_context(|| {
                                format!("{} IVF timestamp exceeds usize", file.display())
                            })?;
                            // COMPAT: av-ivf 0.5 demuxer/muxer carry IVF timestamps in pos;
                            // packet.t.pts is unset. pos is NOT a byte offset in this adapter.
                            if packet.pos != Some(carrier) || packet.data.len() != size as usize {
                                bail!(
                                    "{} IVF timestamp/framing does not match the adapter",
                                    file.display()
                                );
                            }
                            if previous_timestamp.is_some_and(|previous| timestamp < previous) {
                                bail!(
                                    "{} IVF timestamp decreases at output record {record_count}",
                                    file.display()
                                );
                            }
                            previous_timestamp = Some(timestamp);
                            last_local = timestamp
                                .checked_sub(*origin.get_or_insert(timestamp))
                                .with_context(|| {
                                    format!("{} IVF timestamp underflow", file.display())
                                })?;
                            let appended =
                                last_local.checked_add(timestamp_offset).with_context(|| {
                                    format!("{} IVF timestamp offset overflow", file.display())
                                })?;
                            packet.pos = Some(usize::try_from(appended).with_context(|| {
                                format!("{} appended IVF timestamp exceeds usize", file.display())
                            })?);
                            record_count = count_record(record_count).with_context(|| {
                                format!("counting IVF records in {}", file.display())
                            })?;

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
            if origin.is_none() {
                bail!("{} has no IVF records", file.display());
            }
            if index + 1 < total_scenes {
                // IVF ticks are scale/rate seconds; retain gaps, with one nominal tick
                // after the final frame. Do not add any part's absolute origin twice.
                timestamp_offset = timestamp_offset
                    .checked_add(last_local)
                    .and_then(|end| end.checked_add(1))
                    .with_context(|| format!("{} IVF timestamp end overflow", file.display()))?;
            }
        }

        muxer.write_trailer()?;
        let mut writer = muxer.into_writer();
        writer.seek(SeekFrom::Start(24)).context("seeking IVF output record count")?;
        writer
            .write_all(&record_count.to_le_bytes())
            .context("writing IVF output record count")?;
        writer.flush().context("flushing IVF output record count")?;
        drop(writer);
        partial.complete()
    }
}

#[cfg(test)]
mod tests {
    use super::count_record;

    #[test]
    fn x07_actual_record_count_rejects_overflow() {
        assert_eq!(
            count_record(u32::MAX - 1).expect("last representable record"),
            u32::MAX
        );
        assert!(
            count_record(u32::MAX)
                .expect_err("record count overflow")
                .to_string()
                .contains("record count")
        );
    }
}
