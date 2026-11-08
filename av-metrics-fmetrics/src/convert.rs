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
//! yuvxyb supplies matrix/range/transfer conversion; this module validates and
//! copies descriptors, pads/crops valid odd edges, and packs integer output.
//! Its published math dependency retains default fastmath powers even with
//! yuvxyb's root default features disabled; numeric regressions are independent.

use std::mem::size_of;

use crate::{
    config::{ColorInfo, SampleRange, YuvMatrix, source_transfer},
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

/// Convert a planar YUV frame to interleaved RGB.
///
/// `max_sample` is the source's peak: 255 for 8-bit, 1023 for 10-bit, 4095 for
/// 12-bit.
/// yuvxyb clips normalized Y to [0,1] and C to [-0.5,0.5] before the matrix.
/// Video-family SDR is converted through gamma-2.4 linear light to sRGB; existing
/// sRGB/linear data is not transfer-converted. Odd subsampled edges are replicated
/// for conversion and cropped back, never resized.
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

    let transfer = source_transfer(info.transfer)?;
    let expected_colorspace = if transfer == yuvxyb::TransferCharacteristic::Linear {
        FmetricsColorspace::LinearSrgb
    } else {
        FmetricsColorspace::Srgb
    };
    if info.colorspace != expected_colorspace {
        return Err(FmetricsError::UnsupportedFormat {
            reason: format!(
                "source transfer {:?} requires output {expected_colorspace:?}, got {:?}",
                info.transfer, info.colorspace
            ),
        });
    }

    let padded_width = width
        .checked_add(width % subsampling[0])
        .ok_or_else(|| malformed("padded width overflow"))?;
    let padded_height = height
        .checked_add(height % subsampling[1])
        .ok_or_else(|| malformed("padded height overflow"))?;
    // Bound the largest intermediate (float RGB), before frame allocation or reads.
    padded_width
        .checked_mul(padded_height)
        .and_then(|n| n.checked_mul(12))
        .filter(|&n| isize::try_from(n).is_ok())
        .ok_or_else(|| malformed("RGB allocation dimensions overflow"))?;
    let stride = width
        .checked_mul(if info.needs_wide_samples() { 6 } else { 3 })
        .filter(|&n| u32::try_from(n).is_ok())
        .ok_or_else(|| malformed("RGB stride exceeds native representation"))?;
    if u32::try_from(height).is_err() {
        return Err(malformed("RGB height exceeds native representation"));
    }
    for plane in &planes.planes {
        validate_plane_layout(plane)?;
    }
    let config = yuvxyb::YuvConfig {
        bit_depth: info.bit_depth as u8,
        subsampling_x: (subsampling[0] - 1) as u8,
        subsampling_y: (subsampling[1] - 1) as u8,
        full_range: info.range == SampleRange::Full,
        matrix_coefficients: match info.matrix {
            YuvMatrix::Bt601 => yuvxyb::MatrixCoefficients::ST170M,
            YuvMatrix::Bt709 => yuvxyb::MatrixCoefficients::BT709,
            YuvMatrix::Bt2020 => yuvxyb::MatrixCoefficients::BT2020NonConstantLuminance,
        },
        transfer_characteristics: transfer,
        // Resolve explicitly; do not let the library guess a gamut from the matrix.
        color_primaries: yuvxyb::ColorPrimaries::BT709,
    };
    // SAFETY: descriptor arithmetic/layout is checked above; allocation spans and
    // lifetimes remain the caller's responsibility under the unchanged unsafe API.
    let rgb = unsafe {
        if info.needs_wide_samples() {
            convert_owned::<u16>(planes, config, padded_width, padded_height)?
        } else {
            convert_owned::<u8>(planes, config, padded_width, padded_height)?
        }
    };
    let rgb = if matches!(
        transfer,
        yuvxyb::TransferCharacteristic::SRGB | yuvxyb::TransferCharacteristic::Linear
    ) {
        rgb
    } else {
        let linear = yuvxyb::LinearRgb::try_from(rgb).map_err(conversion_error)?;
        yuvxyb::Rgb::try_from((
            linear,
            yuvxyb::TransferCharacteristic::SRGB,
            yuvxyb::ColorPrimaries::BT709,
        ))
        .map_err(conversion_error)?
    };
    let mut data = Vec::with_capacity(stride * height);
    for row in rgb.into_data().chunks_exact(padded_width).take(height) {
        for pixel in &row[..width] {
            for &value in pixel {
                if info.needs_wide_samples() {
                    data.extend_from_slice(
                        &((value * 65535.0).round().clamp(0.0, 65535.0) as u16).to_le_bytes(),
                    );
                } else {
                    data.push((value * 255.0).round().clamp(0.0, 255.0) as u8);
                }
            }
        }
    }
    Ok(RgbImage {
        data,
        width,
        height,
        wide: info.needs_wide_samples(),
    })
}

