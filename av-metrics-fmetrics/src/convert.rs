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
    data:   &'a [u8],
    width:  usize,
    height: usize,
    stride: usize,
    wide:   bool,
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
    pub width:            usize,
    /// Height in samples.
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

        // SAFETY: `Plane::data` is the plane's own allocation, and the geometry's
        // stride and origin describe exactly that allocation. The `PlaneSource` is
        // built and consumed inside the same submit call, so the borrow outlives
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
    data:   Vec<u8>,
    width:  usize,
    height: usize,
    wide:   bool,
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
    /// Offset subtracted from chroma before scaling.
    c_min: f32,
    /// Upper chroma bound, for the same reason as `y_max`.
    c_max: f32,
    /// Red's gain on the V difference.
    r_v:   f32,
    /// Green's loss from the U difference.
    g_u:   f32,
    /// Green's loss from the V difference.
    g_v:   f32,
    /// Blue's gain on the U difference.
    b_u:   f32,
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
            g_u: 2.0 * kr * (1.0 - kr) / kb,
            g_v: 2.0 * (1.0 - kb) * (1.0 - kb) / kb,
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

        // Chroma is rescaled to its own span and centred, since the matrix
        // coefficients assume a full -0.5..0.5 swing.
        let u_off = (u - self.c_min) * (255.0 / (self.c_max - self.c_min)) - 128.0;
        let v_off = (v - self.c_min) * (255.0 / (self.c_max - self.c_min)) - 128.0;

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

/// Where one plane's samples live, resolved once per row rather than per pixel.
#[derive(Debug, Clone, Copy)]
struct RowPlanes {
    /// Pointer to luma sample `(0, 0)`.
    luma:   *const u8,
    /// Pointer to chroma sample `(0, 0)` of each chroma plane.
    chroma: [*const u8; 2],
    /// Bytes per sample.
    bytes:  usize,
}

impl RowPlanes {
    /// Resolve the row pointers for a row index.
    ///
    /// # Safety
    ///
    /// The planes must be live allocations, as the caller of [`yuv_to_rgb`]
    /// guarantees.
    #[inline]
    #[allow(clippy::cast_possible_wrap)]
    unsafe fn at(planes: &PlaneSet, row: usize) -> Self {
        let luma = &planes[0];
        let bytes = luma.bytes_per_sample;

        // SAFETY: delegated to the caller, who guarantees each plane is a live
        // allocation spanning its rows.
        unsafe {
            let luma_row = luma.data.add(luma.data_origin + (row as isize * luma.stride) as usize);

            let mut chroma = [luma.data; 2];
            for (index, plane) in planes.iter().enumerate().skip(1) {
                // A subsampled plane has fewer rows than luma; sampling by luma
                // row unclamped would read past the end of a 4:2:0 plane, so the
                // last row repeats instead.
                let chroma_row = row.min(plane.height.saturating_sub(1));
                let offset = plane.data_origin + (chroma_row as isize * plane.stride) as usize;
                chroma[index - 1] = plane.data.add(offset);
            }

            Self {
                luma: luma_row,
                chroma,
                bytes,
            }
        }
    }

    /// Read one pixel's Y, U and V on the 0-255 scale.
    ///
    /// Chroma is sampled at the midpoint of its covering cell,
    /// nearest-neighbour. This is a measurement, not a display conversion:
    /// both sides of a pair go through exactly this path, so the sampling
    /// choice is consistent rather than optimal.
    ///
    /// # Safety
    ///
    /// The pointers must be readable for the samples read.
    #[inline]
    unsafe fn sample(self, x: usize, chroma_width: [usize; 2]) -> (f32, f32, f32) {
        // SAFETY: delegated to the caller of `yuv_to_rgb`.
        unsafe {
            let y = read_normalized(self.luma.add(x * self.bytes), self.bytes);

            // The midpoint of the chroma cell containing this luma sample, as a
            // shift and a conditional rather than a division: this runs per pixel.
            #[expect(
                clippy::manual_is_multiple_of,
                reason = "`is_multiple_of` is clearer but this is the hot path"
            )]
            let cx = x / 2 + usize::from(x % 2 != 0);

