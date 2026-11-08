//! Planar YUV to interleaved RGB, which fmetrics requires.
//!
//! [`FmetricsImg`] takes one packed, interleaved buffer with a single stride,
//! while decoders hand out three planar planes.
//!
//! This conversion is part of the measurement rather than a convenience: a
//! wrong matrix or range silently changes every score. The tests check
//! known-value behaviour rather than only that the conversion runs.
//!
//! 8-bit input becomes `RGB_UINT8`; 10- and 12-bit are up-converted to
//! `RGB_UINT16` rather than truncated. Range and matrix come from [`ColorInfo`]
//! rather than a default.

use crate::{
    config::{ColorInfo, SampleRange, YuvMatrix},
    error::FmetricsError,
    ffi::{FmetricsColorspace, FmetricsImg},
};

impl RgbSource for RgbImage {
    #[inline]
    fn width(&self) -> usize {
        self.width
    }

    #[inline]
    fn height(&self) -> usize {
        self.height
    }

    #[inline]
    fn stride(&self) -> usize {
        self.stride()
    }

    #[inline]
    fn as_bytes(&self) -> &[u8] {
        &self.data
    }

    #[inline]
    fn is_wide(&self) -> bool {
        self.wide
    }

    #[inline]
    fn as_img(&self, colorspace: FmetricsColorspace, hdr: bool) -> FmetricsImg {
        rgb_img(
            self.data.as_ptr(),
            self.width,
            self.height,
            self.stride(),
            self.wide,
            colorspace,
            hdr,
        )
    }
}

/// A borrowed interleaved RGB image, for a caller that already has the bytes.
///
/// No ownership or allocation, so scoring a caller-supplied buffer costs
/// nothing beyond the library call.
#[derive(Debug, Clone, Copy)]
pub struct RgbView<'a> {
    data: &'a [u8],
    width: usize,
    height: usize,
    stride: usize,
    wide: bool,
}

impl<'a> RgbView<'a> {
    /// Borrow `bytes` as a tightly packed image of `width` by `height`.
    ///
    /// `stride` is `width * 3` for 8-bit samples and `width * 6` for 16-bit
    /// ones. A `wide` view is little-endian.
    ///
    /// # Panics
    ///
    /// Panics if `bytes` is too small for the stated geometry, which is a
    /// programming error at the call site rather than a runtime condition.
    #[inline]
    #[must_use]
    pub fn new(bytes: &'a [u8], width: usize, height: usize, wide: bool) -> Self {
        let channels = if wide { 6 } else { 3 };
        let stride = width * channels;
        assert!(
            bytes.len() >= stride * height,
            "{} bytes is too few for {width}x{height} {}bit RGB, which needs {}",
            bytes.len(),
            if wide { 16 } else { 8 },
            stride * height,
        );
        Self {
            data: bytes,
            width,
            height,
            stride,
            wide,
        }
    }
}

impl RgbSource for RgbView<'_> {
    #[inline]
    fn width(&self) -> usize {
        self.width
    }

    #[inline]
    fn height(&self) -> usize {
        self.height
    }

    #[inline]
    fn stride(&self) -> usize {
        self.stride
    }

    #[inline]
    fn as_bytes(&self) -> &[u8] {
        self.data
    }

    #[inline]
    fn is_wide(&self) -> bool {
        self.wide
    }

    #[inline]
    fn as_img(&self, colorspace: FmetricsColorspace, hdr: bool) -> FmetricsImg {
        rgb_img(
            self.data.as_ptr(),
            self.width,
            self.height,
            self.stride,
            self.wide,
            colorspace,
            hdr,
        )
    }
}

/// Describe a packed interleaved buffer as a [`FmetricsImg`].
///
/// 8-bit samples become `RGB_UINT8` with the sRGB colorspace and 16-bit samples
/// become `RGB_UINT16`; the library has no other integer format, so the bit
/// depth maps one to one.
#[inline]
#[must_use]
fn rgb_img(
    data: *const u8,
    width: usize,
    height: usize,
    stride: usize,
    wide: bool,
    colorspace: FmetricsColorspace,
    hdr: bool,
) -> FmetricsImg {
    let width = width as std::ffi::c_uint;
    let height = height as std::ffi::c_uint;
    let stride = stride as std::ffi::c_uint;

    if wide {
        FmetricsImg::rgb16(data.cast(), width, height, stride, colorspace, hdr)
    } else {
        FmetricsImg::rgb8(data.cast(), width, height, stride, colorspace, hdr)
    }
}

/// Anything that can be described to fmetrics as one interleaved RGB image.
///
/// Implemented by [`RgbImage`], which owns its buffer, and [`RgbView`], which
/// borrows one, so the scorer is written once against both.
pub trait RgbSource {
    /// Width in pixels.
    fn width(&self) -> usize;
    /// Height in pixels.
    fn height(&self) -> usize;
    /// Bytes from one row to the next.
    fn stride(&self) -> usize;
    /// The interleaved samples.
    fn as_bytes(&self) -> &[u8];
    /// Whether the samples are 16-bit.
    fn is_wide(&self) -> bool;
    /// A borrowed [`FmetricsImg`] describing these samples.
    fn as_img(&self, colorspace: FmetricsColorspace, hdr: bool) -> FmetricsImg;
}

/// One planar source plane, describing a decoder's plane without owning it.
#[derive(Debug, Clone, Copy)]
pub struct PlaneSource {
    /// Width in samples, not bytes.
    pub width: usize,
    /// Height in samples.
    pub height: usize,
    /// Samples from one row to the next. May be negative for bottom-up images.
    pub stride: isize,
    /// Offset of the first visible sample from `data`.
    pub data_origin: usize,
    /// Pointer to the start of the underlying allocation.
    pub data: *const u8,
    /// Bytes per sample.
    pub bytes_per_sample: usize,
}

impl PlaneSource {
    /// Describe a plane from a raw pointer, stride and origin.
    ///
    /// # Safety
    ///
    /// `data` must remain readable for `height` rows at `stride` spacing, and
    /// `data_origin` must point at the first visible sample of row 0.
    #[inline]
    #[must_use]
    pub const unsafe fn new(
        width: usize,
        height: usize,
        stride: isize,
        data_origin: usize,
        bytes_per_sample: usize,
        data: *const u8,
    ) -> Self {
        Self {
            width,
            height,
            stride,
            data_origin,
            data,
            bytes_per_sample,
        }
    }
}

/// The three planes of a planar YUV frame, in Y, U, V order.
#[derive(Debug, Clone, Copy)]
pub struct PlaneSet {
    /// The three planes, Y first.
    pub planes: [PlaneSource; 3],
}