fn malformed(reason: impl Into<String>) -> FmetricsError {
    FmetricsError::MalformedFrame {
        reason: reason.into(),
    }
}

fn conversion_error(error: yuvxyb::ConversionError) -> FmetricsError {
    FmetricsError::UnsupportedFormat {
        reason: format!("yuvxyb conversion: {error}"),
    }
}

/// Check byte-offset arithmetic, not the caller's unknowable allocation length.
fn validate_plane_layout(plane: &PlaneSource) -> Result<(), FmetricsError> {
    let row_bytes = plane
        .width
        .checked_mul(plane.bytes_per_sample)
        .and_then(|n| isize::try_from(n).ok())
        .ok_or_else(|| malformed("plane row size overflow"))?;
    let origin =
        isize::try_from(plane.data_origin).map_err(|_| malformed("plane origin overflow"))?;
    let last = isize::try_from(plane.height - 1)
        .ok()
        .and_then(|n| n.checked_mul(plane.stride))
        .and_then(|n| origin.checked_add(n))
        .filter(|&n| n >= 0)
        .ok_or_else(|| malformed("plane row offset overflow"))?;
    if plane.data.is_null()
        || plane.stride.checked_abs().is_none_or(|stride| stride < row_bytes)
        || origin.max(last).checked_add(row_bytes).is_none()
    {
        return Err(malformed("invalid plane pointer, stride or byte extent"));
    }
    Ok(())
}

/// Copy visible source codes into owned storage; only valid odd luma edges pad.
///
/// # Safety
/// Source layouts must be validated and all described samples must remain readable.
unsafe fn convert_owned<T: v_frame::pixel::Pixel>(
    planes: &PlaneSet,
    config: yuvxyb::YuvConfig,
    width: usize,
    height: usize,
) -> Result<yuvxyb::Rgb, FmetricsError> {
    let sampling = match (config.subsampling_x, config.subsampling_y) {
        (0, 0) => v_frame::chroma::ChromaSubsampling::Yuv444,
        (1, 0) => v_frame::chroma::ChromaSubsampling::Yuv422,
        (1, 1) => v_frame::chroma::ChromaSubsampling::Yuv420,
        _ => unreachable!("subsampling was validated"),
    };
    let mut frame = v_frame::frame::FrameBuilder::new(width, height, sampling, config.bit_depth)
        .build::<T>()
        .map_err(|error| malformed(format!("owned YUV frame: {error}")))?;
    for (index, source) in planes.iter().enumerate() {
        let target = frame.plane_mut(index).expect("validated three-plane frame");
        for (y, row) in target.rows_mut().enumerate() {
            let source_y = y.min(source.height - 1);
            let offset = (source.data_origin as isize + source_y as isize * source.stride) as usize;
            for (x, sample) in row.iter_mut().enumerate() {
                // SAFETY: checked row/column offsets address visible samples;
                // read_unaligned supports byte origins and preserves meaningful depth.
                *sample = unsafe {
                    source
                        .data
                        .add(offset + x.min(source.width - 1) * size_of::<T>())
                        .cast::<T>()
                        .read_unaligned()
                };
            }
        }
    }
    let yuv = yuvxyb::Yuv::new(frame, config)
        .map_err(|error| malformed(format!("yuvxyb input: {error}")))?;
    yuvxyb::Rgb::try_from(&yuv).map_err(conversion_error)
}

#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    reason = "a failing assertion should panic loudly, which is what a unit test wants"
)]
mod tests {
    use super::*;

