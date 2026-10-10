//! Native FFMS2 input: construction, format filtering, and y4m serialisation.

use std::{
    ffi::{CString, c_char, c_void},
    io::Write,
    path::Path,
    sync::{
        Once,
        atomic::{AtomicBool, Ordering},
    },
};

use anyhow::Result;
use av_decoders::{Decoder, Ffms2Decoder};
use ffms2_sys::{
    FFMS_CreateIndexer,
    FFMS_DestroyIndex,
    FFMS_DoIndexing2,
    FFMS_ErrorInfo,
    FFMS_IndexBelongsToFile,
    FFMS_Init,
    FFMS_ReadIndex,
    FFMS_SetProgressCallback,
    FFMS_TrackType,
    FFMS_TrackTypeIndexSettings,
    FFMS_WriteIndex,
};

use crate::{
    core::input::{
        IndexProgress,
        Input,
        InputError,
        RawFrame,
        RawPlane,
        clip_info::ClipInfo,
        ffms2_filter::{Ffms2Filter, ffms2_pixel_format},
        frame_feed,
        pixel_format::PixelFormat,
    },
    ffmpeg::get_clip_info,
    models::input::{ImportMethod, Input as InputModel},
};

/// Precedes every frame's planes in a y4m stream.
const FRAME_HEADER: &[u8] = b"FRAME\n";

/// Applies each filter in turn. A filter's unset fields inherit from the
/// previous one, and the first filter's from the decoded stream. Applied before
/// any frame is read, so the decoder's `VideoDetails` describes what it
/// produces.
#[inline]
fn apply_filters(decoder: &mut Ffms2Decoder, filters: &[Ffms2Filter]) -> Result<()> {
    for filter in filters {
        let Ffms2Filter::OutputFormat {
            bit_depth,
            chroma,
            width,
            height,
        } = filter;
        let current = decoder.video_details;

        let bit_depth = bit_depth.map_or(current.bit_depth, usize::from);
        let chroma = chroma.map_or(current.chroma_sampling, |chroma| {
            chroma.to_chroma_subsampling()
        });
        let width = width.map_or(current.width, |width| {
            usize::try_from(width).unwrap_or(current.width)
        });
        let height = height.map_or(current.height, |height| {
            usize::try_from(height).unwrap_or(current.height)
        });

        // Rejected up front: FFMS2 would otherwise substitute a format.
        ffms2_pixel_format(bit_depth, chroma)
            .ok_or_else(|| anyhow::anyhow!("FFMS2 cannot output {bit_depth}-bit {chroma:?}"))?;

        decoder.set_output_format(width, height, bit_depth as u8, chroma)?;
    }
    Ok(())
}

/// Validates that a native input path names an existing, non-script video file.
pub fn validate(path: &Path) -> Result<()> {
    anyhow::ensure!(
        path.exists(),
        InputError::VideoFileNotFound(path.to_owned())
    );
    if let Some(ext) = path.extension() {
        anyhow::ensure!(
            ext != "vpy" && ext != "py",
            InputError::NotAVideoFile(path.to_owned())
        );
    }
    Ok(())
}

/// Opens a native FFMS2 input and applies its filters.
pub fn from_video(data: &InputModel) -> Result<Input> {
    let InputModel::Video {
        path,
        import_method,
        filters,
    } = data
    else {
        panic!("expected `Input::Video`");
    };

    Input::validate(data)?;
    match import_method {
        // ImportMethod::FFmpeg {} => {
        //     unimplemented!();
        // },
        ImportMethod::FFMS2 {
            index,
        } => {
            let mut ffms2_decoder = Ffms2Decoder::new(path, *index)?;

            // Captured before any filter runs, so a filter's unset fields resolve
            // against the source rather than the filtered format.
            let source_details = ffms2_decoder.video_details;

            apply_filters(&mut ffms2_decoder, filters)?;

            let decoder =
                Decoder::from_decoder_impl(av_decoders::DecoderImpl::Ffms2(ffms2_decoder))?;

            Ok(Input::Video {
                path: path.clone(),
                import_method: import_method.clone(),
                filters: filters.clone(),
                source_details,
                decoder,
                clip_info: None,
            })
        },
    }
}

