//! The decode/probe driver shared by every native metric.
//!
//! Comparing an encode against its source means pairing two clips frame by
//! frame and handing each pair to a metric. Everything that is the same for
//! every metric lives here: choosing a reader per side, seeking or advancing to
//! each selected frame, checking the two clips agree on geometry, and calling
//! back with the planes of one pair.
//!
//! The plane descriptor is declared locally rather than imported from a metric
//! crate. Keeping [`PlaneSource`] here is what lets this module stay
//! metric-agnostic: a metric that needs a different type adapts the set inside
//! its own `submit` callback instead of the driver depending on that crate.

use std::{
    mem::size_of,
    sync::atomic::{AtomicBool, Ordering},
};

use anyhow::{Context as _, Result, bail};
use itertools::Itertools;
use vapoursynth::frame::FrameRef;

/// Returned when a pass is abandoned because the user cancelled it.
///
/// A unit struct rather than a formatted error, since the enclosing sequence
/// already knows what cancellation means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cancelled;

impl std::fmt::Display for Cancelled {
    #[inline]
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the quality check was cancelled")
    }
}

impl std::error::Error for Cancelled {
}

/// One decoded plane, described without copying it.
///
/// The fields mirror what a metric library needs to read a plane: dimensions in
/// samples, the row spacing, the offset of the first visible sample, and the
/// pointer to the allocation itself.
///
/// A `PlaneSource` aliases the frame it came from and so is only valid while
/// that frame is alive. Every producer here hands the set to a callback that
/// consumes it before the frame is dropped.
pub struct PlaneSource {
    /// Width in pixels (samples per row, not bytes).
    pub width:            usize,
    /// Height in pixels.
    pub height:           usize,
    /// Samples from one row to the next. May be negative for bottom-up images.
    pub stride:           isize,
    /// Offset of the first visible sample from `data`.
    pub data_origin:      usize,
    /// Pointer to the start of the underlying allocation.
    pub data:             *const u8,
    /// Bytes per sample.
    pub bytes_per_sample: usize,
}

/// The three planes of a planar YUV frame, in Y, U, V order.
pub type PlaneSet = [PlaneSource; 3];

/// Which index space the encoded output is addressed in.
///
/// The call sites pass encodes with different timelines, so conflating them
/// pairs the wrong frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputIndexing {
    /// The output holds only the selected frames, compacted.
    ///
    /// This is a *probe* encode: `ParallelEncoder` trims each task's input to
    /// `frame_indices`, so the concat contains exactly the probed frames,
    /// re-indexed from zero. Output frame *k* encodes source frame
    /// `selected[k]`.
    ///
    /// This matches what the VapourSynth path does for target-quality, which
    /// trims the reference to the selected indices and `Splice`s it while
    /// leaving the distorted node as a bare `Source` — both sides end up
    /// compacted identically.
    Compacted,

    /// The output covers the whole clip and is addressed by source frame
    /// number.
    ///
    /// This is a completed encode being re-scored, so a selected frame sits at
    /// the same index in both clips. It matches what the VapourSynth path does
    /// for quality-check, which trims *both* nodes by the same global indices.
    Aligned,
}

/// The encoded-output frame that pairs with the source frame at `position`
/// within the selection.
///
/// The reference is always the full source clip, so its index is
/// `selected[position]`. The distorted output is addressed per `indexing`, and
/// conflating the two spaces pairs unrelated frames — or, for a compacted
/// output, runs past the end of the clip entirely.
#[must_use]
#[inline]
pub fn output_index(position: usize, source_index: usize, indexing: OutputIndexing) -> usize {
    match indexing {
        OutputIndexing::Compacted => position,
        OutputIndexing::Aligned => source_index,
    }
}

