//! Native vship perceptual quality scoring via the libvship C API.
//!
//! libvship implements SSIMULACRA2, Butteraugli and CVVDP. This crate binds its
//! C API directly and scores frame pairs as they are decoded, so no temporary
//! files and no distortion maps are involved.
//!
//! Frames come from [`av_decoders`], so a clip can be read from Y4M, FFMS2,
//! FFmpeg or a VapourSynth script without this crate knowing which.
//!
//! # Availability
//!
//! libvship is a GPU metric with no CPU fallback in the shipped builds, and it
//! is distributed as a VapourSynth plugin rather than a standalone library. It
//! is opened with `dlopen` at runtime, so the crate compiles on machines
//! without it and [`is_available`] reports `false`. Since libvship v5.1.2,
//! Vulkan initialization is lazy and a missing driver is reported normally.
//! [`Vship_GPUFullCheck`](ffi::VshipApi::gpu_full_check) confirms the device is
//! usable after loading.
//!
//! # Metrics
//!
//! [`VshipMetric::Ssimulacra2`] and [`VshipMetric::Butteraugli`] are per-pair:
//! each score is final the moment `Vship_ComputeHandler` returns, and the
//! scorer keeps one handler per worker. [`VshipMetric::Cvvdp`] is temporal, so
//! it keeps a single handler, sees frames in order, and resets its history at a
//! scene break.
//!
//! # Example
//!
//! ```no_run
//! use av_decoders::Decoder;
//! use av_metrics_vship::{PoolMethod, VideoFormat, VshipConfig, VshipMetric, VshipScorer};
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! // Requires libvship and a usable GPU at runtime; see the crate README.
//! let config = VshipConfig::new().with_handler_threads(4);
//!
//! let mut reference = Decoder::from_file("reference.mkv")?;
//! let mut distorted = Decoder::from_file("distorted.mkv")?;
//!
//! let format = VideoFormat::from_details(reference.get_video_details());
//! let mut scorer = VshipScorer::new(config, format, format, None)?;
//!
//! let scores = scorer.score_decoders::<u8>(&mut reference, &mut distorted, |_, _| {})?;
//! let pooled = VshipScorer::pool(&scores, VshipMetric::Ssimulacra2, PoolMethod::Mean)?;
//! println!("SSIMULACRA2: {pooled:.4}");
//! # Ok(())
//! # }
//! ```

mod config;
mod error;
pub mod ffi;
mod scorer;

pub use config::{
    ButteraugliParams,
    CvvdpParams,
    DEFAULT_HANDLER_THREADS,
    PoolMethod,
    VshipConfig,
    VshipMetric,
    colorspace_from_details,
    sample_for_bit_depth,
    subsample_for_chroma,
};
pub use error::VshipError;
pub use ffi::{
    VshipBackend,
    VshipChromaLocation,
    VshipChromaSubsample,
    VshipColorFamily,
    VshipColorspace,
    VshipCropRectangle,
    VshipDeviceInfo,
    VshipPrimaries,
    VshipRange,
    VshipSample,
    VshipTransferFunction,
    VshipYuvMatrix,
};
pub use scorer::{
    FrameScore,
    PlaneSet,
    PlaneSource,
    VideoFormat,
    VshipDevice,
    VshipScorer,
    devices,
};

/// The runtime libvship version string, if libvship is loaded and usable.
#[inline]
#[must_use]
pub fn libvship_version() -> Option<&'static str> {
    VshipScorer::libvship_version()
}

/// Whether vship scoring is available on this system.
///
/// Cheap to call: libvship is resolved once and cached. Requires both a
/// loadable library and a device that passes `Vship_GPUFullCheck`.
#[inline]
#[must_use]
pub fn is_available() -> bool {
    VshipScorer::is_available()
}

/// The name of the device libvship would use, if it is available.
#[inline]
#[must_use]
pub fn device_name() -> Option<String> {
    VshipScorer::device_name()
}

/// The compute backend the loaded libvship was compiled for, if it is loaded.
///
/// Distinct from [`is_available`]: a library can load and still fail
/// `Vship_GPUFullCheck`, so the backend may be reported for a system where
/// scoring cannot actually run.
#[inline]
#[must_use]
pub fn backend() -> Option<VshipBackend> {
    ffi::VshipApi::load().ok().map(ffi::VshipApi::backend)
}

/// The device index libvship would use when none is configured, if there is
/// one.
///
/// This is what [`VshipConfig::gpu_id_or_default`] resolves to, and it prefers
/// a discrete GPU when one is present. `None` means libvship could not be
/// loaded or reported no devices, so there is no index to name.
#[inline]
#[must_use]
pub fn default_gpu_id() -> Option<u32> {
    scorer::default_gpu_id()
}