/// Describes a native input's clip. `ffprobe` reads the source file, so a
/// filter's conversion overrides resolution and pixel format here; `ffprobe`
/// remains the only source of colour range and transfer characteristics.
pub fn clip_info(path: &Path, filtered: bool, decoder: &Decoder) -> Result<ClipInfo> {
    let mut info = get_clip_info(path)?;
    if filtered {
        let details = decoder.get_video_details();
        info.resolution = (details.width as u32, details.height as u32);
        info.format_info = PixelFormat::FFmpeg {
            format: ffms2_pixel_format(details.bit_depth, details.chroma_sampling)
                .unwrap_or(info.format_info.as_pixel_format()?),
        };
    }
    Ok(info)
}

const ERR_BUFFER_SIZE: usize = 1024;

fn empty_error_info(buffer: &mut [c_char; ERR_BUFFER_SIZE]) -> FFMS_ErrorInfo {
    FFMS_ErrorInfo {
        ErrorType:  0,
        SubType:    0,
        BufferSize: ERR_BUFFER_SIZE as i32,
        Buffer:     buffer.as_mut_ptr(),
    }
}

fn error_message(err: FFMS_ErrorInfo) -> String {
    if err.Buffer.is_null() {
        return "unknown FFMS2 error".to_owned();
    }
    // SAFETY: FFMS2 wrote a nul-terminated C string into the caller-owned buffer.
    unsafe { std::ffi::CStr::from_ptr(err.Buffer) }.to_string_lossy().into_owned()
}

/// Context shared with the FFMS2 progress trampoline for the duration of
/// `FFMS_DoIndexing2` (a synchronous call on this thread).
///
/// Two lifetimes rather than one: `&mut` is invariant over its lifetime, so
/// the two borrows cannot be forced to a common one without rejecting inputs
/// whose callback borrow is longer than the cancel flag's (or vice versa).
struct ProgressContext<'progress, 'cancelled> {
    progress:  Option<IndexProgress<'progress>>,
    cancelled: Option<&'cancelled AtomicBool>,
}

/// Trampoline the FFMS2 indexer calls with `(current, total)`. Returning
/// non-zero asks FFMS2 to cancel indexing.
unsafe extern "C" fn progress_trampoline(
    current: i64,
    total: i64,
    opaque: *mut c_void,
) -> std::os::raw::c_int {
    // SAFETY: `opaque` is the `ProgressContext` borrowed for this call's lifetime.
    let context = unsafe { &mut *(opaque as *mut ProgressContext<'_, '_>) };
    if let Some(cancelled) = context.cancelled
        && cancelled.load(Ordering::Relaxed)
    {
        return 1;
    }
    if let Some(progress) = context.progress.as_mut() {
        (**progress)(current.max(0) as u64, total.max(0) as u64);
    }
    0
}