/// One side of the comparison, read by the cheapest method available for it.
///
/// Each side picks its own reader, independently. In both call sites the
/// reference is a VapourSynth script while the distorted side is an encoded
/// file opened through FFMS2, so a single "both must be VapourSynth" fast path
/// would never be taken.
pub enum Side<'decoder> {
    /// Read straight off a VapourSynth node.
    ///
    /// `av-decoders` would copy every plane into a `v_frame::Frame` first,
    /// which for 1080p is ~5.9 MB per frame per side. The node's planes are
    /// already strided buffers, so pointing at them skips that copy
    /// entirely.
    ///
    /// The node is built once and held for the whole run. `get_output` keeps
    /// the decoder borrowed for the node's lifetime, which is fine because
    /// a VapourSynth node seeks per frame inside [`Side::with_planes`], so
    /// the decoder itself is never touched again. Holding one also avoids
    /// re-running any registered `ModifyNode` on every frame.
    Node(vapoursynth::node::Node<'decoder>),

    /// Read through `av-decoders`: the only option for FFMS2, FFmpeg and y4m.
    ///
    /// `next` is the frame the following read will return: `advance` sets it
    /// once the decoder is positioned, and `with_planes` moves it past the
    /// frame it just read. Backends that cannot seek rely on it to move forward
    /// by decoding and discarding the frames in between.
    Native {
        decoder:   &'decoder mut av_decoders::Decoder,
        bit_depth: usize,
        next:      usize,
        seekable:  bool,
    },
}

impl<'decoder> Side<'decoder> {
    /// Open `decoder` as a scoring side.
    ///
    /// # Errors
    ///
    /// Returns an error when the decoder is VapourSynth but its node cannot be
    /// built. Building it here proves the graph works, so a broken script is
    /// reported before any frame is decoded.
    #[inline]
    pub fn open(decoder: &'decoder mut av_decoders::Decoder, bit_depth: usize) -> Result<Self> {
        // Probed first, in a borrow that ends at the end of the call. Asking
        // `open` for the node directly would not work: `Node<'decoder>` in the
        // return type forces the decoder's borrow to outlive the function, which
        // overlaps the `&mut` the native path needs.
        if is_vapoursynth(decoder) {
            let impl_ = decoder
                .get_vapoursynth_impl()
                .expect("the decoder was just probed as VapourSynth");
            let node = impl_.get_output(impl_.get_output_index(), impl_.get_node_modifier())?;
            return Ok(Self::Node(node));
        }

        // Seeking to frame 0 is a no-op for a backend that supports seeking, and
        // an error for one that does not.
        let seekable = decoder
            .seek_to_frame(0)
            .or_else(|error| {
                // `EndOfFile` for an empty clip still means seeking is
                // implemented.
                matches!(error, av_decoders::DecoderError::EndOfFile).then_some(()).ok_or(error)
            })
            .is_ok();

        Ok(Self::Native {
            seekable,
            decoder,
            bit_depth,
            next: 0,
        })
    }

    /// Whether this side can reach an arbitrary frame without decoding the ones
    /// in between.
    #[inline]
    #[must_use]
    pub fn is_seekable(&self) -> bool {
        !matches!(self, Self::Native {
            seekable: false,
            ..
        })
    }

    /// Point at the planes of frame `index` and hand them to `submit`.
    ///
    /// The frame cannot outlive this call -- a `v_frame::Frame` is only valid
    /// until the decoder's next read, and a VapourSynth `FrameRef` borrows the
    /// node -- so the planes go straight to a callback instead of being
    /// returned. `submit` is what copies them into the metric.
    ///
    /// # Errors
    ///
    /// Returns an error if the frame cannot be decoded or has no scorable
    /// plane layout.
    #[inline]
    pub fn with_planes<T>(
        &mut self,
        index: usize,
        submit: impl FnOnce(PlaneSet) -> Result<T>,
    ) -> Result<T> {
        match self {
            Self::Node(node) => {
                let frame = node
                    .get_frame(index)
                    .map_err(|error| anyhow::anyhow!("failed to read frame {index}: {error}"))?;
                submit(vapoursynth_planes(&frame)?)
            },
            Self::Native {
                decoder,
                bit_depth,
                next,
                ..
            } => {
                let read = with_native_planes(decoder, *bit_depth, index, submit);
                // A successful read consumed frame `index`, so the tracked
                // position moves past it. A failure leaves the position
                // untouched; the pass stops either way.
                if read.is_ok() {
                    *next = index + 1;
                }
                read
            },
        }
    }

