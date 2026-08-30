use anyhow::Result;
use av1_grain::{GrainTableSegment, TransferFunction, generate_photon_noise_params};

use super::FGS;
use crate::{
    core::input::color_range::ColorRange,
    vapoursynth::{VapourSynthError, plugins::PluginFunction},
};

/// Raw binary representation of `Dav1dFilmGrainData` matching the C struct
/// layout from dav1d's `headers.h`. All fields are primitive types so the
/// struct can be safely re-interpreted as bytes.
#[repr(C)]
pub(crate) struct Dav1dFilmGrainDataRaw {
    seed:                     u32,
    num_y_points:             i32,
    y_points:                 [[u8; 2]; 14],
    chroma_scaling_from_luma: i32,
    num_uv_points:            [i32; 2],
    uv_points:                [[[u8; 2]; 10]; 2],
    scaling_shift:            i32,
    ar_coeff_lag:             i32,
    ar_coeffs_y:              [i8; 24],
    ar_coeffs_uv:             [[i8; 28]; 2],
    ar_coeff_shift:           u64,
    grain_scale_shift:        i32,
    uv_mult:                  [i32; 2],
    uv_luma_mult:             [i32; 2],
    uv_offset:                [i32; 2],
    overlap_flag:             i32,
    clip_to_restricted_range: i32,
}

// Safety: all fields are POD; repr(C) guarantees a stable layout.
impl Dav1dFilmGrainDataRaw {
    #[inline]
    unsafe fn as_bytes(&self) -> &[u8] {
        // SAFETY: `Dav1dFilmGrainDataRaw` is `#[repr(C)]` and contains only
        // primitive types (integers and fixed-size arrays thereof).
        unsafe {
            std::slice::from_raw_parts(
                self as *const Self as *const u8,
                std::mem::size_of::<Self>(),
            )
        }
    }
}

/// Convert an `av1_grain::GrainTableSegment` (produced by
/// `generate_photon_noise_params`) into the C-compatible binary blob.
impl From<&GrainTableSegment> for Dav1dFilmGrainDataRaw {
    #[inline]
    fn from(segment: &GrainTableSegment) -> Self {
        let mut y_points = [[0u8; 2]; 14];
        for (i, pt) in segment.scaling_points_y.iter().enumerate() {
            y_points[i] = *pt;
        }

        let mut uv_points = [[[0u8; 2]; 10]; 2];
        for (i, pt) in segment.scaling_points_cb.iter().enumerate() {
            uv_points[0][i] = *pt;
        }
        for (i, pt) in segment.scaling_points_cr.iter().enumerate() {
            uv_points[1][i] = *pt;
        }

        let mut ar_coeffs_y = [0i8; 24];
        for (i, c) in segment.ar_coeffs_y.iter().enumerate() {
            ar_coeffs_y[i] = *c;
        }

        let mut ar_coeffs_uv = [[0i8; 28]; 2];
        for (i, c) in segment.ar_coeffs_cb.iter().enumerate() {
            ar_coeffs_uv[0][i] = *c;
        }
        for (i, c) in segment.ar_coeffs_cr.iter().enumerate() {
            ar_coeffs_uv[1][i] = *c;
        }

        Self {
            seed: segment.random_seed as u32,
            num_y_points: segment.scaling_points_y.len() as i32,
            y_points,
            chroma_scaling_from_luma: segment.chroma_scaling_from_luma as i32,
            num_uv_points: [
                segment.scaling_points_cb.len() as i32,
                segment.scaling_points_cr.len() as i32,
            ],
            uv_points,
            scaling_shift: segment.scaling_shift as i32,
            ar_coeff_lag: segment.ar_coeff_lag as i32,
            ar_coeffs_y,
            ar_coeffs_uv,
            ar_coeff_shift: segment.ar_coeff_shift as u64,
            grain_scale_shift: segment.grain_scale_shift as i32,
            uv_mult: [segment.cb_mult as i32, segment.cr_mult as i32],
            uv_luma_mult: [segment.cb_luma_mult as i32, segment.cr_luma_mult as i32],
            uv_offset: [segment.cb_offset as i32, segment.cr_offset as i32],
            overlap_flag: segment.overlap_flag as i32,
            clip_to_restricted_range: 1,
        }
    }
}

