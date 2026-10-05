//! Native FFMS2 input: construction, format filtering, and y4m serialisation.

use std::{io::Write, path::Path};

use anyhow::{Context, Result};
use av_decoders::{
    Decoder,
    Ffms2Decoder,
    v_frame::{frame::Frame, pixel::Pixel},
};

use crate::{
    core::input::{
        Input,
        InputError,
        clip_info::ClipInfo,
        ffms2_filter::{Ffms2Filter, ffms2_pixel_format},
        pixel_format::PixelFormat,
    },
    ffmpeg::get_clip_info,
    models::input::{ImportMethod, Input as InputModel},
};

/// Context attached to native decode failures.
const CONTEXT: &str = "get y4m frame";

/// Precedes every frame's planes in a y4m stream.
const FRAME_HEADER: &[u8] = b"FRAME\n";

/// A pixel type that can be written to a y4m plane. `Pixel` is sealed to `u8`
/// and `u16`, so these cover every frame the native decoder produces.
pub(crate) trait Sample: Pixel + Sized {
    /// Appends one visible row of this plane to `stream`.
    fn write_row(stream: &mut Vec<u8>, row: &[Self]);

    /// Builds a pixel from a 16-bit test value, for the byte-equivalence tests.
    #[cfg(test)]
    fn from_test(value: u16) -> Self;
}

impl Sample for u8 {
    #[inline]
    fn write_row(stream: &mut Vec<u8>, row: &[u8]) {
        stream.extend_from_slice(row);
    }

    #[cfg(test)]
    #[inline]
    fn from_test(value: u16) -> Self {
        value as Self
    }
}

impl Sample for u16 {
    #[inline]
    fn write_row(stream: &mut Vec<u8>, row: &[u16]) {
        if cfg!(target_endian = "little") {
            let bytes = unsafe {
                // SAFETY: `row` is a live `[u16]`, so `size_of_val(row)` bytes
                // are initialised and readable from `row.as_ptr()`, and `u8`
                // has an alignment of 1, so the cast is properly aligned. The
                // slice is only read and does not outlive `row`.
                std::slice::from_raw_parts(row.as_ptr().cast::<u8>(), std::mem::size_of_val(row))
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

    #[cfg(test)]
    #[inline]
    fn from_test(value: u16) -> Self {
        value
    }
}

/// Appends a decoded frame's planes to `stream` as y4m plane data, in Y, U, V
/// order, skipping padding. Bulk row copies rather than `Plane::byte_data`,
/// which yields one byte at a time and dominates the cost of writing a frame.
#[inline]
fn write_y4m_frame<T: Sample>(stream: &mut Vec<u8>, frame: &Frame<T>) {
    let planes = [Some(&frame.y_plane), frame.u_plane.as_ref(), frame.v_plane.as_ref()];
    for plane in planes.into_iter().flatten() {
        for row in plane.rows() {
            T::write_row(stream, row);
        }
    }
}

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

/// Writes a single decoded frame to `stream` as a complete y4m frame.
pub fn y4m_frame(decoder: &mut Decoder, index: usize, stream: &mut Vec<u8>) -> Result<()> {
    stream.write_all(FRAME_HEADER)?;
    // `T` must match the decoded bit depth: `u8` for 8-bit, `u16` for higher.
    match decoder.get_video_details().bit_depth {
        8 => write_decoded::<u8>(decoder, index, stream),
        _ => write_decoded::<u16>(decoder, index, stream),
    }
}

/// Streams `frame_indices` as y4m frames through `emit`, in order. Each frame
/// is emitted before the next is decoded, so `emit` bounds resident frames.
pub fn y4m_frames<T: Sample>(
    decoder: &mut Decoder,
    frame_indices: &[usize],
    mut emit: impl FnMut(Vec<u8>) -> Result<()>,
) -> Result<()> {
    for index in frame_indices {
        let mut buffer = Vec::new();
        buffer.write_all(FRAME_HEADER)?;
        write_decoded::<T>(decoder, *index, &mut buffer)?;
        emit(buffer)?;
    }
    Ok(())
}

/// Decodes one frame and writes its planes.
fn write_decoded<T: Sample>(
    decoder: &mut Decoder,
    index: usize,
    stream: &mut Vec<u8>,
) -> Result<()> {
    let frame = decoder.get_video_frame::<T>(index).context(CONTEXT)?;
    write_y4m_frame(stream, &frame);
    Ok(())
}

#[cfg(test)]
mod tests {
    use av_decoders::v_frame::chroma::ChromaSubsampling;

    use super::*;

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
}