    /// Move to `target`, so the next read returns it.
    ///
    /// A VapourSynth node is random access, so this is a no-op for it.
    ///
    /// # Errors
    ///
    /// Returns an error if `target` cannot be reached, either because seeking
    /// failed or because a backend that cannot seek was asked to go backwards.
    #[inline]
    pub fn advance(&mut self, target: usize) -> Result<()> {
        let Self::Native {
            decoder,
            bit_depth,
            next,
            seekable,
        } = self
        else {
            return Ok(());
        };

        if *next == target {
            return Ok(());
        }

        if *seekable {
            decoder
                .seek_to_frame(target)
                .map_err(|error| anyhow::anyhow!("failed to seek to frame {target}: {error}"))?;
        } else if target > *next {
            // Decode and discard the frames in between. Skipped frames are still
            // decoded, just never scored, so the planes go to a no-op.
            for skipped in *next..target {
                with_native_planes(decoder, *bit_depth, skipped, |_| Ok(()))?;
            }
        } else {
            bail!("cannot go back to frame {target} from {next} with a decoder that cannot seek");
        }

        // The seek or the discard run has positioned the decoder at `target`,
        // which is what the following read returns; `with_planes` moves past it
        // once the frame is read.
        *next = target;
        Ok(())
    }
}

/// Whether this decoder reads through a VapourSynth node.
#[inline]
fn is_vapoursynth(decoder: &mut av_decoders::Decoder) -> bool {
    decoder.get_vapoursynth_impl().is_some()
}

/// Decode one frame through `av-decoders` and hand its planes to `submit`.
///
/// The pixel type has to be chosen before the read, so the dispatch on bit
/// depth happens here rather than at every call site.
///
/// This is callback-shaped rather than returning the [`PlaneSet`] because the
/// set aliases the decoded frame, which is dropped when this returns. Handing
/// the planes to `submit` inside the call is what keeps that from being a
/// use-after-free.
///
/// # Errors
///
/// Returns an error if the frame cannot be decoded, or whatever `submit`
/// returns.
#[inline]
fn with_native_planes<T>(
    decoder: &mut av_decoders::Decoder,
    bit_depth: usize,
    index: usize,
    submit: impl FnOnce(PlaneSet) -> Result<T>,
) -> Result<T> {
    macro_rules! read {
        ($pixel:ty) => {{
            let frame = decoder
                .read_video_frame::<$pixel>()
                .map_err(|error| anyhow::anyhow!("failed to decode frame {index}: {error}"))?;
            submit(native_planes(&frame)?)
        }};
    }

    // `av-decoders` requires the pixel type to match the video's bit depth.
    if bit_depth > 8 { read!(u16) } else { read!(u8) }
}

/// Describe a `v_frame` frame as its three scorable planes.
///
/// The returned sources alias `frame`, so `frame` must outlive them;
/// [`with_native_planes`] enforces that by never returning the set.
///
/// # Errors
///
/// Returns an error when the frame has no chroma planes, which is the case for
/// monochrome content no planar metric can score.
#[inline]
fn native_planes<T>(frame: &av_decoders::v_frame::frame::Frame<T>) -> Result<PlaneSet>
where
    T: av_decoders::v_frame::pixel::Pixel,
{
    let bytes_per_sample = size_of::<T>();
    let (Some(u_plane), Some(v_plane)) = (&frame.u_plane, &frame.v_plane) else {
        bail!("frame has no chroma planes; a metric requires 4:2:0, 4:2:2 or 4:4:4 content");
    };

    Ok([&frame.y_plane, u_plane, v_plane].map(|plane| {
        let geometry = plane.geometry();

        // SAFETY: `Plane::data` is the plane's own allocation, and `geometry`'s
        // stride and origin describe exactly that allocation. The sources are
        // consumed within the `submit` call that receives them, while `frame` is
        // still alive.
        //
        // `v_frame` reports stride and origin in samples, so both are scaled to
        // bytes here. The stride is widened rather than cast, because the field
        // is signed to allow bottom-up planes from other frame sources.
        PlaneSource {
            width: geometry.width(),
            height: geometry.height(),
            stride: geometry.stride() as isize * bytes_per_sample as isize,
            data_origin: geometry.data_origin() * bytes_per_sample,
            data: plane.data().as_ptr().cast::<u8>(),
            bytes_per_sample,
        }
    }))
}

