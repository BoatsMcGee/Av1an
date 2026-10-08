//! YUV frame → 48×27 RGB tile, the resolution TransNetV2 was exported with.
//!
//! The same box filter and colour conversion serve both ways of reaching a
//! frame: [`frame_to_tile`] reads an av-decoders `Frame`, `raw_frame_to_tile`
//! reads the [`RawFrame`] the input's
//! [`FrameFeed`](crate::core::input::FrameFeed) handed over. Only the
//! per-sample access differs, so the two are compared against each other across
//! formats in the tests.

#[cfg(test)]
use std::num::NonZeroUsize;

#[cfg(test)]
use av_decoders::v_frame::{frame::Frame, pixel::Pixel, plane::Plane};

use crate::core::input::{RawFrame, RawPlane};

pub(crate) const WIDTH: usize = 48;
pub(crate) const HEIGHT: usize = 27;
pub(crate) const TILE_LEN: usize = WIDTH * HEIGHT * 3;

/// One plane's visible pixels, addressed by row and column. The two
/// implementations differ only in how a sample is decoded.
trait Samples {
    fn width(&self) -> usize;
    fn height(&self) -> usize;
    fn sample(&self, row: usize, col: usize) -> u32;
}

impl Samples for RawPlane<'_> {
    #[inline]
    fn width(&self) -> usize {
        RawPlane::width(self)
    }

    #[inline]
    fn height(&self) -> usize {
        RawPlane::height(self)
    }

    #[inline]
    fn sample(&self, row: usize, col: usize) -> u32 {
        RawPlane::sample(self, row, col)
    }
}

/// The visible rows of an av-decoders frame plane, the route the tests tile
/// through to check the raw reader.
#[cfg(test)]
struct PlaneRows<'a, T: Pixel> {
    rows:  Vec<&'a [T]>,
    width: usize,
}

#[cfg(test)]
impl<'a, T: Pixel> PlaneRows<'a, T> {
    fn new(plane: &'a Plane<T>) -> Self {
        Self {
            rows:  plane.rows().collect(),
            width: plane.width(),
        }
    }

    #[cfg(test)]
    fn from_rows(rows: Vec<&'a [T]>, width: usize) -> Self {
        Self {
            rows,
            width,
        }
    }
}

#[cfg(test)]
impl<T: Pixel> Samples for PlaneRows<'_, T> {
    #[inline]
    fn width(&self) -> usize {
        self.width
    }

    #[inline]
    fn height(&self) -> usize {
        self.rows.len()
    }

    #[inline]
    fn sample(&self, row: usize, col: usize) -> u32 {
        u32::from(Into::<u16>::into(self.rows[row][col]))
    }
}

/// Box-downscales `frame` to [`WIDTH`]×[`HEIGHT]` and converts limited-range
/// BT.709 Y'CbCr to 0–255 RGB. The av-decoders route, kept as the reference the
/// raw reader is checked against.
#[cfg(test)]
pub(crate) fn frame_to_tile<T: Pixel>(frame: &Frame<T>) -> Vec<f32> {
    let luma = PlaneRows::new(&frame.y_plane);
    let chroma = match (frame.u_plane.as_ref(), frame.v_plane.as_ref()) {
        (Some(u), Some(v)) => Some((PlaneRows::new(u), PlaneRows::new(v))),
        // Monochrome: neutral chroma.
        _ => None,
    };
    tile(
        &luma,
        chroma.as_ref().map(|(cb, cr)| (cb, cr)),
        NonZeroUsize::from(frame.bit_depth).get(),
    )
}

/// The same tile, built from a [`RawFrame`] the input's backend decoded in
/// place, without copying its planes into a `Frame` first.
pub(crate) fn raw_frame_to_tile(frame: &RawFrame<'_>) -> Vec<f32> {
    tile(
        &frame.luma,
        frame.chroma.as_ref().map(|(cb, cr)| (cb, cr)),
        frame.bit_depth,
    )
}