    #[test]
    fn m04_sdr_video_transfers_convert_gray_to_srgb() {
        use av_decoders::TransferCharacteristic as Transfer;
        // Independent: L=(Y/255)^2.4, piecewise sRGB encoding, round once.
        for transfer in
            [Transfer::BT709, Transfer::SMPTE170M, Transfer::BT2020_10, Transfer::BT2020_12]
        {
            let details = av_decoders::VideoDetails {
                width: 5,
                height: 1,
                bit_depth: 8,
                chroma_sampling: v_frame::chroma::ChromaSubsampling::Yuv444,
                frame_rate: av_decoders::Rational32::new(24, 1),
                total_frames: None,
                color_range: Some(av_decoders::ColorRange::Full),
                color_space: Some(av_decoders::ColorSpace::BT709),
                color_primaries: Some(av_decoders::ColorPrimaries::BT709),
                color_transfer_characteristic: Some(transfer),
                chroma_location: None,
                mastering_display: None,
                content_light_level: None,
            };
            let info = ColorInfo::from_details(&details).expect("ordinary tagged SDR is supported");
            let planes = frame444(5, 1, vec![0, 16, 64, 128, 255], vec![128; 5], vec![128; 5]);
            // SAFETY: the fixture owns readable planes for every sample.
            let image = unsafe { yuv_to_rgb(&planes, &info, 255).unwrap() };
            let actual: Vec<_> = (0..5).map(|x| image.pixel(x, 0)).collect();
            assert_eq!(
                actual,
                [0, 4, 53, 121, 255].map(|v| (v, v, v)),
                "{transfer:?}"
            );
            assert_eq!(info.colorspace, FmetricsColorspace::Srgb);
        }
    }

    #[test]
    fn m04_sdr_srgb_and_linear_preserve_encoded_values() {
        for (transfer, colorspace) in [
            (
                av_decoders::TransferCharacteristic::IEC61966_2_1,
                FmetricsColorspace::Srgb,
            ),
            (
                av_decoders::TransferCharacteristic::Linear,
                FmetricsColorspace::LinearSrgb,
            ),
        ] {
            let info = ColorInfo {
                transfer,
                colorspace,
                ..full_range(YuvMatrix::Bt709)
            };
            let planes = frame444(5, 1, vec![0, 16, 64, 128, 255], vec![128; 5], vec![128; 5]);
            // SAFETY: the fixture owns all visible samples.
            let image = unsafe { yuv_to_rgb(&planes, &info, 255).unwrap() };
            for (x, gray) in [0, 16, 64, 128, 255].into_iter().enumerate() {
                assert_eq!(image.pixel(x, 0), (gray, gray, gray));
            }
        }
    }

    #[test]
    fn m04_sdr_clips_nominal_excursions_before_matrix() {
        // Clamp Y to [0,1], C to [-.5,.5], then independent BT.709 inverse luma.
        assert_eq!(
            m01_convert_pixel([0, 0, 128], YuvMatrix::Bt709, SampleRange::Limited),
            (0, 24, 0)
        );
        assert_eq!(
            m01_convert_pixel([255, 128, 255], YuvMatrix::Bt709, SampleRange::Limited),
            (255, 195, 255)
        );
    }

    #[test]
    fn m04_sdr_invalid_stored_codes_are_rejected() {
        for depth in [10, 12] {
            for invalid_plane in 0..3 {
                let mut samples = [1u16 << (depth - 1); 3];
                samples[invalid_plane] = 1 << depth;
                let info = ColorInfo {
                    bit_depth: depth,
                    ..full_range(YuvMatrix::Bt709)
                };
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
                // SAFETY: each plane borrows a live u16, including the invalid stored code.
                let result = unsafe { yuv_to_rgb(&planes, &info, info.max_sample().unwrap()) };
                assert!(matches!(result, Err(FmetricsError::MalformedFrame { .. })));
            }
        }
    }