/// Describe a VapourSynth frame as its three scorable planes.
///
/// The returned sources point into `frame`'s plane allocations, so `frame` must
/// outlive their use. Every caller here passes them straight to `submit`, which
/// copies them before the frame is dropped.
///
/// # Errors
///
/// Returns an error for a layout the metrics here cannot score. They are
/// defined on planar YUV, so RGB or a non-planar layout is rejected rather
/// than reinterpreted, which would silently compare the wrong channels.
#[inline]
fn vapoursynth_planes(frame: &FrameRef<'_>) -> Result<PlaneSet> {
    use vapoursynth::format::ColorFamily;

    let format = frame.format();
    if format.color_family() != ColorFamily::YUV || format.plane_count() != 3 {
        bail!(
            "a metric requires planar YUV, but the frame is in the {:?} family",
            format.color_family()
        );
    }

    let bytes_per_sample = usize::from(format.bytes_per_sample());

    Ok(std::array::from_fn(|index| {
        // `data_ptr` and `stride` describe this frame's own plane allocations,
        // which the caller keeps alive across `submit`. VapourSynth already
        // reports both in bytes, so they pass through unscaled.
        PlaneSource {
            width: frame.width(index),
            height: frame.height(index),
            stride: frame.stride(index) as isize,
            data_origin: 0,
            data: frame.data_ptr(index),
            bytes_per_sample,
        }
    }))
}

/// Read every frame in `selected` from both sides and hand each pair to
/// `submit`, returning one value per selected frame.
///
/// This owns the whole loop so that every metric seeks, validates and reads
/// identically; a metric only supplies `submit`, which receives the position in
/// the selection plus both plane sets.
///
/// `validate` runs once, after both sides are opened and before any frame is
/// decoded, so a metric can reject the clip pair before spending any work on
/// it. [`check_geometry`] is the shared implementation.
///
/// # Index spaces
///
/// The two decoders may use different index spaces, which is why `indexing` is
/// passed explicitly rather than inferred. See [`output_index`].
///
/// # Cost
///
/// When a side can seek, each selected frame is reached directly, so frames
/// outside the selection are never decoded at all — the same saving the plugin
/// metrics get by trimming frames out of the graph. Backends that cannot seek
/// read forward and discard instead, which avoids *scoring* skipped frames but
/// still decodes them.
///
/// `selected` need not be sorted; frames are visited in the order given.
///
/// `cancelled` is polled once per frame, so a long pass stops promptly when the
/// user aborts instead of running to completion.
///
/// # Errors
///
/// Returns an error when `validate` rejects the two clips, when a selected
/// frame cannot be reached or decoded, when `cancelled` is set, or with
/// whatever `submit` returns.
#[inline]
pub fn probe_selected_frames<T, R>(
    reference: &mut av_decoders::Decoder,
    distorted: &mut av_decoders::Decoder,
    selected: &[usize],
    indexing: OutputIndexing,
    cancelled: Option<&AtomicBool>,
    validate: impl FnOnce(av_decoders::VideoDetails, av_decoders::VideoDetails) -> Result<()>,
    mut submit: impl FnMut(usize, PlaneSet, PlaneSet) -> Result<R>,
) -> Result<Vec<R>> {
    if selected.is_empty() {
        return Ok(Vec::new());
    }

    let reference_details = *reference.get_video_details();
    let distorted_details = *distorted.get_video_details();
    validate(reference_details, distorted_details)?;

    let bit_depth = reference_details.bit_depth;

    // Both sides are opened before any frame is decoded, so a broken graph is
    // reported up front rather than partway through a long pass.
    let mut reference_side = Side::open(reference, bit_depth)?;
    let mut distorted_side = Side::open(distorted, bit_depth)?;

    check_seekable_selection(&reference_side, &distorted_side, selected, indexing)?;

    let mut results = Vec::with_capacity(selected.len());

    // Pairs are read as they are decoded, so peak memory is the metric's own
    // picture pool plus one pair regardless of clip length; buffering the whole
    // selection instead would scale without bound.
    for (position, &source_index) in selected.iter().enumerate() {
        if cancelled.is_some_and(|cancelled| cancelled.load(Ordering::Relaxed)) {
            return Err(Cancelled.into());
        }

        let output_index = output_index(position, source_index, indexing);

        reference_side.advance(source_index)?;
        distorted_side.advance(output_index).with_context(|| {
            // A compacted output holds exactly the selection; an aligned one
            // is the full-length encode.
            let output_frames = match indexing {
                OutputIndexing::Compacted => Some(selected.len()),
                OutputIndexing::Aligned => distorted_details.total_frames,
            };
            let output = output_frames.map_or_else(
                || String::from("the encoded output"),
                |frames| format!("the {frames} frame encoded output"),
            );
            format!("failed to reach frame {output_index} of {output}")
        })?;

        // Both sides are read inside nested calls so each frame's planes stay
        // valid until the metric has copied them. The decoders are never borrowed
        // in the same expression, because a VapourSynth node borrows its
        // decoder.
        results.push(
            distorted_side.with_planes(output_index, |distorted_planes| {
                reference_side.with_planes(source_index, |reference_planes| {
                    submit(position, reference_planes, distorted_planes)
                })
            })?,
        );
    }

    Ok(results)
}