/// Box-filters every plane to the static output grid, then converts to RGB.
fn tile<S: Samples>(luma: &S, chroma: Option<(&S, &S)>, bit_depth: usize) -> Vec<f32> {
    let shift = bit_depth.saturating_sub(8);
    // Limited-range headroom scaled to the frame's bit depth.
    let y_offset = (16u32 << shift) as f32;
    let y_range = (219u32 << shift) as f32;
    let c_offset = (128u32 << shift) as f32;
    let c_range = (224u32 << shift) as f32;

    let luma = box_average(luma);
    let (cb, cr) = match chroma {
        Some((cb, cr)) => (box_average(cb), box_average(cr)),
        None => (vec![c_offset; WIDTH * HEIGHT], vec![
            c_offset;
            WIDTH * HEIGHT
        ]),
    };

    let mut tile = Vec::with_capacity(TILE_LEN);
    for index in 0..WIDTH * HEIGHT {
        let y = ((luma[index] - y_offset) / y_range).clamp(0.0, 1.0);
        let cb = normalize_chroma(cb[index], c_offset, c_range);
        let cr = normalize_chroma(cr[index], c_offset, c_range);
        let (r, g, b) = yuv_to_rgb(y, cb, cr);
        tile.push(r);
        tile.push(g);
        tile.push(b);
    }
    tile
}

/// Limited-range chroma normalized around neutral (0.0).
fn normalize_chroma(value: f32, offset: f32, range: f32) -> f32 {
    ((value - offset) / range).clamp(-0.5, 0.5)
}

/// Limited-range BT.709 Y'CbCr (normalized) to 0–255 RGB. Byte scale, not
/// 0–1: the exported graph divides by 255 itself, so 0–1 input would reach
/// the model as near-black and yield flat predictions.
fn yuv_to_rgb(y: f32, cb: f32, cr: f32) -> (f32, f32, f32) {
    let r = 1.5748f32.mul_add(cr, y);
    let g = (-0.468124f32).mul_add(cr, (-0.187324f32).mul_add(cb, y));
    let b = 1.8556f32.mul_add(cb, y);
    (
        r.clamp(0.0, 1.0) * 255.0,
        g.clamp(0.0, 1.0) * 255.0,
        b.clamp(0.0, 1.0) * 255.0,
    )
}