impl FGS {
    /// Build the `Dav1dFilmGrainData` binary blob from the stored
    /// [`PhotonNoise`] and the given clip metadata.
    ///
    /// If `width` or `height` are [`None`], falls back to
    /// [`PhotonNoise::width`] / [`PhotonNoise::height`], then to the
    /// hard-coded defaults (1920×1080).
    pub(super) fn build_grain_binary(
        &self,
        width: Option<u32>,
        height: Option<u32>,
        transfer_function: TransferFunction,
        color_range: Option<ColorRange>,
    ) -> Result<Vec<u8>, VapourSynthError> {
        let width = self.photon_noise.width.or(width).unwrap_or(1920);
        let height = self.photon_noise.height.or(height).unwrap_or(1080);

        let mut params = generate_photon_noise_params(0, u64::MAX, av1_grain::NoiseGenArgs {
            iso_setting: self.photon_noise.iso,
            width,
            height,
            transfer_function,
            chroma_grain: self
                .photon_noise
                .chroma_iso
                .is_some_and(|c_iso| c_iso == self.photon_noise.iso),
            random_seed: None,
            full_range: matches!(color_range, Some(ColorRange::Full)),
        });

        // Separate chroma ISO override (when chroma_iso != iso)
        if let Some(chroma_iso) = self.photon_noise.chroma_iso
            && chroma_iso != self.photon_noise.iso
        {
            let chroma_params =
                generate_photon_noise_params(0, u64::MAX, av1_grain::NoiseGenArgs {
                    iso_setting: chroma_iso,
                    width,
                    height,
                    transfer_function,
                    chroma_grain: true,
                    random_seed: None,
                    full_range: matches!(color_range, Some(ColorRange::Full)),
                });
            params.scaling_points_cr = chroma_params.scaling_points_cr;
            params.scaling_points_cb = chroma_params.scaling_points_cb;
        }

        // Custom AR-coefficient overrides (c_y, ccb, ccr)
        if let Some(cy) = &self.photon_noise.c_y {
            if cy.len() > 24 {
                return Err(Self::new_error(
                    "c_y must be at most 24 coefficients".to_owned(),
                ));
            }
            params.ar_coeffs_y = arrayvec::ArrayVec::<i8, 24>::from_iter(cy.iter().copied());
        }
        if let Some(ccb) = &self.photon_noise.ccb {
            if ccb.len() > 25 {
                return Err(Self::new_error(
                    "ccb must be at most 25 coefficients".to_owned(),
                ));
            }
            params.ar_coeffs_cb = arrayvec::ArrayVec::<i8, 25>::from_iter(ccb.iter().copied());
        }
        if let Some(ccr) = &self.photon_noise.ccr {
            if ccr.len() > 25 {
                return Err(Self::new_error(
                    "ccr must be at most 25 coefficients".to_owned(),
                ));
            }
            params.ar_coeffs_cr = arrayvec::ArrayVec::<i8, 25>::from_iter(ccr.iter().copied());
        }

        // Convert to binary
        let raw: Dav1dFilmGrainDataRaw = (&params).into();

        // SAFETY: `Dav1dFilmGrainDataRaw` is `#[repr(C)]` and contains only
        // primitive types (integers and fixed-size arrays thereof).
        let entry_bytes = unsafe { raw.as_bytes() };

        // The vs-fgs plugin indexes `fgs_data` entries with
        // `std::min((size_t)n, fg_data_array.size() - 1)`, so a single
        // entry applies to every frame.
        Ok(entry_bytes.to_vec())
    }
}