/// Reject a selection that a decoder without seeking cannot serve.
///
/// A side that cannot seek reads forward and discards, so it serves any
/// ascending selection but fails outright on a step backwards. Checking this up
/// front turns a mid-pass decode failure into an error beside the
/// configuration that caused it.
///
/// # Errors
///
/// Returns an error if a non-seekable side would have to read a frame before
/// the one it last read.
#[inline]
fn check_seekable_selection(
    reference: &Side<'_>,
    distorted: &Side<'_>,
    selected: &[usize],
    indexing: OutputIndexing,
) -> Result<()> {
    // Each side is addressed in its own index space. A compacted output is read by
    // position, so it ascends by construction; only an aligned output follows
    // the reference into the source frame numbers and can step back.
    let backwards = (!reference.is_seekable() && steps_backwards(selected.iter().copied()))
        || (!distorted.is_seekable()
            && indexing == OutputIndexing::Aligned
            && steps_backwards(selected.iter().copied()));

    if backwards {
        bail!(
            "cannot score this selection with a decoder that cannot seek: it steps backwards, and \
             a decoder without seeking can only read forward. Use a seekable decoder, or select \
             frames in ascending order."
        );
    }

    Ok(())
}

/// Whether `indices` ever decrease.
///
/// This is the condition [`Side::advance`] enforces: it decodes forward and
/// discards, so a step backwards is the one thing it cannot serve.
#[inline]
fn steps_backwards(indices: impl IntoIterator<Item = usize>) -> bool {
    indices
        .into_iter()
        // The window type is spelled out because `indices` is generic here, so
        // there is no concrete iterator for the item type to be inferred from.
        .tuple_windows::<(usize, usize)>()
        .any(|(previous, current)| current < previous)
}