/// Averages `plane` down to [`WIDTH`]×[`HEIGHT`] with box filtering.
fn box_average<S: Samples>(plane: &S) -> Vec<f32> {
    let src_w = plane.width();
    let src_h = plane.height();
    let mut out = vec![0.0f32; WIDTH * HEIGHT];
    for out_y in 0..HEIGHT {
        let y0 = out_y * src_h / HEIGHT;
        let y1 = ((out_y + 1) * src_h / HEIGHT).max(y0 + 1).min(src_h);
        for out_x in 0..WIDTH {
            let x0 = out_x * src_w / WIDTH;
            let x1 = ((out_x + 1) * src_w / WIDTH).max(x0 + 1).min(src_w);
            let mut sum = 0u64;
            let mut count = 0u64;
            for row in y0..y1 {
                for col in x0..x1 {
                    sum += u64::from(plane.sample(row, col));
                    count += 1;
                }
            }
            out[out_y * WIDTH + out_x] = sum as f32 / count.max(1) as f32;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use av_decoders::v_frame::{chroma::ChromaSubsampling, frame::FrameBuilder};

    use super::*;

    /// A pixel type the tests can synthesise; `Pixel` itself only reads.
    trait TestSample: Pixel {
        fn from_u16(value: u16) -> Self;
    }

    impl TestSample for u8 {
        #[inline]
        fn from_u16(value: u16) -> Self {
            value as Self
        }
    }

    impl TestSample for u16 {
        #[inline]
        fn from_u16(value: u16) -> Self {
            value
        }
    }

    /// Re-lays a plane's visible pixels out as a decoder would hand them over:
    /// no leading padding, `stride` samples per row, little-endian samples.
    /// Deliberately a different stride than the `Frame`'s own, so a wrong
    /// stride or padding offset in [`RawPlane`] shows up as a mismatch.
    fn decoder_bytes<T: TestSample>(plane: &Plane<T>, stride: usize) -> (Vec<u8>, usize) {
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

    /// Fills a frame with a value that depends on plane, row and column, so a
    /// swapped plane, shifted row or dropped column changes the tile.
    fn fill<T: TestSample>(frame: &mut Frame<T>, seed: u16, ceiling: u16) {
        for (plane_index, plane) in std::iter::once(&mut frame.y_plane)
            .chain(frame.u_plane.iter_mut())
            .chain(frame.v_plane.iter_mut())
            .enumerate()
        {
            let plane_index = u16::try_from(plane_index).unwrap_or(0);
            for (row, row_data) in plane.rows_mut().enumerate() {
                let row = u16::try_from(row).unwrap_or(0);
                for (col, value) in row_data.iter_mut().enumerate() {
                    let col = u16::try_from(col).unwrap_or(0);
                    *value = T::from_u16((seed + plane_index * 97 + row * 7 + col) % ceiling);
                }
            }
        }
    }

    /// Every format the detector can be handed, covering each bit depth and
    /// chroma layout the backends produce.
    const FORMATS: &[(usize, usize, ChromaSubsampling, u8)] = &[
        (96, 54, ChromaSubsampling::Yuv420, 8),
        (96, 54, ChromaSubsampling::Yuv420, 10),
        (96, 54, ChromaSubsampling::Yuv420, 12),
        (96, 54, ChromaSubsampling::Yuv422, 10),
        (96, 54, ChromaSubsampling::Yuv444, 8),
        (96, 54, ChromaSubsampling::Yuv444, 16),
        (96, 54, ChromaSubsampling::Monochrome, 8),
        (96, 54, ChromaSubsampling::Monochrome, 10),
    ];

    /// The raw reader must agree with `frame_to_tile` byte for byte: both tile
    /// the same pixels reached through different strides, padding and sample
    /// decoding.
    #[test]
    fn raw_tiles_match_frame_to_tile_across_formats() {
        for &(width, height, subsampling, bit_depth) in FORMATS {
            if bit_depth == 8 {
                compare::<u8>(width, height, subsampling, bit_depth);
            } else {
                compare::<u16>(width, height, subsampling, bit_depth);
            }
        }
    }

    fn compare<T: TestSample>(
        width: usize,
        height: usize,
        subsampling: ChromaSubsampling,
        bit_depth: u8,
    ) {
        let format = format!("{width}x{height} {subsampling:?} {bit_depth}-bit");
        let mut frame = FrameBuilder::new(width, height, subsampling, bit_depth)
            .luma_padding_left(8)
            .luma_padding_right(8)
            .luma_padding_top(4)
            .luma_padding_bottom(4)
            .build::<T>()
            .unwrap_or_else(|error| panic!("{format}: {error}"));
        let ceiling = if size_of::<T>() == 1 { 256 } else { 1024 };
        fill(&mut frame, 17, ceiling);

        let expected = frame_to_tile(&frame);

        // The geometry production derives from the clip's format must be the
        // geometry `FrameBuilder` actually built.
        let chroma_dims = subsampling.subsample_ratio().map(|(x, y)| {
            (
                width / NonZeroUsize::from(x).get(),
                height / NonZeroUsize::from(y).get(),
            )
        });
        for plane in [&frame.u_plane, &frame.v_plane].into_iter().flatten() {
            assert_eq!(
                Some((plane.width(), plane.height())),
                chroma_dims,
                "{format}: chroma geometry"
            );
        }

        let (luma_bytes, luma_stride) = decoder_bytes(&frame.y_plane, width + 5);
        let luma = RawPlane::new(&luma_bytes, luma_stride, width, height, size_of::<T>())
            .unwrap_or_else(|| panic!("{format}: luma plane rejected"));

        let chroma_bytes = match (&frame.u_plane, &frame.v_plane) {
            (Some(u), Some(v)) => Some((
                decoder_bytes(u, u.width() + 7),
                decoder_bytes(v, v.width() + 7),
            )),
            _ => None,
        };
        let chroma = chroma_bytes.as_ref().map(|((u_bytes, u_stride), (v_bytes, v_stride))| {
            let u = frame.u_plane.as_ref().expect("Cb plane");
            let v = frame.v_plane.as_ref().expect("Cr plane");
            (
                RawPlane::new(u_bytes, *u_stride, u.width(), u.height(), size_of::<T>())
                    .unwrap_or_else(|| panic!("{format}: Cb plane rejected")),
                RawPlane::new(v_bytes, *v_stride, v.width(), v.height(), size_of::<T>())
                    .unwrap_or_else(|| panic!("{format}: Cr plane rejected")),
            )
        });

        let actual = raw_frame_to_tile(&RawFrame {
            luma,
            chroma,
            bit_depth: usize::from(bit_depth),
        });
        assert_eq!(actual, expected, "{format}");
    }

    #[test]
    fn box_average_preserves_constant_planes() {
        let rows: Vec<Vec<u8>> = vec![vec![77u8; 96]; 54];
        let slices: Vec<&[u8]> = rows.iter().map(Vec::as_slice).collect();
        let out = box_average(&PlaneRows::from_rows(slices, 96));
        assert_eq!(out.len(), WIDTH * HEIGHT);
        assert!(out.iter().all(|value| (*value - 77.0).abs() < f32::EPSILON));
    }

    #[test]
    fn box_average_downscales_by_two() {
        // 2×2 blocks average to one output pixel; half the plane is 0, half 128.
        let mut data = vec![0u8; 96 * 54];
        for (index, value) in data.iter_mut().enumerate() {
            let x = index % 96;
            *value = if x >= 48 { 128 } else { 0 };
        }
        let rows: Vec<&[u8]> = data.chunks(96).collect();
        let out = box_average(&PlaneRows::from_rows(rows, 96));
        for out_y in 0..HEIGHT {
            for out_x in 0..WIDTH {
                let expected = if out_x >= 24 { 128.0 } else { 0.0 };
                assert_eq!(out[out_y * WIDTH + out_x], expected, "at {out_x}x{out_y}");
            }
        }
    }

    #[test]
    fn white_limited_range_maps_to_white() {
        let (r, g, b) = yuv_to_rgb(1.0, 0.0, 0.0);
        assert_eq!((r, g, b), (255.0, 255.0, 255.0));
    }

    #[test]
    fn neutral_chroma_maps_to_gray() {
        // Byte scale: the model's in-graph Div by 255 must undo this.
        let (r, g, b) = yuv_to_rgb(0.5, 0.0, 0.0);
        assert_eq!((r, g, b), (127.5, 127.5, 127.5));
    }

    #[test]
    fn neutral_chroma_value_normalizes_to_zero() {
        // Regression: an extra -0.5 here once clamped R and B to zero for
        // every frame, flattening all predictions.
        let neutral = normalize_chroma(128.0, 128.0, 224.0);
        assert_eq!(neutral, 0.0);
        let (r, g, b) = yuv_to_rgb(0.5, neutral, neutral);
        assert_eq!((r, g, b), (127.5, 127.5, 127.5));
        // The full chroma range stays inside ±0.5.
        assert_eq!(normalize_chroma(16.0, 128.0, 224.0), -0.5);
        assert_eq!(normalize_chroma(240.0, 128.0, 224.0), 0.5);
    }
}