/// Ensure `{path}.ffindex` exists and belongs to `path`, indexing with progress
/// if needed. Returns `true` when an index was built (false if one already
/// existed).
///
/// This drives FFMS2's indexer directly so progress can be reported — the same
/// indexer `av-decoders` uses, writing the same `.ffindex` format — so a later
/// `Ffms2Decoder::new` finds the cache valid and skips indexing entirely.
pub fn index_video_with_progress(
    path: &Path,
    mut progress: Option<IndexProgress<'_>>,
    cancelled: Option<&AtomicBool>,
) -> Result<bool> {
    // FFMS2 is initialised once per process; av-decoders does the same.
    static FFMS2_INIT: Once = Once::new();
    FFMS2_INIT.call_once(|| {
        // SAFETY: FFI call with infallible parameters.
        unsafe {
            FFMS_Init(0, 0);
        }
    });

    let input_cstring = CString::new(path.to_string_lossy().as_bytes())
        .map_err(|e| anyhow::anyhow!("invalid input path: {e}"))?;
    let index_path = format!("{}.ffindex", path.to_string_lossy());
    let index_cstring = CString::new(index_path.as_str())
        .map_err(|e| anyhow::anyhow!("invalid index path: {e}"))?;

    // A valid cached index short-circuits indexing entirely.
    let mut err_buffer = [0 as c_char; ERR_BUFFER_SIZE];
    let cached_index = if Path::new(&index_path).exists() {
        // SAFETY: pointers are live and the error buffer is valid.
        unsafe {
            FFMS_ReadIndex(
                index_cstring.as_ptr(),
                &mut empty_error_info(&mut err_buffer),
            )
        }
    } else {
        std::ptr::null_mut()
    };
    if !cached_index.is_null() {
        // SAFETY: `cached_index` is a live index handle.
        let belongs = unsafe {
            FFMS_IndexBelongsToFile(
                cached_index,
                input_cstring.as_ptr(),
                &mut empty_error_info(&mut err_buffer),
            )
        };
        // SAFETY: `cached_index` is a live index handle we always destroy here.
        unsafe {
            FFMS_DestroyIndex(cached_index);
        }
        if belongs == 0 {
            return Ok(false);
        }
    }

    // Reaching here means the cache was missing or stale, so this is the only
    // point that can honestly claim indexing is starting.
    if let Some(progress) = progress.as_mut() {
        (**progress)(0, 0);
    }

    // SAFETY: `input_cstring` and the error buffer are live.
    let indexer = unsafe {
        FFMS_CreateIndexer(
            input_cstring.as_ptr(),
            &mut empty_error_info(&mut err_buffer),
        )
    };
    if indexer.is_null() {
        anyhow::bail!(
            "failed to create FFMS2 indexer for {}: {}",
            path.display(),
            error_message(empty_error_info(&mut err_buffer))
        );
    }

    // SAFETY: `indexer` is a live indexer handle; each call only toggles which
    // track types it indexes, and audio/data/subtitles/attachments would only
    // slow indexing down.
    unsafe {
        FFMS_TrackTypeIndexSettings(indexer, FFMS_TrackType::FFMS_TYPE_AUDIO as i32, 0, 0);
        FFMS_TrackTypeIndexSettings(indexer, FFMS_TrackType::FFMS_TYPE_DATA as i32, 0, 0);
        FFMS_TrackTypeIndexSettings(indexer, FFMS_TrackType::FFMS_TYPE_SUBTITLE as i32, 0, 0);
        FFMS_TrackTypeIndexSettings(indexer, FFMS_TrackType::FFMS_TYPE_ATTACHMENT as i32, 0, 0);
    }

    let mut context = ProgressContext {
        progress,
        cancelled,
    };
    if context.progress.is_some() || context.cancelled.is_some() {
        // SAFETY: `indexer` is live and `context` outlives the synchronous
        // `DoIndexing2` call that invokes the trampoline, which consumes the
        // indexer so the pointer cannot be used again afterwards.
        unsafe {
            FFMS_SetProgressCallback(
                indexer,
                Some(progress_trampoline),
                std::ptr::from_mut(&mut context).cast(),
            );
        }
    }

    // SAFETY: `indexer` is live; `DoIndexing2` consumes it and returns the index.
    let idx = unsafe { FFMS_DoIndexing2(indexer, 0, &mut empty_error_info(&mut err_buffer)) };
    if idx.is_null() {
        if context.cancelled.is_some_and(|c| c.load(Ordering::Relaxed)) {
            anyhow::bail!("indexing of {} was cancelled", path.display());
        }
        anyhow::bail!(
            "failed to index {}: {}",
            path.display(),
            error_message(empty_error_info(&mut err_buffer))
        );
    }

    // SAFETY: `idx` is a live index handle; writing and destroying it is our job.
    let write_result = unsafe {
        FFMS_WriteIndex(
            index_cstring.as_ptr(),
            idx,
            &mut empty_error_info(&mut err_buffer),
        )
    };
    // SAFETY: `idx` is a live index handle, and destroying it is our job.
    unsafe {
        FFMS_DestroyIndex(idx);
    }
    if write_result != 0 {
        anyhow::bail!(
            "failed to write index {}: {}",
            index_path,
            error_message(empty_error_info(&mut err_buffer))
        );
    }

    Ok(true)
}