/// Reject pairs whose geometry or bit depth disagree.
///
/// Every metric defined on a full-frame comparison needs both clips to agree on
/// resolution and bit depth, so this is offered as the shared `validate` step.
///
/// # Errors
///
/// Returns an error if the two clips differ in resolution or bit depth.
#[inline]
pub fn check_geometry(
    reference: av_decoders::VideoDetails,
    distorted: av_decoders::VideoDetails,
) -> Result<()> {
    if reference.width != distorted.width || reference.height != distorted.height {
        bail!(
            "reference is {}x{} but the encoded output is {}x{}",
            reference.width,
            reference.height,
            distorted.width,
            distorted.height
        );
    }

    if reference.bit_depth != distorted.bit_depth {
        bail!(
            "reference is {}-bit but the encoded output is {}-bit",
            reference.bit_depth,
            distorted.bit_depth
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A compacted output holds only the selected frames, so it is read by
    /// position within the selection while the reference is read by source
    /// frame number.
    #[test]
    fn the_compacted_output_is_read_by_position_and_the_reference_by_source_frame() {
        // A `Subset{Middle, 11}` probe of a scene starting at frame 500.
        let selected: Vec<usize> = (500..511).collect();

        for (position, &source_index) in selected.iter().enumerate() {
            assert_eq!(
                output_index(position, source_index, OutputIndexing::Compacted),
                position,
                "the compacted output must be read from the matching position"
            );
        }
    }

    #[test]
    fn a_sparse_selection_never_seeks_the_compacted_output_past_its_own_length() {
        // A compacted output holds exactly `selected.len()` frames, so every
        // output index must be below that. Seeking by source index would exceed
        // it and fail outright.
        let selected: Vec<usize> = (500..511).collect();

        for (position, &source_index) in selected.iter().enumerate() {
            let output = output_index(position, source_index, OutputIndexing::Compacted);
            assert!(
                output < selected.len(),
                "output index {output} is out of range for a {}-frame selection",
                selected.len()
            );
        }
    }

    #[test]
    fn an_unsorted_selection_still_never_seeks_the_compacted_output_out_of_range() {
        let selected = [7usize, 3, 900, 12];

        for (position, &source_index) in selected.iter().enumerate() {
            assert_eq!(
                output_index(position, source_index, OutputIndexing::Compacted),
                position
            );
        }
    }

    /// A completed encode is full length, so both sides are addressed by the
    /// same index. This is what the quality-check call site passes.
    #[test]
    fn an_aligned_output_is_addressed_by_source_frame() {
        let selected: Vec<usize> = (500..511).collect();

        for (position, &source_index) in selected.iter().enumerate() {
            assert_eq!(
                output_index(position, source_index, OutputIndexing::Aligned),
                source_index,
                "an aligned output uses the same index as the source"
            );
        }
    }

    /// The two index spaces coincide only for a contiguous selection starting
    /// at zero, which is exactly the `Whole` strategy.
    #[test]
    fn both_indexings_agree_only_for_a_whole_selection() {
        let whole: Vec<usize> = (0..32).collect();
        for (position, &source_index) in whole.iter().enumerate() {
            assert_eq!(
                output_index(position, source_index, OutputIndexing::Compacted),
                output_index(position, source_index, OutputIndexing::Aligned),
                "a Whole selection is the same in either index space"
            );
        }

        let sparse: Vec<usize> = (10..40).step_by(10).collect();
        for (position, &source_index) in sparse.iter().enumerate() {
            assert_ne!(
                output_index(position, source_index, OutputIndexing::Compacted),
                output_index(position, source_index, OutputIndexing::Aligned),
                "a sparse selection must not be read the same way in both index spaces"
            );
        }
    }

    #[test]
    fn a_single_frame_selection_reads_the_first_frame_of_each_side() {
        // A compacted output holds one frame, so it is position zero.
        assert_eq!(output_index(0, 42, OutputIndexing::Compacted), 0);
        // An aligned output sits at the source index instead.
        assert_eq!(output_index(0, 42, OutputIndexing::Aligned), 42);
    }

    /// A side that cannot seek reads forward, so an ascending selection works
    /// from any starting frame — only a step backwards is impossible.
    /// Checking this up front turns a mid-pass failure into an error beside
    /// the configuration.
    #[test]
    fn a_non_seekable_side_rejects_a_selection_that_steps_backwards() {
        // An ascending run from a nonzero start is servable: the frames before
        // it are decoded and discarded, which is exactly what `advance` does.
        assert!(!steps_backwards([500usize, 501, 502].iter().copied()));
        // A single frame has no pair to step back between.
        assert!(!steps_backwards(std::iter::once(7usize)));

        // A step backwards cannot be served, whichever side asks for it.
        for selection in [&[10usize, 3, 12][..], &[5usize, 4][..]] {
            assert!(
                steps_backwards(selection.iter().copied()),
                "the unsorted selection {selection:?} must be rejected"
            );
        }
    }

    #[test]
    fn a_non_seekable_side_tracks_the_next_frame_across_reads() {
        let file = tempfile::Builder::new()
            .suffix(".y4m")
            .tempfile()
            .expect("create a y4m fixture");
        let path = file.into_temp_path();

        // Three 4x4 frames with distinct luma: y4m is a header plus `FRAME`
        // and planar Y, U, V per frame.
        let mut bytes = b"YUV4MPEG2 W4 H4 F24:1 Ip A1:1 C420jpeg\n".to_vec();
        for luma in [10u8, 110, 210] {
            bytes.extend_from_slice(b"FRAME\n");
            bytes.extend_from_slice(&[luma; 16]);
            bytes.extend_from_slice(&[128; 4]);
            bytes.extend_from_slice(&[128; 4]);
        }
        std::fs::write(&path, &bytes).expect("write the y4m fixture");

        {
            let mut decoder = av_decoders::Decoder::from_file(&path).expect("open the y4m fixture");
            let mut side = Side::open(&mut decoder, 8).expect("open the fixture as a scoring side");
            assert!(!side.is_seekable(), "y4m must be detected as non-seekable");

            for target in 0..3usize {
                side.advance(target).expect("advance sequentially");
                side.with_planes(target, |_| Ok(())).expect("read the frame");

                let Side::Native {
                    next,
                    seekable,
                    ..
                } = &side
                else {
                    panic!("the fixture must open as a native side");
                };
                assert!(!*seekable, "y4m has no seek support");
                assert_eq!(
                    *next,
                    target + 1,
                    "the tracked position must follow the frame just read"
                );
            }
        }

        path.close().expect("remove the y4m fixture");
    }
}