impl PlaneSet {
    /// Describe the planes of a decoded frame.
    ///
    /// The stride and origin conversion lives here rather than in the caller,
    /// so it happens once.
    ///
    /// # Errors
    ///
    /// Returns an error for a frame with no chroma planes. fmetrics measures
    /// colour, so monochrome input is rejected rather than scored as greyscale.
    #[inline]
    pub fn from_frame<T>(frame: &v_frame::frame::Frame<T>) -> Result<Self, FmetricsError>
    where
        T: v_frame::pixel::Pixel,
    {
        let (Some(u_plane), Some(v_plane)) = (&frame.u_plane, &frame.v_plane) else {
            return Err(FmetricsError::UnsupportedFormat {
                reason: "frame has no chroma planes; fmetrics measures colour and requires 4:2:0, \
                         4:2:2 or 4:4:4 content"
                    .to_owned(),
            });
        };

        let bytes_per_sample = std::mem::size_of::<T>();

        Ok(Self {
            planes: [
                Self::describe(&frame.y_plane, bytes_per_sample),
                Self::describe(u_plane, bytes_per_sample),
                Self::describe(v_plane, bytes_per_sample),
            ],
        })
    }

    /// Describe one plane, scaling `v_frame`'s sample-based geometry to bytes.
    #[inline]
    fn describe<T>(plane: &v_frame::plane::Plane<T>, bytes_per_sample: usize) -> PlaneSource {
        let geometry = plane.geometry();

        // SAFETY: `Plane::data` is the plane's own allocation, and the
        // geometry's stride and origin describe exactly that
        // allocation. The `PlaneSource` is built and consumed inside
        // the same submit call, so the borrow outlives
        // it, and `yuv_to_rgb` reads only within what these values describe.
        PlaneSource {
            width: geometry.width(),
            height: geometry.height(),
            // `v_frame` reports both in samples. The stride is widened rather
            // than cast because the field is signed, to allow bottom-up planes.
            stride: geometry.stride() as isize * bytes_per_sample as isize,
            data_origin: geometry.data_origin() * bytes_per_sample,
            data: plane.data().as_ptr().cast::<u8>(),
            bytes_per_sample,
        }
    }

    /// The planes in Y, U, V order.
    #[inline]
    #[must_use]
    pub const fn as_array(&self) -> &[PlaneSource; 3] {
        &self.planes
    }
}

impl std::ops::Deref for PlaneSet {
    type Target = [PlaneSource; 3];

    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.planes
    }
}

/// A packed, interleaved RGB image owned by this crate.
///
/// The buffer outlives the [`FmetricsImg`] referencing it, which is why an
/// image is borrowed rather than passed as a bare pointer.
#[derive(Debug)]
pub struct RgbImage {
    /// Interleaved samples, `u8` or `u16` according to [`RgbImage::is_wide`].
    data: Vec<u8>,
    width: usize,
    height: usize,
    wide: bool,
}

impl RgbImage {
    /// Bytes per row, which is the stride fmetrics is given.
    #[inline]
    #[must_use]
    pub const fn stride(&self) -> usize {
        if self.wide {
            self.width * 6
        } else {
            self.width * 3
        }
    }

    /// Width in pixels.
    #[inline]
    #[must_use]
    pub const fn width(&self) -> usize {
        self.width
    }

    /// Height in pixels.
    #[inline]
    #[must_use]
    pub const fn height(&self) -> usize {
        self.height
    }

    /// Whether the samples are 16-bit.
    #[inline]
    #[must_use]
    pub const fn is_wide(&self) -> bool {
        self.wide
    }

    /// The interleaved samples, as bytes.
    ///
    /// 16-bit samples are little-endian, so hashing or comparing works on
    /// bytes.
    #[inline]
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.data
    }

    /// Read one pixel as `(r, g, b)`.
    ///
    /// # Panics
    ///
    /// Panics if `(x, y)` is outside the image.
    #[inline]
    #[must_use]
    pub fn pixel(&self, x: usize, y: usize) -> (u32, u32, u32) {
        assert!(
            x < self.width && y < self.height,
            "{x},{y} is outside {}x{}",
            self.width,
            self.height
        );

        let channels = if self.wide { 6 } else { 3 };
        let offset = y * self.stride() + x * channels;

        if self.wide {
            let mut out = [0u32; 3];
            for (index, channel) in out.iter_mut().enumerate() {
                let start = offset + index * 2;
                *channel = u16::from_le_bytes([self.data[start], self.data[start + 1]]) as u32;
            }
            (out[0], out[1], out[2])
        } else {
            (
                self.data[offset] as u32,
                self.data[offset + 1] as u32,
                self.data[offset + 2] as u32,
            )
        }
    }

    /// A borrowed [`FmetricsImg`] describing this image.
    #[inline]
    #[must_use]
    pub fn as_img(&self, colorspace: FmetricsColorspace, hdr: bool) -> FmetricsImg {
        rgb_img(
            self.data.as_ptr(),
            self.width,
            self.height,
            self.stride(),
            self.wide,
            colorspace,
            hdr,
        )
    }
}

/// Everything the conversion needs, resolved once per frame rather than per
/// pixel.
///
/// The matrix coefficients and range gains are fixed for a clip, and the output
/// scale by the destination format, so hoisting them out of the inner loop
/// replaces a chain of divisions and branches per pixel with one multiply-add.
#[derive(Debug, Clone, Copy)]
struct Coefficients {
    /// Offset subtracted from luma before scaling, for a limited range.
    y_min: f32,
    /// Upper luma bound. Divided per pixel rather than used as a
    /// precomputed reciprocal, for reproducibility.
    y_max: f32,
    /// Lower chroma bound, used with `c_max` to resolve the range gain.
    c_min: f32,
    /// Upper chroma bound, for the same reason as `y_max`.
    c_max: f32,
    /// Red's gain on the V difference.
    r_v: f32,
    /// Green's loss from the U difference.
    g_u: f32,
    /// Green's loss from the V difference.
    g_v: f32,
    /// Blue's gain on the U difference.
    b_u: f32,
}

impl Coefficients {
    /// Resolve the coefficients for a matrix and range.
    #[inline]
    fn for_range(matrix: YuvMatrix, range: SampleRange) -> Self {
        let (kr, kb) = match matrix {
            YuvMatrix::Bt601 => (0.299, 0.114),
            YuvMatrix::Bt709 => (0.2126, 0.0722),
            YuvMatrix::Bt2020 => (0.2627, 0.0593),
        };
        let kg = 1.0 - kr - kb;

        // (y_min, y_max, chroma_min, chroma_max)
        let (y_min, y_max, c_min, c_max) = match range {
            SampleRange::Limited => (16.0, 235.0, 16.0, 240.0),
            SampleRange::Full => (0.0, 255.0, 0.0, 255.0),
        };

        Self {
            y_min,
            y_max,
            c_min,
            c_max,
            r_v: 2.0 * (1.0 - kr),
            g_u: 2.0 * kb * (1.0 - kb) / kg,
            g_v: 2.0 * kr * (1.0 - kr) / kg,
            b_u: 2.0 * (1.0 - kb),
        }
    }