/// Writes a single decoded frame to `stream` as a complete y4m frame.
pub fn y4m_frame(decoder: &mut Decoder, index: usize, stream: &mut Vec<u8>) -> Result<()> {
    pack_frame(decoder, index, stream)
}

/// Streams `frame_indices` as y4m frames through `emit`, in order. Each frame
/// is emitted before the next is decoded, so `emit` bounds resident frames.
pub fn y4m_frames(
    decoder: &mut Decoder,
    frame_indices: &[usize],
    mut emit: impl FnMut(Vec<u8>) -> Result<()>,
) -> Result<()> {
    for &index in frame_indices {
        let mut buffer = Vec::new();
        pack_frame(decoder, index, &mut buffer)?;
        emit(buffer)?;
    }
    Ok(())
}

/// Packs `index` into `out` as a complete y4m frame — header plus plane data —
/// straight from FFMS2's own planes, without building an av-decoders `Frame`.
fn pack_frame(decoder: &mut Decoder, index: usize, out: &mut Vec<u8>) -> Result<()> {
    let ffms2 = decoder
        .get_ffms2_impl()
        .ok_or_else(|| anyhow::anyhow!("native input does not decode with FFMS2"))?;
    let video_source = ffms2.video_source;
    let details = ffms2.video_details;
    // SAFETY: `video_source` belongs to `decoder`, which this call borrows, and
    // `frame` is local — its planes are read into `out` here and the view
    // cannot outlive this call, so a later `FFMS_GetFrame` cannot overlap it.
    let frame = unsafe { frame_feed::ffms2_frame_at(video_source, &details, index)? };
    out.write_all(FRAME_HEADER)?;
    write_raw_frame(out, &frame);
    Ok(())
}

/// Appends a raw frame's planes to `stream` in Y, U, V order, visible rows
/// only — y4m carries no stride padding. Monochrome frames carry luma alone.
fn write_raw_frame(stream: &mut Vec<u8>, frame: &RawFrame<'_>) {
    write_raw_plane(stream, &frame.luma);
    if let Some((cb, cr)) = &frame.chroma {
        write_raw_plane(stream, cb);
        write_raw_plane(stream, cr);
    }
}

/// Appends one raw plane's visible rows to `stream`.
fn write_raw_plane(stream: &mut Vec<u8>, plane: &RawPlane<'_>) {
    for row in 0..plane.height() {
        stream.extend_from_slice(plane.row(row));
    }
}

#[cfg(test)]
mod tests {
    use av_decoders::v_frame::{
        chroma::ChromaSubsampling,
        frame::Frame,
        pixel::Pixel,
        plane::Plane,
    };

    use super::*;

    /// A pixel type the Frame-era writer serialises. `Pixel` is sealed to
    /// `u8` and `u16`, so these cover every frame the native decoder produced.
    trait Sample: Pixel + Sized {
        /// Appends one visible row of this plane to `stream`.
        fn write_row(stream: &mut Vec<u8>, row: &[Self]);

        /// Builds a pixel from a 16-bit test value, for the byte-equivalence
        /// tests.
        fn from_test(value: u16) -> Self;
    }

    impl Sample for u8 {
        fn write_row(stream: &mut Vec<u8>, row: &[u8]) {
            stream.extend_from_slice(row);
        }

        fn from_test(value: u16) -> Self {
            value as Self
        }
    }