    #[test]
    fn m04_sdr_even_and_odd_frames_preserve_strides_origins_and_crop() {
        for depth in [8, 10, 12] {
            let bytes = if depth == 8 { 1 } else { 2 };
            let peak = (1u16 << depth) - 1;
            let midpoint = 1u16 << (depth - 1);
            let codes = [0, 1 << (depth - 2), midpoint, peak];
            for (width, height) in [(5usize, 3usize), (6, 4)] {
                for (sx, sy) in [(1usize, 1usize), (2, 1), (2, 2)] {
                    let geometry = [
                        (width, height),
                        (width.div_ceil(sx), height.div_ceil(sy)),
                        (width.div_ceil(sx), height.div_ceil(sy)),
                    ];
                    let storage: [Vec<u8>; 3] = std::array::from_fn(|index| {
                        let (w, h) = geometry[index];
                        let stride = w * bytes + 3;
                        let mut data = vec![0; 1 + stride * h];
                        for y in 0..h {
                            for x in 0..w {
                                let code = if index == 0 {
                                    codes[(y * w + x) % 4]
                                } else {
                                    midpoint
                                };
                                let offset = 1 + (h - 1 - y) * stride + x * bytes;
                                if bytes == 1 {
                                    data[offset] = code as u8;
                                } else {
                                    data[offset..offset + 2].copy_from_slice(&code.to_ne_bytes());
                                }
                            }
                        }
                        data
                    });
                    let planes = PlaneSet {
                        planes: std::array::from_fn(|index| {
                            let (w, h) = geometry[index];
                            let stride = w * bytes + 3;
                            PlaneSource {
                                width: w,
                                height: h,
                                stride: -(stride as isize),
                                data_origin: 1 + (h - 1) * stride,
                                data: storage[index].as_ptr(),
                                bytes_per_sample: bytes,
                            }
                        }),
                    };
                    let info = ColorInfo {
                        bit_depth: depth,
                        ..full_range(YuvMatrix::Bt709)
                    };
                    // SAFETY: buffers include origins, bottom-up rows and odd-byte u16 addresses.
                    let image = unsafe { yuv_to_rgb(&planes, &info, u32::from(peak)).unwrap() };
                    assert_eq!((image.width(), image.height()), (width, height));
                    let expected = if depth == 8 {
                        [0, 64, 128, 255]
                    } else if depth == 10 {
                        [0, 16400, 32800, 65535]
                    } else {
                        [0, 16388, 32776, 65535]
                    };
                    for index in 0..width * height {
                        let gray = expected[index % 4];
                        assert_eq!(
                            image.pixel(index % width, index / width),
                            (gray, gray, gray)
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn m04_sdr_invalid_descriptor_arithmetic_fails_before_reads() {
        let sample = 128u8;
        let base = PlaneSource {
            width: 1,
            height: 1,
            stride: 1,
            data_origin: 0,
            data: &sample,
            bytes_per_sample: 1,
        };
        for plane in [
            PlaneSource {
                data_origin: usize::MAX,
                ..base
            },
            PlaneSource {
                stride: isize::MIN,
                ..base
            },
            PlaneSource {
                width: usize::MAX,
                ..base
            },
        ] {
            let planes = PlaneSet { planes: [plane; 3] };
            // SAFETY: live sample; invalid descriptors must be rejected before raw access.
            let result = unsafe { yuv_to_rgb(&planes, &full_range(YuvMatrix::Bt709), 255) };
            assert!(matches!(result, Err(FmetricsError::MalformedFrame { .. })));
        }
    }

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
            transfer: av_decoders::TransferCharacteristic::IEC61966_2_1,
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
            transfer: av_decoders::TransferCharacteristic::IEC61966_2_1,
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
            transfer: av_decoders::TransferCharacteristic::IEC61966_2_1,
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
                    let actual = image.pixel(0, 0);
                    for value in [actual.0, actual.1, actual.2] {
                        // Exactly half light is 32767.5 in u16; yuvxyb's f32
                        // matrix can land one ULP below that tie (BT.601 R/B).
                        let tolerance = u32::from(gray == 32768);
                        assert!(
                            value.abs_diff(gray) <= tolerance,
                            "{depth}-bit {matrix:?} Y={y}: {actual:?} != {gray}"
                        );
                    }
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
                    // Approved clipping precedes the matrix, including full-range
                    // code zero, which lies just below -0.5 about the integer center.
                    let cb = ((f64::from(u) - f64::from(midpoint)) / f64::from(high - low))
                        .clamp(-0.5, 0.5);
                    let cr = ((f64::from(v) - f64::from(midpoint)) / f64::from(high - low))
                        .clamp(-0.5, 0.5);
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
            transfer: av_decoders::TransferCharacteristic::IEC61966_2_1,
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
                    actual.abs_diff(expected) <= u32::from(bytes == 2),
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
            transfer: av_decoders::TransferCharacteristic::IEC61966_2_1,
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