    /// Convert one pixel to RGB on the 0-255 scale.
    #[inline]
    #[expect(
        clippy::suboptimal_flops,
        reason = "fusing these changes rounding; scores must stay bit-reproducible"
    )]
    fn apply(self, y: f32, u: f32, v: f32) -> (f32, f32, f32) {
        let y_lin = (y - self.y_min) * (255.0 / (self.y_max - self.y_min));

        // Centre before range expansion so code 128 stays neutral in limited range.
        let u_off = (u - 128.0) * (255.0 / (self.c_max - self.c_min));
        let v_off = (v - 128.0) * (255.0 / (self.c_max - self.c_min));

        let red = self.r_v * v_off + y_lin;
        let blue = self.b_u * u_off + y_lin;
        let green = y_lin - (self.g_u * u_off) - (self.g_v * v_off);

        (red, green, blue)
    }
}

/// Clamp a converted channel to `0..=255`, rounded.
#[inline]
fn clamp_u8(value: f32) -> u8 {
    value.round().clamp(0.0, 255.0) as u8
}

/// Clamp a converted channel to `0..=65535`, rounded.
#[inline]
fn clamp_u16(value: f32) -> u16 {
    value.round().clamp(0.0, 65535.0) as u16
}

/// Source-code scaling into the matrix's 8-bit-equivalent domain.
#[derive(Debug, Clone, Copy)]
struct SampleScale {
    gain: f32,
    chroma_center: f32,
}

/// Where one plane's samples live, resolved once per row rather than per pixel.
#[derive(Debug, Clone, Copy)]
struct RowPlanes {
    /// Pointer to luma sample `(0, 0)`.
    luma: *const u8,
    /// Pointer to chroma sample `(0, 0)` of each chroma plane.
    chroma: [*const u8; 2],
    /// Bytes per sample.
    bytes: usize,
    /// Luma columns covered by each chroma sample.
    subsample_x: usize,
}

impl RowPlanes {
    /// Resolve the row pointers for a row index.
    ///
    /// # Safety
    ///
    /// The planes must be live allocations, as the caller of [`yuv_to_rgb`]
    /// guarantees. Geometry and subsampling must already be validated.
    #[inline]
    #[allow(clippy::cast_possible_wrap)]
    unsafe fn at(planes: &PlaneSet, row: usize, subsampling: [usize; 2]) -> Self {
        let luma = &planes[0];
        let bytes = luma.bytes_per_sample;

        // SAFETY: delegated to the caller, who guarantees each plane is a live
        // allocation spanning its rows.
        unsafe {
            let luma_row = luma.data.add(luma.data_origin + (row as isize * luma.stride) as usize);

            let mut chroma = [luma.data; 2];
            for (index, plane) in planes.iter().enumerate().skip(1) {
                let chroma_row = row / subsampling[1];
                // Stride and origin are already bytes, including for u16 planes.
                let offset = plane.data_origin + (chroma_row as isize * plane.stride) as usize;
                chroma[index - 1] = plane.data.add(offset);
            }

            Self {
                luma: luma_row,
                chroma,
                bytes,
                subsample_x: subsampling[0],
            }
        }
    }

    /// Read one pixel's Y, U and V on the 0-255 scale.
    ///
    /// Chroma is sampled from its covering cell, nearest-neighbour.
    /// This is a measurement, not a display conversion:
    /// both sides of a pair go through exactly this path, so the sampling
    /// choice is consistent rather than optimal.
    ///
    /// # Safety
    ///
    /// The pointers must be readable for the samples read.
    #[inline]
    unsafe fn sample(self, x: usize, scale: SampleScale) -> (f32, f32, f32) {
        // SAFETY: delegated to the caller of `yuv_to_rgb`.
        unsafe {
            let y = read_normalized(self.luma.add(x * self.bytes), self.bytes, 0.0, scale.gain);

            let cx = x / self.subsample_x;

            let mut offsets = [0f32; 2];
            for (index, pointer) in self.chroma.iter().enumerate() {
                // Center in source codes first: full-range midpoint is not half
                // the peak, and the matrix expects neutral chroma at 128.
                offsets[index] = read_normalized(
                    pointer.add(cx * self.bytes),
                    self.bytes,
                    scale.chroma_center,
                    scale.gain,
                ) + 128.0;
            }

            (y, offsets[0], offsets[1])
        }
    }
}

/// Read and center one sample before scaling to 8-bit-equivalent units.
///
/// # Safety
///
/// `pointer` must be readable for `bytes`, which must be 1 or 2.
#[inline]
unsafe fn read_normalized(pointer: *const u8, bytes: usize, center: f32, gain: f32) -> f32 {
    let raw = match bytes {
        1 => {
            // SAFETY: delegated to the caller; one byte is read.
            unsafe { *pointer as f32 }
        },
        2 => {
            // SAFETY: delegated to the caller; a `u16` is read unaligned.
            unsafe { pointer.cast::<u16>().read_unaligned() as f32 }
        },
        _ => unreachable!("sample widths are validated before conversion"),
    };
    (raw - center) * gain
}