    impl Sample for u16 {
        fn write_row(stream: &mut Vec<u8>, row: &[u16]) {
            if cfg!(target_endian = "little") {
                let bytes = unsafe {
                    // SAFETY: `row` is a live `[u16]`, so `size_of_val(row)` bytes
                    // are initialised and readable from `row.as_ptr()`, and `u8`
                    // has an alignment of 1, so the cast is properly aligned. The
                    // slice is only read and does not outlive `row`.
                    std::slice::from_raw_parts(
                        row.as_ptr().cast::<u8>(),
                        std::mem::size_of_val(row),
                    )
                };
                stream.extend_from_slice(bytes);
            } else {
                // y4m stores samples little-endian.
                stream.reserve(std::mem::size_of_val(row));
                for sample in row {
                    stream.extend_from_slice(&sample.to_le_bytes());
                }
            }
        }

        fn from_test(value: u16) -> Self {
            value
        }
    }

    /// The Frame-era writer, kept as the oracle the raw packer is proven
    /// against: appends a decoded frame's planes as y4m plane data in Y, U, V
    /// order, skipping padding. Bulk row copies rather than `Plane::byte_data`,
    /// which yields one byte at a time.
    fn write_y4m_frame<T: Sample>(stream: &mut Vec<u8>, frame: &Frame<T>) {
        let planes = [Some(&frame.y_plane), frame.u_plane.as_ref(), frame.v_plane.as_ref()];
        for plane in planes.into_iter().flatten() {
            for row in plane.rows() {
                T::write_row(stream, row);
            }
        }
    }

    /// Every visible pixel distinct, padding left at zero, so a wrong row
    /// order, dropped plane, or serialised padding shows up as a mismatch.
    fn sample_frame<T: Sample>(width: usize, height: usize, chroma: ChromaSubsampling) -> Frame<T> {
        let mut frame = av_decoders::v_frame::frame::FrameBuilder::new(
            width,
            height,
            chroma,
            if size_of::<T>() == 1 { 8 } else { 10 },
        )
        .luma_padding_left(8)
        .luma_padding_right(8)
        .luma_padding_top(8)
        .luma_padding_bottom(8)
        .build()
        .expect("valid frame geometry");

        let mut next = 1u16;
        for plane in [Some(&mut frame.y_plane), frame.u_plane.as_mut(), frame.v_plane.as_mut()]
            .into_iter()
            .flatten()
        {
            for row in plane.rows_mut() {
                for sample in row {
                    *sample = T::from_test(next);
                    next = next.wrapping_add(1);
                }
            }
        }
        frame
    }

    /// Bulk row copies are only safe if they match `Plane::byte_data` exactly.
    #[test]
    fn writer_matches_byte_data() {
        for chroma in [
            ChromaSubsampling::Yuv420,
            ChromaSubsampling::Yuv422,
            ChromaSubsampling::Yuv444,
            ChromaSubsampling::Monochrome,
        ] {
            let frame = sample_frame::<u8>(64, 32, chroma);
            let mut expected = Vec::new();
            expected.extend(frame.y_plane.byte_data());
            if let Some(plane) = &frame.u_plane {
                expected.extend(plane.byte_data());
            }
            if let Some(plane) = &frame.v_plane {
                expected.extend(plane.byte_data());
            }
            let mut actual = Vec::new();
            write_y4m_frame(&mut actual, &frame);
            assert_eq!(
                actual, expected,
                "8-bit writer differs from byte_data for {chroma:?}"
            );
        }
    }

    /// As above for high bit depth, where y4m requires little-endian samples
    /// regardless of host byte order.
    #[test]
    fn writer_matches_byte_data_high_bit_depth() {
        for chroma in [
            ChromaSubsampling::Yuv420,
            ChromaSubsampling::Yuv444,
            ChromaSubsampling::Monochrome,
        ] {
            let frame = sample_frame::<u16>(64, 32, chroma);
            let mut expected = Vec::new();
            expected.extend(frame.y_plane.byte_data());
            if let Some(plane) = &frame.u_plane {
                expected.extend(plane.byte_data());
            }
            if let Some(plane) = &frame.v_plane {
                expected.extend(plane.byte_data());
            }
            let mut actual = Vec::new();
            write_y4m_frame(&mut actual, &frame);
            assert_eq!(
                actual, expected,
                "16-bit writer differs from byte_data for {chroma:?}"
            );
            // Samples really did come out little-endian.
            assert_eq!(&actual[..2], &[1, 0]);
        }
    }