            let mut offsets = [0f32; 2];
            for (index, pointer) in self.chroma.iter().enumerate() {
                // Clamp into the plane, which is narrower when subsampled and may
                // be narrower still than this midpoint suggests for odd sizes.
                let px = cx.min(chroma_width[index].saturating_sub(1));
                offsets[index] = read_normalized(pointer.add(px * self.bytes), self.bytes);
            }

            (y, offsets[0], offsets[1])
        }
    }
}

/// Read one sample and normalise it to the 0-255 scale.
///
/// # Safety
///
/// `pointer` must be readable for the width implied by `bytes`.
#[inline]
unsafe fn read_normalized(pointer: *const u8, bytes: usize) -> f32 {
    match bytes {
        1 => {
            // SAFETY: delegated to the caller; one byte is read.
            unsafe { *pointer.cast::<u8>() as f32 }
        },
        2 => {
            // SAFETY: delegated to the caller; a `u16` is read unaligned.
            let raw = unsafe { pointer.cast::<u16>().read_unaligned() } as u32;
            raw as f32 * (255.0 / u16::MAX as f32)
        },
        _ => {
            // SAFETY: as above, for a wider sample read as `u16`.
            let raw = unsafe { pointer.cast::<u16>().read_unaligned() } as u32;
            raw as f32 * (255.0 / u16::MAX as f32)
        },
    }
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
    let luma = &planes[0];
    let width = luma.width;
    let height = luma.height;

    if width == 0 || height == 0 {
        return Err(FmetricsError::MalformedFrame {
            reason: format!("frame is {width}x{height}"),
        });
    }

    let coefficients = Coefficients::for_range(info.matrix, info.range);

    // Truncating a wider source to 8-bit would discard precision it has.
    let wide = info.needs_wide_samples() || max_sample > u32::from(u8::MAX);

    // `read_normalized` already brings every depth onto the 0-255 scale, so only
    // the output scale differs between the two formats.
    let out_gain = if wide { 65535.0 / 255.0 } else { 1.0 };

    let channels = if wide { 6 } else { 3 };
    let stride = width * channels;
    let mut data = vec![0u8; stride * height];

    let chroma_width = [planes[1].width, planes[2].width];

    // The two output formats are handled separately rather than branching per
    // pixel: 8-bit is the common case and would otherwise pay for a test and a
    // wider write path it never takes.
    if wide {
        convert_rows::<true>(
            planes,
            coefficients,
            chroma_width,
            &mut data,
            stride,
            out_gain,
        );
    } else {
        convert_rows::<false>(planes, coefficients, chroma_width, &mut data, stride, 1.0);
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
    chroma_width: [usize; 2],
    data: &mut [u8],
    stride: usize,
    out_gain: f32,
) {
    let width = planes[0].width;
    let height = planes[0].height;

    for row in 0..height {
        // SAFETY: delegated to the caller of `yuv_to_rgb`, who guarantees each
        // plane is a live allocation spanning its rows.
        let row_planes = unsafe { RowPlanes::at(planes, row) };

        // SAFETY: `row * stride` is within `data`, which is `stride * height` long.
        let out = unsafe { data.as_mut_ptr().add(row * stride) };

        for column in 0..width {
            // SAFETY: as above; the row is inside the allocation.
            let (y, u, v) = unsafe { row_planes.sample(column, chroma_width) };

            let (r, g, b) = coefficients.apply(y, u, v);

            if WIDE {
                // SAFETY: `column < width`, so `column * 6 + 6 <= stride`, within
                // this row.
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

    /// A 4:2:0 frame, with half-width, half-height chroma.
    fn frame420(width: usize, height: usize, y: Vec<u8>, u: Vec<u8>, v: Vec<u8>) -> PlaneSet {
        PlaneSet {
            planes: [
                plane(width, height, y),
                plane(width / 2, height / 2, u),
                plane(width / 2, height / 2, v),
            ],
        }
    }

    fn full_range(matrix: YuvMatrix) -> ColorInfo {
        ColorInfo {
            matrix,
            range: SampleRange::Full,
            bit_depth: 8,
            hdr: false,
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
        // Green is *not* expected to stay neutral: it is the channel both chroma
        // differences subtract from, so extreme U and V drive it up. What matters
        // is that it saturates rather than wrapping.
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
        assert_eq!(g, 255, "green must saturate high, not wrap");
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
            matrix:    YuvMatrix::Bt709,
            range:     SampleRange::Limited,
            bit_depth: 8,
            hdr:       false,
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
        // BT.601 and BT.709 differ in chroma gain. A fully saturated sample would clamp
        // to 255 under both and hide the difference, so the chroma is set far
        // enough off neutral to stay in range.
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
        // Sanity: a mid-luma reddish sample stays mid-range rather than clamping.
        assert!(
            bt601.0 < 255 && bt601.2 < 255,
            "the probe must not clamp, got {bt601:?}"
        );
    }

    #[test]
    fn subsampled_chroma_rows_are_clamped_to_the_plane() {
        // A 4:2:0 plane has half as many rows as luma, so sampling chroma by luma row
        // unclamped reads past the end of the plane on the bottom half. Every row
        // has a distinct luma value, so a clamped row shows up as the last row's
        // chroma repeating rather than as a crash alone.
        let width = 4;
        let height = 8;
        let luma: Vec<u8> = (0..width * height).map(|i| (i * 8) as u8).collect();
        let chroma: Vec<u8> = vec![128u8; (width / 2) * (height / 2)];

        // SAFETY: leaked buffers of the stated size.
        let image = unsafe {
            yuv_to_rgb(
                &frame420(width, height, luma, chroma.clone(), chroma),
                &full_range(YuvMatrix::Bt709),
                255,
            )
            .unwrap()
        };

        // The bottom luma row is luma[4 * 7] = 224; it must still convert.
        let bottom = image.pixel(0, height - 1);
        assert_eq!(
            bottom,
            (bottom.0, bottom.0, bottom.0),
            "clamped chroma must still yield a defined pixel"
        );
    }

    #[test]
    fn subsampled_chroma_is_sampled_within_bounds() {
        // 4:2:0 on odd dimensions: the chroma midpoint can exceed the plane,
        // so this catches a missing clamp.
        // SAFETY: leaked buffers of the stated size.
        let image = unsafe {
            yuv_to_rgb(
                &frame420(7, 5, vec![100u8; 35], vec![128u8; 6], vec![128u8; 6]),
                &full_range(YuvMatrix::Bt709),
                255,
            )
            .unwrap()
        };
        assert_eq!((image.width(), image.height()), (7, 5));
        assert_eq!(image.stride(), 21);
        assert_eq!(image.data.len(), 21 * 5);
    }

    #[test]
    fn ten_bit_input_up_converts_rather_than_truncating() {
        let info = ColorInfo {
            matrix:    YuvMatrix::Bt709,
            range:     SampleRange::Full,
            bit_depth: 10,
            hdr:       false,
        };
        // SAFETY: leaked buffers of the stated size.
        let image = unsafe {
            yuv_to_rgb(
                &frame444(2, 2, vec![128u8; 4], vec![128u8; 4], vec![128u8; 4]),
                &info,
                1023,
            )
            .unwrap()
        };
        assert!(image.is_wide(), "10-bit must produce 16-bit samples");
        assert_eq!(image.stride(), 12);

        let (r, g, b) = pixel(&image, 0, 0);
        assert_eq!((r, g, b), (r, r, r), "still achromatic at depth");
        assert!(r > 0, "mid-grey must not be black, got {r}");
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
        // PQ/HLG samples pass through untouched; only the flag marks them.
        let info = ColorInfo {
            matrix:    YuvMatrix::Bt2020,
            range:     SampleRange::Limited,
            bit_depth: 10,
            hdr:       true,
        };
        // SAFETY: leaked buffers of the stated size.
        let image = unsafe {
            yuv_to_rgb(
                &frame444(2, 2, vec![128u8; 4], vec![128u8; 4], vec![128u8; 4]),
                &info,
                1023,
            )
            .unwrap()
        };
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