/// Convert a planar YUV frame to interleaved RGB.
///
/// `max_sample` is the source's peak: 255 for 8-bit, 1023 for 10-bit, 4095 for
/// 12-bit.
///
/// # Errors
///
/// Returns an error for a frame whose planes cannot describe a consistent
/// image.
///
/// # Safety
///
/// Each [`PlaneSource`] must point at a live allocation covering the samples
/// read.
#[inline]
pub unsafe fn yuv_to_rgb(
    planes: &PlaneSet,
    info: &ColorInfo,
    max_sample: u32,
) -> Result<RgbImage, FmetricsError> {
    let expected_peak = info.max_sample()?;
    if max_sample != expected_peak {
        return Err(FmetricsError::UnsupportedFormat {
            reason: format!(
                "{}-bit input requires peak {expected_peak}, got {max_sample}",
                info.bit_depth
            ),
        });
    }
    let expected_bytes = if info.needs_wide_samples() { 2 } else { 1 };
    for (index, plane) in planes.iter().enumerate() {
        if plane.bytes_per_sample != expected_bytes {
            return Err(FmetricsError::MalformedFrame {
                reason: format!(
                    "plane {index} has {}-byte samples; {}-bit input requires {expected_bytes}",
                    plane.bytes_per_sample, info.bit_depth
                ),
            });
        }
    }

    let luma = &planes[0];
    let width = luma.width;
    let height = luma.height;

    if width == 0 || height == 0 {
        return Err(FmetricsError::MalformedFrame {
            reason: format!("frame is {width}x{height}"),
        });
    }

    let u = &planes[1];
    let v = &planes[2];
    if (u.width, u.height) != (v.width, v.height) {
        return Err(FmetricsError::MalformedFrame {
            reason: format!(
                "chroma planes disagree: U is {}x{}, V is {}x{}",
                u.width, u.height, v.width, v.height
            ),
        });
    }
    // Prefer 4:4:4 when one-pixel axes make layouts indistinguishable;
    // every matching layout addresses the same valid cells on those axes.
    let subsampling = [[1, 1], [2, 1], [2, 2]]
        .into_iter()
        .find(|&[sx, sy]| u.width == width.div_ceil(sx) && u.height == height.div_ceil(sy))
        .ok_or_else(|| FmetricsError::MalformedFrame {
            reason: format!(
                "chroma is {}x{} for {width}x{height} luma; expected 4:2:0, 4:2:2 or 4:4:4 ceil dimensions",
                u.width, u.height
            ),
        })?;

    let coefficients = Coefficients::for_range(info.matrix, info.range);
    let scale = SampleScale {
        gain: match info.range {
            SampleRange::Full => 255.0 / max_sample as f32,
            // Nominal endpoints are exact shifts of 8-bit codes, not peak ratios.
            SampleRange::Limited => 1.0 / (1u32 << (info.bit_depth - 8)) as f32,
        },
        chroma_center: (1u32 << (info.bit_depth - 1)) as f32,
    };

    // Truncating a wider source to 8-bit would discard precision it has.
    let wide = info.needs_wide_samples();

    // `read_normalized` already brings every depth onto the 0-255 scale, so
    // only the output scale differs between the two formats.
    let out_gain = if wide { 65535.0 / 255.0 } else { 1.0 };

    let channels = if wide { 6 } else { 3 };
    let stride = width * channels;
    let mut data = vec![0u8; stride * height];

    // The two output formats are handled separately rather than branching per
    // pixel: 8-bit is the common case and would otherwise pay for a test and a
    // wider write path it never takes.
    if wide {
        convert_rows::<true>(
            planes,
            coefficients,
            scale,
            subsampling,
            &mut data,
            stride,
            out_gain,
        );
    } else {
        convert_rows::<false>(
            planes,
            coefficients,
            scale,
            subsampling,
            &mut data,
            stride,
            1.0,
        );
    }

    Ok(RgbImage {
        data,
        width,
        height,
        wide,
    })
}

/// Convert every row, writing either 3- or 6-byte pixels.
///
/// The format is a const parameter so each instantiation compiles down to one
/// write path with no per-pixel branch.
#[inline]
fn convert_rows<const WIDE: bool>(
    planes: &PlaneSet,
    coefficients: Coefficients,
    scale: SampleScale,
    subsampling: [usize; 2],
    data: &mut [u8],
    stride: usize,
    out_gain: f32,
) {
    let width = planes[0].width;
    let height = planes[0].height;

    for row in 0..height {
        // SAFETY: delegated to the caller of `yuv_to_rgb`, who guarantees each
        // plane is a live allocation spanning its rows. Validated ceil chroma
        // dimensions keep covering-cell coordinates within both chroma planes.
        let row_planes = unsafe { RowPlanes::at(planes, row, subsampling) };

        // SAFETY: `row * stride` is within `data`, which is `stride * height`
        // long.
        let out = unsafe { data.as_mut_ptr().add(row * stride) };

        for column in 0..width {
            // SAFETY: as above; the row is inside the allocation.
            let (y, u, v) = unsafe { row_planes.sample(column, scale) };

            let (r, g, b) = coefficients.apply(y, u, v);

            if WIDE {
                // SAFETY: `column < width`, so `column * 6 + 6 <= stride`,
                // within this row.
                unsafe {
                    let pixel = out.add(column * 6).cast::<u16>();
                    pixel.write_unaligned(clamp_u16(r * out_gain).to_le());
                    pixel.add(1).write_unaligned(clamp_u16(g * out_gain).to_le());
                    pixel.add(2).write_unaligned(clamp_u16(b * out_gain).to_le());
                }
            } else {
                // SAFETY: as above, with three bytes per pixel.
                unsafe {
                    let pixel = out.add(column * 3);
                    pixel.write(clamp_u8(r * out_gain));
                    pixel.add(1).write(clamp_u8(g * out_gain));
                    pixel.add(2).write(clamp_u8(b * out_gain));
                }
            }
        }
    }
}

#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    reason = "a failing assertion should panic loudly, which is what a unit test wants"
)]
mod tests {
    use super::*;

    /// An owned single plane, leaked so its pointer outlives the test.
    fn plane(width: usize, height: usize, samples: Vec<u8>) -> PlaneSource {
        let leaked = Box::leak(samples.into_boxed_slice());
        PlaneSource {
            width,
            height,
            stride: width as isize,
            data_origin: 0,
            data: leaked.as_ptr(),
            bytes_per_sample: 1,
        }
    }

    /// A 4:4:4 frame.
    fn frame444(width: usize, height: usize, y: Vec<u8>, u: Vec<u8>, v: Vec<u8>) -> PlaneSet {
        PlaneSet {
            planes: [plane(width, height, y), plane(width, height, u), plane(width, height, v)],
        }
    }

    fn full_range(matrix: YuvMatrix) -> ColorInfo {
        ColorInfo {
            matrix,
            range: SampleRange::Full,
            bit_depth: 8,
            hdr: false,
            colorspace: FmetricsColorspace::Srgb,
        }
    }

    /// Read one pixel out of a converted image.
    fn pixel(image: &RgbImage, x: usize, y: usize) -> (u32, u32, u32) {
        let channels = if image.is_wide() { 6 } else { 3 };
        let offset = y * image.stride() + x * channels;
        if image.is_wide() {
            let mut out = [0u32; 3];
            for (index, channel) in out.iter_mut().enumerate() {
                let start = offset + index * 2;
                *channel = u16::from_le_bytes([image.data[start], image.data[start + 1]]) as u32;
            }
            (out[0], out[1], out[2])
        } else {
            (
                image.data[offset] as u32,
                image.data[offset + 1] as u32,
                image.data[offset + 2] as u32,
            )
        }
    }

    fn m01_convert_pixel(
        samples: [u8; 3],
        matrix: YuvMatrix,
        range: SampleRange,
    ) -> (u32, u32, u32) {
        let planes = PlaneSet {
            planes: samples.each_ref().map(|sample| PlaneSource {
                width: 1,
                height: 1,
                stride: 1,
                data_origin: 0,
                data: std::ptr::from_ref(sample),
                bytes_per_sample: 1,
            }),
        };
        let info = ColorInfo {
            matrix,
            range,
            bit_depth: 8,
            hdr: false,
            colorspace: FmetricsColorspace::Srgb,
        };
        // SAFETY: each plane borrows one byte of `samples`, live through conversion.
        let image = unsafe { yuv_to_rgb(&planes, &info, 255).unwrap() };
        pixel(&image, 0, 0)
    }