    /// Re-lays a frame's visible pixels into a stride-padded decoder buffer,
    /// the layout `ffms2_frame_at` reads. Stride deliberately differs from
    /// the `Frame`'s own, so padding mistakes change the packed bytes.
    fn decoder_layout<T: Pixel>(plane: &Plane<T>, stride: usize) -> (Vec<u8>, usize) {
        let bytes = size_of::<T>();
        let stride_bytes = stride * bytes;
        let mut data = vec![0u8; stride_bytes * plane.height()];
        for (row_index, row) in plane.rows().enumerate() {
            for (col, value) in row.iter().enumerate() {
                let value = Into::<u16>::into(*value);
                let at = row_index * stride_bytes + col * bytes;
                if bytes == 1 {
                    data[at] = value as u8;
                } else {
                    data[at..at + bytes].copy_from_slice(&value.to_le_bytes());
                }
            }
        }
        (data, stride_bytes)
    }

    fn compare_writers<T: Sample>(width: usize, height: usize, chroma: ChromaSubsampling) {
        let format = format!("{chroma:?} {width}x{height}");
        let frame = sample_frame::<T>(width, height, chroma);
        let mut expected = Vec::new();
        write_y4m_frame(&mut expected, &frame);

        let bytes = size_of::<T>();
        let bit_depth = if bytes == 1 { 8 } else { 10 };
        let (luma_data, luma_stride) = decoder_layout(&frame.y_plane, width + 6);
        let luma = RawPlane::new(
            &luma_data,
            luma_stride,
            frame.y_plane.width(),
            frame.y_plane.height(),
            bytes,
        )
        .unwrap_or_else(|| panic!("{format}: luma plane rejected"));
        let u_layout = frame.u_plane.as_ref().map(|u| decoder_layout(u, u.width() + 4));
        let v_layout = frame.v_plane.as_ref().map(|v| decoder_layout(v, v.width() + 4));
        let chroma_pair = match (
            frame.u_plane.as_ref(),
            frame.v_plane.as_ref(),
            &u_layout,
            &v_layout,
        ) {
            (Some(u), Some(v), Some((u_data, u_stride)), Some((v_data, v_stride))) => {
                let cb = RawPlane::new(u_data, *u_stride, u.width(), u.height(), bytes)
                    .unwrap_or_else(|| panic!("{format}: Cb plane rejected"));
                let cr = RawPlane::new(v_data, *v_stride, v.width(), v.height(), bytes)
                    .unwrap_or_else(|| panic!("{format}: Cr plane rejected"));
                Some((cb, cr))
            },
            _ => None,
        };

        let raw = RawFrame {
            luma,
            chroma: chroma_pair,
            bit_depth,
        };
        let mut actual = Vec::new();
        write_raw_frame(&mut actual, &raw);
        assert_eq!(actual, expected, "{format}");
    }

    /// The raw-plane writer (production `y4m_frames`) must emit exactly the
    /// bytes the `Frame`-based writer produced before the switch, across
    /// bit depths and chroma layouts including monochrome.
    #[test]
    fn raw_writer_matches_frame_writer() {
        compare_writers::<u8>(64, 32, ChromaSubsampling::Yuv420);
        compare_writers::<u8>(64, 32, ChromaSubsampling::Monochrome);
        compare_writers::<u16>(64, 32, ChromaSubsampling::Yuv444);
        compare_writers::<u16>(96, 48, ChromaSubsampling::Yuv422);
    }
}
