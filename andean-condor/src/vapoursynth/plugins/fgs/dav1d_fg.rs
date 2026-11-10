use anyhow::Result;
use av1_grain::{GrainTableSegment, TransferFunction, generate_photon_noise_params};

use self::raw::Dav1dFilmGrainDataRaw;
use super::FGS;
use crate::{
    core::input::color_range::ColorRange,
    vapoursynth::{VapourSynthError, plugins::PluginFunction},
};

mod raw;

#[cfg(test)]
mod e18_tests;

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
        self.photon_noise
            .validate_coefficients()
            .map_err(|error| Self::new_error(error.to_string()))?;
        let width = self.photon_noise.width.or(width).unwrap_or(1920);
        let height = self.photon_noise.height.or(height).unwrap_or(1080);

        let mut params = generate_photon_noise_params(
            0,
            u64::MAX,
            av1_grain::NoiseGenArgs {
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
            },
        );

        // Separate chroma ISO override (when chroma_iso != iso)
        if let Some(chroma_iso) = self.photon_noise.chroma_iso
            && chroma_iso != self.photon_noise.iso
        {
            let chroma_params = generate_photon_noise_params(
                0,
                u64::MAX,
                av1_grain::NoiseGenArgs {
                    iso_setting: chroma_iso,
                    width,
                    height,
                    transfer_function,
                    chroma_grain: true,
                    random_seed: None,
                    full_range: matches!(color_range, Some(ColorRange::Full)),
                },
            );
            params.scaling_points_cr = chroma_params.scaling_points_cr;
            params.scaling_points_cb = chroma_params.scaling_points_cb;
        }

        self.photon_noise
            .apply_coefficients(&mut params)
            .map_err(|error| Self::new_error(error.to_string()))?;

        // Convert to binary
        let raw: Dav1dFilmGrainDataRaw = (&params).into();

        // The vs-fgs plugin indexes `fgs_data` entries with
        // `std::min((size_t)n, fg_data_array.size() - 1)`, so a single
        // entry applies to every frame.
        Ok(raw.to_bytes())
    }
}

#[cfg(test)]
mod tests {
    use std::mem::{offset_of, size_of};

    use super::*;
    use crate::{
        models::encoder::photon_noise::PhotonNoise,
        vapoursynth::script_builder::{VapourSynthPluginScript, script::Line},
    };

    fn fgs() -> FGS {
        FGS {
            photon_noise: PhotonNoise {
                iso: 800,
                chroma_iso: Some(400),
                width: Some(1280),
                height: Some(720),
                c_y: Some(vec![-128, 0, 127, 1]),
                ccb: Some(vec![-127, 126, 0, 1, -1]),
                ccr: Some(vec![125, -126, 124, 0, 1]),
            },
            dynamic_seed: None,
            simd_mask: None,
        }
    }

    #[test]
    fn i12_fgs_blob_overrides_zero_tails_and_script_hex() {
        let fgs = fgs();
        let bytes = fgs
            .build_grain_binary(None, None, TransferFunction::BT1886, None)
            .expect("FGS blob");
        assert_eq!(bytes.len(), size_of::<Dav1dFilmGrainDataRaw>());
        for (offset, capacity, expected) in [
            (
                offset_of!(Dav1dFilmGrainDataRaw, ar_coeffs_y),
                24,
                vec![128, 0, 127, 1],
            ),
            (
                offset_of!(Dav1dFilmGrainDataRaw, ar_coeffs_uv),
                28,
                vec![129, 126, 0, 1, 255],
            ),
            (
                offset_of!(Dav1dFilmGrainDataRaw, ar_coeffs_uv) + 28,
                28,
                vec![125, 130, 124, 0, 1],
            ),
        ] {
            assert_eq!(&bytes[offset..offset + expected.len()], expected);
            assert!(bytes[offset + expected.len()..offset + capacity].iter().all(|&b| b == 0));
        }

        // The generated scaling points must keep their unused ABI slots zero.
        let count_at = |offset| {
            i32::from_ne_bytes(bytes[offset..offset + 4].try_into().expect("i32 count")) as usize
        };
        for (count_offset, points_offset, capacity) in [
            (
                offset_of!(Dav1dFilmGrainDataRaw, num_y_points),
                offset_of!(Dav1dFilmGrainDataRaw, y_points),
                14,
            ),
            (
                offset_of!(Dav1dFilmGrainDataRaw, num_uv_points),
                offset_of!(Dav1dFilmGrainDataRaw, uv_points),
                10,
            ),
            (
                offset_of!(Dav1dFilmGrainDataRaw, num_uv_points) + 4,
                offset_of!(Dav1dFilmGrainDataRaw, uv_points) + 20,
                10,
            ),
        ] {
            let count = count_at(count_offset);
            assert!(count <= capacity);
            assert!(
                bytes[points_offset + count * 2..points_offset + capacity * 2]
                    .iter()
                    .all(|&b| b == 0)
            );
        }

        let (_, lines) = fgs.generate_script("clip".to_owned()).expect("FGS script");
        let Line::Expression(_, expression) = &lines[0] else {
            panic!("expected FGS expression")
        };
        let hex = expression
            .split("bytes.fromhex(\"")
            .nth(1)
            .expect("hex literal")
            .split('"')
            .next()
            .expect("closing quote");
        let (pairs, remainder) = hex.as_bytes().as_chunks::<2>();
        assert!(remainder.is_empty());
        let decoded: Vec<u8> = pairs
            .iter()
            .map(|pair| {
                u8::from_str_radix(std::str::from_utf8(pair).expect("ASCII hex"), 16)
                    .expect("hex byte")
            })
            .collect();
        assert_eq!(decoded, bytes);
        assert_eq!(
            fgs.build_grain_binary(None, None, TransferFunction::BT1886, None)
                .expect("repeated FGS blob"),
            bytes
        );
    }

    #[test]
    fn i12_fgs_coefficient_capacity_checks() {
        for (field, capacity) in [("c_y", 24), ("ccb", 25), ("ccr", 25)] {
            let mut fgs = fgs();
            fgs.photon_noise.c_y = Some(vec![127; 24]);
            fgs.photon_noise.ccb = Some(vec![127; 25]);
            fgs.photon_noise.ccr = Some(vec![127; 25]);
            for len in [capacity, capacity + 1] {
                let coefficients = match field {
                    "c_y" => &mut fgs.photon_noise.c_y,
                    "ccb" => &mut fgs.photon_noise.ccb,
                    _ => &mut fgs.photon_noise.ccr,
                };
                *coefficients = Some(vec![127; len]);
                let result = fgs.build_grain_binary(None, None, TransferFunction::BT1886, None);
                if len == capacity {
                    assert!(result.is_ok());
                } else {
                    let error = result.expect_err("reject coefficients exceeding capacity");
                    assert!(error.to_string().contains(&format!("{field} must contain exactly")));
                }
            }
        }
    }
}