    #[test]
    fn m01_matrix_reference_vectors() {
        let samples = [[128, 144, 128], [128, 128, 144], [96, 112, 152], [160, 152, 112]];
        // Independent inverse-luma reference: R=Y+2(1-Kr)Cr, B=Y+2(1-Kb)Cb,
        // G=(Y-Kr*R-Kb*B)/(1-Kr-Kb), rounded once. Limited Y uses 255/219,
        // Cb/Cr use (sample-128)*255/224. All reference channels are unclipped.
        let expected = [
            (
                YuvMatrix::Bt601,
                SampleRange::Full,
                [(128, 122, 156), (150, 117, 128), (130, 84, 68), (138, 163, 203)],
            ),
            (
                YuvMatrix::Bt601,
                SampleRange::Limited,
                [(130, 124, 163), (156, 117, 130), (131, 80, 61), (142, 171, 216)],
            ),
            (
                YuvMatrix::Bt709,
                SampleRange::Full,
                [(128, 125, 158), (153, 121, 128), (134, 88, 66), (135, 163, 205)],
            ),
            (
                YuvMatrix::Bt709,
                SampleRange::Limited,
                [(130, 127, 164), (159, 122, 130), (136, 84, 59), (139, 171, 218)],
            ),
            (
                YuvMatrix::Bt2020,
                SampleRange::Full,
                [(128, 125, 158), (152, 119, 128), (131, 85, 66), (136, 165, 205)],
            ),
            (
                YuvMatrix::Bt2020,
                SampleRange::Limited,
                [(130, 127, 165), (157, 120, 130), (133, 81, 59), (141, 174, 219)],
            ),
        ];
        let actual = expected.map(|(matrix, range, _)| {
            (
                matrix,
                range,
                samples.map(|samples| m01_convert_pixel(samples, matrix, range)),
            )
        });
        assert_eq!(actual, expected, "matrix/range reference RGB values");
    }

    #[test]
    fn m01_neutral_chroma_preserves_gray_and_endpoints() {
        for matrix in [YuvMatrix::Bt601, YuvMatrix::Bt709, YuvMatrix::Bt2020] {
            for (range, cases) in [
                (SampleRange::Full, [(0, 0), (128, 128), (255, 255)]),
                (SampleRange::Limited, [(16, 0), (128, 130), (235, 255)]),
            ] {
                for (y, gray) in cases {
                    assert_eq!(
                        m01_convert_pixel([y, 128, 128], matrix, range),
                        (gray, gray, gray),
                        "{matrix:?} {range:?} neutral Y={y}"
                    );
                }
            }
        }
    }

    fn m02_convert_pixel<T: v_frame::pixel::Pixel>(
        samples: [T; 3],
        bit_depth: usize,
        matrix: YuvMatrix,
        range: SampleRange,
    ) -> RgbImage {
        let bytes = std::mem::size_of::<T>();
        let planes = PlaneSet {
            planes: samples.each_ref().map(|sample| PlaneSource {
                width: 1,
                height: 1,
                stride: bytes as isize,
                data_origin: 0,
                data: std::ptr::from_ref(sample).cast(),
                bytes_per_sample: bytes,
            }),
        };
        let info = ColorInfo {
            matrix,
            range,
            bit_depth,
            hdr: false,
            colorspace: FmetricsColorspace::Srgb,
        };
        // SAFETY: each plane borrows one typed sample, live through conversion.
        unsafe { yuv_to_rgb(&planes, &info, info.max_sample().unwrap()).unwrap() }
    }

    #[test]
    fn m02_full_range_neutral_uses_meaningful_midpoint_and_peak() {
        for (depth, midpoint, cases) in [
            (
                10,
                512u16,
                [(0, 0), (256, 16400), (512, 32800), (1023, 65535)],
            ),
            (
                12,
                2048,
                [(0, 0), (1024, 16388), (2048, 32776), (4095, 65535)],
            ),
        ] {
            for matrix in [YuvMatrix::Bt601, YuvMatrix::Bt709, YuvMatrix::Bt2020] {
                for (y, gray) in cases {
                    let image = m02_convert_pixel(
                        [y, midpoint, midpoint],
                        depth,
                        matrix,
                        SampleRange::Full,
                    );
                    assert!(image.is_wide(), "{depth}-bit input needs wide output");
                    assert_eq!(image.stride(), 6);
                    // Independent gray reference: round(Y * 65535 / source peak).
                    assert_eq!(
                        image.pixel(0, 0),
                        (gray, gray, gray),
                        "{depth}-bit {matrix:?} Y={y}"
                    );
                }
            }
        }
    }

    #[test]
    fn m02_limited_range_nominal_endpoints_shift_exactly() {
        for (depth, midpoint, cases) in [
            (
                10,
                512u16,
                [(64, 0), (502, 32768), (512, 33516), (940, 65535)],
            ),
            (
                12,
                2048,
                [(256, 0), (2008, 32768), (2048, 33516), (3760, 65535)],
            ),
        ] {
            for matrix in [YuvMatrix::Bt601, YuvMatrix::Bt709, YuvMatrix::Bt2020] {
                for (y, gray) in cases {
                    let image = m02_convert_pixel(
                        [y, midpoint, midpoint],
                        depth,
                        matrix,
                        SampleRange::Limited,
                    );
                    // Nominal Y=16/235 and neutral C=128 are shifted, not peak-scaled.
                    assert_eq!(
                        image.pixel(0, 0),
                        (gray, gray, gray),
                        "{depth}-bit {matrix:?} Y={y}"
                    );
                }
            }
        }
    }

    #[test]
    #[expect(
        clippy::suboptimal_flops,
        reason = "keep the independent inverse-luma reference equation directly readable"
    )]
    fn m02_chroma_endpoints_match_independent_inverse_luma_reference() {
        for (depth, range, black, white, low, midpoint, high) in [
            (10, SampleRange::Full, 0u16, 1023, 0, 512, 1023),
            (12, SampleRange::Full, 0, 4095, 0, 2048, 4095),
            (10, SampleRange::Limited, 64, 940, 64, 512, 960),
            (12, SampleRange::Limited, 256, 3760, 256, 2048, 3840),
        ] {
            for (matrix, kr, kb) in [
                (YuvMatrix::Bt601, 0.299, 0.114),
                (YuvMatrix::Bt709, 0.2126, 0.0722),
                (YuvMatrix::Bt2020, 0.2627, 0.0593),
            ] {
                for (u, v) in [(low, midpoint), (high, midpoint), (midpoint, low), (midpoint, high)]
                {
                    let image = m02_convert_pixel([midpoint, u, v], depth, matrix, range);
                    // Reference works in unit RGB, reconstructing G from the luma identity
                    // rather than reusing the production matrix or normalization helpers.
                    let y = f64::from(midpoint - black) / f64::from(white - black);
                    let cb = (f64::from(u) - f64::from(midpoint)) / f64::from(high - low);
                    let cr = (f64::from(v) - f64::from(midpoint)) / f64::from(high - low);
                    let r = y + 2.0 * (1.0 - kr) * cr;
                    let b = y + 2.0 * (1.0 - kb) * cb;
                    let g = (y - kr * r - kb * b) / (1.0 - kr - kb);
                    let expected =
                        [r, g, b].map(|c| (c * 65535.0).round().clamp(0.0, 65535.0) as u32);
                    let actual = image.pixel(0, 0);
                    for (actual, expected) in
                        [actual.0, actual.1, actual.2].into_iter().zip(expected)
                    {
                        assert!(
                            actual.abs_diff(expected) <= 1,
                            "{depth}-bit {range:?} {matrix:?} U={u} V={v}: {actual} != {expected}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn m02_equivalent_colors_agree_across_depths() {
        // Full-range Y follows the peak ratio; chroma differences follow that
        // ratio around the integer midpoint. These are rounded source codes.
        let full = [
            ([0u8, 128, 128], [0u16, 512, 512], [0u16, 2048, 2048]),
            ([128, 128, 128], [514, 512, 512], [2056, 2048, 2048]),
            ([255, 128, 128], [1023, 512, 512], [4095, 2048, 2048]),
            ([96, 112, 152], [385, 448, 608], [1542, 1791, 2433]),
            ([160, 152, 112], [642, 608, 448], [2569, 2433, 1791]),
        ];
        for range in [SampleRange::Full, SampleRange::Limited] {
            for matrix in [YuvMatrix::Bt601, YuvMatrix::Bt709, YuvMatrix::Bt2020] {
                for (narrow, ten, twelve) in full {
                    let baseline = m02_convert_pixel(narrow, 8, matrix, range).pixel(0, 0);
                    for (depth, samples) in [(10, ten), (12, twelve)] {
                        let samples = if range == SampleRange::Limited {
                            narrow.map(|s| u16::from(s) << (depth - 8))
                        } else {
                            samples
                        };
                        let actual = m02_convert_pixel(samples, depth, matrix, range).pixel(0, 0);
                        for (actual, expected) in [actual.0, actual.1, actual.2]
                            .into_iter()
                            .zip([baseline.0, baseline.1, baseline.2])
                        {
                            assert!(
                                actual.abs_diff(expected * 257) <= 257,
                                "{depth}-bit {range:?} {matrix:?}: {actual} != {expected} * 257 (one 8-bit code tolerance)"
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn m02_adjacent_high_depth_codes_retain_sub_eight_bit_precision() {
        for (depth, midpoint, expected) in
            [(10, 512u16, [32800, 32864]), (12, 2048u16, [32776, 32792])]
        {
            let actual = [midpoint, midpoint + 1].map(|y| {
                m02_convert_pixel(
                    [y, midpoint, midpoint],
                    depth,
                    YuvMatrix::Bt709,
                    SampleRange::Full,
                )
                .pixel(0, 0)
            });
            assert_eq!(
                actual,
                expected.map(|gray| (gray, gray, gray)),
                "adjacent {depth}-bit images must not collapse or quantize to 8-bit"
            );
        }
    }

    #[test]
    fn m02_inconsistent_sample_widths_are_rejected() {
        let samples = [512u16; 3];
        for depth in [8, 10, 12] {
            let expected_bytes = if depth == 8 { 1 } else { 2 };
            for plane in 0..3 {
                for bytes in [0, 1, 2, 3, 4].into_iter().filter(|&bytes| bytes != expected_bytes) {
                    let mut planes = PlaneSet {
                        planes: samples.each_ref().map(|sample| PlaneSource {
                            width: 1,
                            height: 1,
                            stride: 2,
                            data_origin: 0,
                            data: std::ptr::from_ref(sample).cast(),
                            bytes_per_sample: expected_bytes,
                        }),
                    };
                    planes.planes[plane].bytes_per_sample = bytes;
                    let info = ColorInfo {
                        bit_depth: depth,
                        ..full_range(YuvMatrix::Bt709)
                    };
                    // SAFETY: live u16 buffers also accommodate the old reader's
                    // invalid-width fallback; rejection must precede any reads.
                    let result = unsafe { yuv_to_rgb(&planes, &info, info.max_sample().unwrap()) };
                    assert!(
                        matches!(result, Err(FmetricsError::MalformedFrame { .. })),
                        "{depth}-bit plane {plane} with {bytes}-byte samples must fail: {result:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn m02_inconsistent_peak_and_unsupported_depth_are_rejected() {
        let samples = [512u16; 3];
        for (depth, peak) in [(8, 1023), (10, 255), (10, 4095), (12, 1023), (16, 65535)] {
            let planes = PlaneSet {
                planes: samples.each_ref().map(|sample| PlaneSource {
                    width: 1,
                    height: 1,
                    stride: 2,
                    data_origin: 0,
                    data: std::ptr::from_ref(sample).cast(),
                    bytes_per_sample: if depth == 8 { 1 } else { 2 },
                }),
            };
            let info = ColorInfo {
                bit_depth: depth,
                ..full_range(YuvMatrix::Bt709)
            };
            // SAFETY: all samples are live and readable; metadata is rejected before use.
            let result = unsafe { yuv_to_rgb(&planes, &info, peak) };
            assert!(
                matches!(result, Err(FmetricsError::UnsupportedFormat { .. })),
                "{depth}-bit peak {peak} must fail: {result:?}"
            );
        }
    }

    #[test]
    fn neutral_chroma_yields_grey() {
        // With U and V centred the output must be achromatic; any colour cast
        // here means the chroma offsets are wrong.
        for matrix in [YuvMatrix::Bt601, YuvMatrix::Bt709, YuvMatrix::Bt2020] {
            let neutral = vec![128u8; 4];
            // SAFETY: the planes are leaked buffers of the stated size.
            let image = unsafe {
                yuv_to_rgb(
                    &frame444(2, 2, vec![128u8; 4], neutral.clone(), neutral),
                    &full_range(matrix),
                    255,
                )
                .unwrap()
            };
            let (r, g, b) = pixel(&image, 0, 0);
            assert_eq!((r, g, b), (r, r, r), "{matrix:?} must be achromatic");
        }
    }

    #[test]
    fn extreme_chroma_clamps_instead_of_wrapping() {
        // A wrapped value would appear as a small number where saturation is
        // expected, so this catches a missing clamp.
        //
        // Green stays in range: the independent BT.709 inverse-luma equation
        // gives 164.129724, even though red and blue need clamping.
        // SAFETY: leaked buffers of the stated size.
        let image = unsafe {
            yuv_to_rgb(
                &frame444(2, 2, vec![128u8; 4], vec![255u8; 4], vec![0u8; 4]),
                &full_range(YuvMatrix::Bt709),
                255,
            )
            .unwrap()
        };
        let (r, g, b) = pixel(&image, 0, 0);
        assert_eq!(b, 255, "blue must saturate high, not wrap");
        assert_eq!(r, 0, "red must saturate low, not wrap");
        assert_eq!(g, 164, "green must retain its unclipped BT.709 value");
    }

    #[test]
    fn full_range_black_and_white_map_to_extremes() {
        let neutral = vec![128u8; 4];
        // SAFETY: leaked buffers of the stated size.
        let black = unsafe {
            yuv_to_rgb(
                &frame444(2, 2, vec![0u8; 4], neutral.clone(), neutral.clone()),
                &full_range(YuvMatrix::Bt709),
                255,
            )
            .unwrap()
        };
        assert_eq!(pixel(&black, 0, 0), (0, 0, 0), "black must stay black");

        // SAFETY: as above.
        let white = unsafe {
            yuv_to_rgb(
                &frame444(2, 2, vec![255u8; 4], neutral.clone(), neutral),
                &full_range(YuvMatrix::Bt709),
                255,
            )
            .unwrap()
        };
        assert_eq!(
            pixel(&white, 0, 0),
            (255, 255, 255),
            "white must stay white"
        );
    }

    #[test]
    fn limited_range_expands_before_converting() {
        // Limited-range black is Y=16 and white is Y=235. Treating 16 as black
        // only works if the expansion happens; skipping it would lift the
        // shadows of every limited-range clip.
        let limited = ColorInfo {
            matrix: YuvMatrix::Bt709,
            range: SampleRange::Limited,
            bit_depth: 8,
            hdr: false,
            colorspace: FmetricsColorspace::Srgb,
        };
        let neutral = vec![128u8; 4];
        // SAFETY: leaked buffers of the stated size.
        let image = unsafe {
            yuv_to_rgb(
                &frame444(2, 2, vec![16u8, 235, 16, 16], neutral.clone(), neutral),
                &limited,
                255,
            )
            .unwrap()
        };
        assert_eq!(pixel(&image, 0, 0).0, 0, "limited-range 16 is black");
        assert!(
            pixel(&image, 1, 0).0 >= 254,
            "limited-range 235 is white, got {}",
            pixel(&image, 1, 0).0
        );
    }

    #[test]
    fn matrix_changes_the_result_for_saturated_colour() {
        // BT.601 and BT.709 differ in chroma gain. A fully saturated sample
        // would clamp to 255 under both and hide the difference, so the
        // chroma is set far enough off neutral to stay in range.
        let neutral = vec![128u8; 4];
        let convert = |matrix| {
            // SAFETY: leaked buffers of the stated size.
            unsafe {
                yuv_to_rgb(
                    &frame444(2, 2, vec![96u8; 4], neutral.clone(), vec![176u8; 4]),
                    &full_range(matrix),
                    255,
                )
                .unwrap()
            }
        };

        let bt601 = pixel(&convert(YuvMatrix::Bt601), 0, 0);
        let bt709 = pixel(&convert(YuvMatrix::Bt709), 0, 0);

        // The matrix must actually be applied, and must not be ignored.
        assert_ne!(bt601, bt709, "the matrix must affect the output");
        // Sanity: a mid-luma reddish sample stays mid-range rather than
        // clamping.
        assert!(
            bt601.0 < 255 && bt601.2 < 255,
            "the probe must not clamp, got {bt601:?}"
        );
    }

    // Hand-written covering-cell maps for 5x3 luma, including its final row/column.
    const M03_LAYOUTS: [([usize; 2], [usize; 15]); 3] = [
        ([3, 2], [0, 0, 1, 1, 2, 0, 0, 1, 1, 2, 3, 3, 4, 4, 5]),
        ([3, 3], [0, 0, 1, 1, 2, 3, 3, 4, 4, 5, 6, 6, 7, 7, 8]),
        ([5, 3], [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14]),
    ];
    const M03_Y: [u8; 15] =
        [100, 104, 108, 112, 116, 120, 124, 128, 132, 136, 140, 144, 148, 152, 156];
    const M03_U: [u8; 15] =
        [104, 120, 136, 148, 112, 140, 124, 144, 108, 116, 132, 152, 100, 128, 156];
    const M03_V: [u8; 15] =
        [148, 112, 140, 116, 136, 104, 152, 124, 144, 108, 128, 156, 120, 132, 100];

    #[expect(
        clippy::suboptimal_flops,
        reason = "keep the independent inverse-luma reference equation directly readable"
    )]
    fn m03_assert_cell_map<T: v_frame::pixel::Pixel + Into<u16>>(
        geometry: [[usize; 2]; 2],
        samples: [&[T]; 3],
        cells: &[usize],
        bit_depth: usize,
        range: SampleRange,
    ) {
        let [[width, height], _] = geometry;
        let bytes = std::mem::size_of::<T>();
        let planes = PlaneSet {
            planes: std::array::from_fn(|index| {
                let [width, height] = geometry[usize::from(index != 0)];
                assert!(samples[index].len() >= width * height);
                PlaneSource {
                    width,
                    height,
                    stride: (width * bytes) as isize,
                    data_origin: 0,
                    data: samples[index].as_ptr().cast(),
                    bytes_per_sample: bytes,
                }
            }),
        };
        let info = ColorInfo {
            range,
            bit_depth,
            ..full_range(YuvMatrix::Bt709)
        };
        // SAFETY: all three typed buffers cover the stated geometry and remain live.
        let image = unsafe { yuv_to_rgb(&planes, &info, info.max_sample().unwrap()).unwrap() };
        assert_eq!((image.width(), image.height()), (width, height));
        assert_eq!(image.is_wide(), bytes == 2);
        assert_eq!(image.stride(), width * 3 * bytes);
        assert_eq!(image.as_bytes().len(), width * height * 3 * bytes);
        assert_eq!(cells.len(), width * height);

        let shift = f64::from(1u32 << (bit_depth - 8));
        let peak = f64::from(info.max_sample().unwrap());
        let center = f64::from(1u32 << (bit_depth - 1));
        let (black, white, chroma_span) = match range {
            SampleRange::Full => (0.0, peak, peak),
            SampleRange::Limited => (16.0 * shift, 235.0 * shift, 224.0 * shift),
        };
        for (index, &cell) in cells.iter().enumerate() {
            // Only the literal map associates pixels with chroma; this f64 reference
            // reconstructs green from the BT.709 luma identity, independently of production.
            let y = (f64::from(samples[0][index].into()) - black) / (white - black);
            let cb = (f64::from(samples[1][cell].into()) - center) / chroma_span;
            let cr = (f64::from(samples[2][cell].into()) - center) / chroma_span;
            let r = y + 2.0 * (1.0 - 0.2126) * cr;
            let b = y + 2.0 * (1.0 - 0.0722) * cb;
            let g = (y - 0.2126 * r - 0.0722 * b) / (1.0 - 0.2126 - 0.0722);
            let output_peak = if bytes == 2 { 65535.0 } else { 255.0 };
            let expected =
                [r, g, b].map(|c| (c * output_peak).round().clamp(0.0, output_peak) as u32);
            let actual = image.pixel(index % width, index / width);
            for (actual, expected) in [actual.0, actual.1, actual.2].into_iter().zip(expected) {
                assert!(
                    actual.abs_diff(expected) <= 1,
                    "{geometry:?} {bit_depth}-bit {range:?} pixel {index}, cell {cell}: {actual} != {expected}"
                );
            }
        }
    }

    #[test]
    fn m03_odd_420_422_444_use_explicit_covering_cells() {
        for (chroma, cells) in M03_LAYOUTS {
            m03_assert_cell_map(
                [[5, 3], chroma],
                [&M03_Y, &M03_U, &M03_V],
                &cells,
                8,
                SampleRange::Full,
            );
        }
    }

    #[test]
    fn m03_multi_pixel_u16_preserves_byte_strides_and_cell_precision() {
        for depth in [10, 12] {
            let samples = [M03_Y, M03_U, M03_V].map(|plane| {
                std::array::from_fn::<_, 15, _>(|index| {
                    (u16::from(plane[index]) << (depth - 8)) + (index % 3) as u16
                })
            });
            for range in [SampleRange::Full, SampleRange::Limited] {
                for (chroma, cells) in M03_LAYOUTS {
                    m03_assert_cell_map(
                        [[5, 3], chroma],
                        samples.each_ref().map(|plane| plane.as_slice()),
                        &cells,
                        depth,
                        range,
                    );
                }
            }
        }
    }

    #[test]
    fn m03_single_pixel_axes_accept_ambiguous_valid_layouts() {
        for (luma, chroma, cells) in [
            ([1, 1], [1, 1], &[0][..]),
            ([1, 3], [1, 2], &[0, 0, 1][..]),
            ([1, 3], [1, 3], &[0, 1, 2][..]),
            ([3, 1], [2, 1], &[0, 0, 1][..]),
            ([3, 1], [3, 1], &[0, 1, 2][..]),
        ] {
            m03_assert_cell_map(
                [luma, chroma],
                [&M03_Y, &M03_U, &M03_V],
                cells,
                8,
                SampleRange::Full,
            );
        }
    }

    #[test]
    fn m03_inconsistent_or_unsupported_chroma_geometry_is_rejected() {
        let samples = [[128u8; 64]; 3];
        for (u, v) in [
            ([3, 2], [3, 3]),
            ([3, 3], [5, 3]),
            ([5, 3], [3, 3]),
            ([3, 2], [2, 2]),
            ([2, 2], [3, 2]),
            ([2, 1], [2, 1]),
            ([2, 2], [2, 2]),
            ([3, 1], [3, 1]),
            ([4, 2], [4, 2]),
            ([6, 3], [6, 3]),
            ([5, 4], [5, 4]),
            ([5, 2], [5, 2]),
            ([0, 0], [0, 0]),
            ([0, 2], [0, 2]),
            ([3, 0], [3, 0]),
        ] {
            let geometry = [[5, 3], u, v];
            let planes = PlaneSet {
                planes: std::array::from_fn(|index| {
                    let [width, height] = geometry[index];
                    PlaneSource {
                        width,
                        height,
                        stride: width as isize,
                        data_origin: 0,
                        data: samples[index].as_ptr(),
                        bytes_per_sample: 1,
                    }
                }),
            };
            // SAFETY: live buffers cover even the old clamped reader's accesses;
            // invalid geometry must instead be rejected before raw access.
            let result = unsafe { yuv_to_rgb(&planes, &full_range(YuvMatrix::Bt709), 255) };
            assert!(
                matches!(result, Err(FmetricsError::MalformedFrame { .. })),
                "U={u:?}, V={v:?} must fail: {result:?}"
            );
        }
    }

    #[test]
    fn eight_bit_input_produces_narrow_samples() {
        // SAFETY: leaked buffers of the stated size.
        let image = unsafe {
            yuv_to_rgb(
                &frame444(2, 2, vec![128u8; 4], vec![128u8; 4], vec![128u8; 4]),
                &full_range(YuvMatrix::Bt709),
                255,
            )
            .unwrap()
        };
        assert!(!image.is_wide());
        assert_eq!(image.stride(), 6);
    }

    #[test]
    fn stride_matches_width_times_channels() {
        for (width, height) in [(1usize, 1usize), (3, 2), (16, 9)] {
            let size = width * height;
            // SAFETY: leaked buffers of the stated size.
            let image = unsafe {
                yuv_to_rgb(
                    &frame444(
                        width,
                        height,
                        vec![128u8; size],
                        vec![128u8; size],
                        vec![128u8; size],
                    ),
                    &full_range(YuvMatrix::Bt709),
                    255,
                )
                .unwrap()
            };
            assert_eq!(image.stride(), width * 3, "{width}x{height}");
            assert_eq!(image.data.len(), width * 3 * height, "{width}x{height}");
        }
    }

    #[test]
    fn hdr_flag_is_carried_rather_than_linearised() {
        // The flag does not decode transfer functions; from_details rejects PQ/HLG.
        let info = ColorInfo {
            matrix: YuvMatrix::Bt2020,
            range: SampleRange::Limited,
            bit_depth: 10,
            hdr: true,
            colorspace: FmetricsColorspace::Srgb,
        };
        let samples = [512u16; 3];
        let planes = PlaneSet {
            planes: samples.each_ref().map(|sample| PlaneSource {
                width: 1,
                height: 1,
                stride: 2,
                data_origin: 0,
                data: std::ptr::from_ref(sample).cast(),
                bytes_per_sample: 2,
            }),
        };
        // SAFETY: each plane borrows one live u16 sample.
        let image = unsafe { yuv_to_rgb(&planes, &info, 1023).unwrap() };
        let img = image.as_img(FmetricsColorspace::Srgb, true);
        assert!(img.hdr, "the hdr flag must survive");
        assert_eq!(img.colorspace, FmetricsColorspace::Srgb);
    }

    #[test]
    fn a_degenerate_frame_is_rejected() {
        let info = full_range(YuvMatrix::Bt709);
        // SAFETY: a zero-sized plane reads nothing, and is rejected before use.
        let result = unsafe { yuv_to_rgb(&frame444(0, 0, vec![], vec![], vec![]), &info, 255) };
        assert!(matches!(result, Err(FmetricsError::MalformedFrame { .. })));
    }
}
