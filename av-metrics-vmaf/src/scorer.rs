//! Streaming per-frame VMAF scoring.

use std::{ffi::CString, marker::PhantomData, mem::size_of};

use av_decoders::{Decoder, v_frame::pixel::Pixel};
use v_frame::chroma::ChromaSubsampling;

use crate::{
    backend::{self, Backend, Context},
    config::{PoolMethod, VmafConfig, VmafModel},
    error::VmafError,
    ffi::{
        self,
        VmafConfiguration,
        VmafModel as VmafModelHandle,
        VmafModelConfig,
        VmafPicture,
        VmafPixelFormat,
    },
};

/// Geometry and format of the clips being compared.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VideoFormat {
    /// Width in pixels.
    pub width:           u32,
    /// Height in pixels.
    pub height:          u32,
    /// Bits per channel: 8, 10, 12 or 16.
    pub bit_depth:       u32,
    /// Chroma subsampling.
    pub chroma_sampling: ChromaSubsampling,
}

/// One plane of a frame: a strided block of samples with a visible origin.
///
/// This is the single description both supported frame sources reduce to. A
/// VapourSynth `Frame` and a [`v_frame::plane::Plane`] are each just a pointer,
/// a stride and an origin, so one copy loop serves both and neither source
/// needs its own path through the scorer.
///
/// # Safety
///
/// `data` must be valid for reads covering every visible row, and must stay
/// valid until the value has been consumed. The type is `Copy`, so keep it
/// within a single expression rather than storing it.
#[derive(Debug, Clone, Copy)]
pub struct PlaneSource {
    /// Width in pixels (samples per row, not bytes).
    pub width:        usize,
    /// Height in pixels.
    pub height:       usize,
    /// Samples from one row to the next. May be negative for bottom-up images.
    pub stride:       isize,
    /// Offset of the first visible sample from `data`.
    pub data_origin:  usize,
    /// Pointer to the start of the underlying allocation.
    data:             *const u8,
    /// Bytes per sample, as passed to libvmaf.
    bytes_per_sample: usize,
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

    /// Describe a plane that is contiguous, with no padding.
    ///
    /// # Safety
    ///
    /// `data` must be valid for reads of `height * width * bytes_per_sample`
    /// bytes.
    #[inline]
    #[must_use]
    pub const unsafe fn contiguous(
        width: usize,
        height: usize,
        bytes_per_sample: usize,
        data: *const u8,
    ) -> Self {
        // SAFETY: the caller's contract for this constructor is exactly the one
        // `new` requires.
        unsafe {
            Self::new(
                width,
                height,
                (width * bytes_per_sample) as isize,
                0,
                bytes_per_sample,
                data,
            )
        }
    }

    /// Pointer to the first visible byte of `row`, or `None` when out of range.
    #[inline]
    fn row(&self, row: usize) -> Option<*const u8> {
        if row >= self.height {
            return None;
        }
        // SAFETY: the constructor's contract guarantees the allocation spans
        // `height` rows at `stride` spacing, and `row` is bounds-checked above.
        Some(unsafe { self.data.add(self.data_origin + (row as isize * self.stride) as usize) })
    }
}

/// A frame as the three planes libvmaf scores.
///
/// All three representations a caller might have -- a VapourSynth `Frame`, a
/// `v_frame::frame::Frame`, or a decoded `Frame` from any backend -- reduce to
/// this, which is why the scorer needs no per-source special cases.
pub type PlaneSet = [PlaneSource; 3];

/// Describe a `v_frame` plane as a [`PlaneSource`].
///
/// This is the only adaptor `v_frame` needs: the plane already exposes a stride
/// and a data origin, so no copying or per-row walking happens here.
fn plane_source<T: Pixel>(
    plane: &v_frame::plane::Plane<T>,
    bytes_per_sample: usize,
) -> PlaneSource {
    let geometry = plane.geometry();

    // SAFETY: `Plane::data` is the plane's own allocation, and `geometry`'s stride
    // and origin describe exactly that allocation. The `PlaneSource` is built and
    // consumed inside the same `submit` call, so the borrow outlives it.
    //
    // `v_frame` reports stride and origin in samples, so both are scaled to bytes
    // here. The stride is widened rather than cast, because the field is signed to
    // allow bottom-up planes from other frame sources.
    unsafe {
        PlaneSource::new(
            geometry.width(),
            geometry.height(),
            geometry.stride() as isize * bytes_per_sample as isize,
            geometry.data_origin() * bytes_per_sample,
            bytes_per_sample,
            plane.data().as_ptr().cast::<u8>(),
        )
    }
}

impl VideoFormat {
    /// The libvmaf pixel format for this chroma sampling, or `None` if
    /// unsupported.
    ///
    /// libvmaf accepts only 4:2:0, 4:2:2 and 4:4:4 planar YUV.
    #[inline]
    #[must_use]
    pub const fn pixel_format(&self) -> Option<VmafPixelFormat> {
        match self.chroma_sampling {
            ChromaSubsampling::Yuv420 => Some(VmafPixelFormat::Yuv420p),
            ChromaSubsampling::Yuv422 => Some(VmafPixelFormat::Yuv422p),
            ChromaSubsampling::Yuv444 => Some(VmafPixelFormat::Yuv444p),
            _ => None,
        }
    }

    /// Validate that libvmaf can process this format.
    ///
    /// # Errors
    ///
    /// Returns [`VmafError::UnsupportedFormat`] if the bit depth or chroma
    /// sampling is outside libvmaf's supported set.
    #[inline]
    pub fn validate(&self) -> Result<VmafPixelFormat, VmafError> {
        if !matches!(self.bit_depth, 8 | 10 | 12 | 16) {
            return Err(VmafError::UnsupportedFormat {
                reason: format!(
                    "bit depth {} is not supported; libvmaf accepts 8, 10, 12 and 16",
                    self.bit_depth
                ),
            });
        }

        self.pixel_format().ok_or_else(|| VmafError::UnsupportedFormat {
            reason: format!(
                "chroma sampling {:?} is not supported; libvmaf accepts 4:2:0, 4:2:2 and 4:4:4",
                self.chroma_sampling
            ),
        })
    }
}

/// A single frame's scores.
#[derive(Debug, Clone, Default)]
pub struct FrameScore {
    /// The pooled model score for this frame.
    pub score:    f64,
    /// Per-feature scores, keyed by libvmaf feature name.
    pub features: Vec<(String, f64)>,
}

/// A VMAF scoring session.
///
/// Scores are produced by streaming matching frame pairs from a reference and a
/// distorted [`Decoder`]. libvmaf uses a sliding window, so the score for index
/// *n* is only final once frame *n+1* has been submitted; the last score
/// therefore arrives on flush.
///
/// A `VmafScorer` is single-use and drives a single [`VmafContext`], which
/// libvmaf documents as not safe for concurrent use. It is [`Send`] so it can
/// be moved into a worker, but must not be shared.
pub struct VmafScorer {
    context:           Context,
    config:            VmafConfig,
    format:            VideoFormat,
    model:             *mut VmafModelHandle,
    /// Feature keys used to read pooled values back, e.g. `psnr_y`.
    feature_names:     Vec<CString>,
    /// Reference picture, pooled from libvmaf and reused across frames.
    reference_picture: VmafPicture,
    /// Distorted picture, pooled from libvmaf and reused across frames.
    distorted_picture: VmafPicture,
    /// Scratch buffers for the C strings handed to `vmaf_init`.
    _model_spec:       CString,
    scored:            usize,
    /// Ensures `!Sync`, because a context is single-threaded.
    _not_sync:         PhantomData<*mut ()>,
}

impl Drop for VmafScorer {
    #[inline]
    fn drop(&mut self) {
        // Return the pooled pictures and release the model. This runs before the
        // struct's fields are dropped, so `self.context` is still live here.
        self.context.release_picture(&mut self.reference_picture);
        self.context.release_picture(&mut self.distorted_picture);

        if !self.model.is_null() {
            // SAFETY: `model` came from `vmaf_model_load` or
            // `vmaf_model_load_from_path` and is destroyed exactly once, here.
            unsafe { (self.context.api().model_destroy)(self.model) };
        }
    }
}

impl VmafScorer {
    /// Create a scorer for two clips of the given format.
    ///
    /// The reference and distorted clips must share this format exactly;
    /// mismatches are reported when scoring begins.
    ///
    /// # Errors
    ///
    /// Returns [`VmafError::LibraryNotFound`] if libvmaf could not be loaded,
    /// [`VmafError::UnsupportedFormat`] if the format is outside libvmaf's
    /// supported set, and [`VmafError::CallFailed`] if no requested backend can
    /// be initialised.
    #[expect(
        clippy::missing_inline_in_public_items,
        reason = "inlining the scorer would duplicate the whole body at every call site"
    )]
    pub fn new(config: VmafConfig, format: VideoFormat) -> Result<Self, VmafError> {
        let pixel_format = format.validate()?;

        // Resolve the model first so a bad path fails before libvmaf is touched.
        let model_spec = config.model.as_libvmaf_model()?;
        let model_spec = CString::new(model_spec).map_err(|_| VmafError::InvalidConfiguration {
            reason: "model specification contains an interior null byte".to_owned(),
        })?;

        // Registration uses extractor names (`psnr`), while reading values back uses
        // feature keys (`psnr_y`). They differ for the per-plane extractors, so both
        // lists are kept.
        let extractor_names: Vec<CString> = config
            .extractor_names()
            .into_iter()
            .map(|name| CString::new(name).expect("static extractor names contain no nulls"))
            .collect();

        let feature_names: Vec<CString> = config
            .feature_names()
            .into_iter()
            .map(|name| CString::new(name).expect("static feature names contain no nulls"))
            .collect();

        let api = ffi::VmafApi::load()?;

        let configuration = VmafConfiguration {
            log_level:   backend::LOG_LEVEL_ERROR,
            n_threads:   config.n_threads,
            // Always 1. libvmaf applies this as a gate on the *spatial*
            // extractors only, so a higher value would leave the frames in
            // between with no score at all. Subsample by submitting every n-th
            // frame pair instead.
            n_subsample: 1,
            cpumask:     0,
            gpumask:     0,
        };

        let geometry = backend::Geometry {
            width: format.width,
            height: format.height,
            bit_depth: format.bit_depth,
            pixel_format,
        };

        // Follow libvmaf's own initialisation order exactly: context, then its
        // picture pool, then the model, then the feature extractors that model
        // requires. `vmaf_read_pictures` assumes every registered extractor has a
        // live context, so ordering here is not incidental.
        let context = backend::initialise(api, configuration, config.backend, geometry)?;

        // Built-in models and on-disk models use separate loaders; each rejects the
        // other's input with `EINVAL`.
        //
        // Not every libvmaf build compiles the models in. When the built-in load
        // fails for a stock model, look for the same model as a JSON file on disk
        // before giving up, so a distribution that installs models separately works
        // without the caller having to say so.
        let mut model_config = VmafModelConfig::new();
        let mut model: *mut VmafModelHandle = std::ptr::null_mut();

        let load_from_path = config.model.is_path();
        let mut load_spec = model_spec.clone();

        // SAFETY: `model` is a valid out-pointer, `model_config` a valid mutable
        // config, and `load_spec` a NUL-terminated string that outlives the call.
        let mut status = unsafe {
            let loader = if load_from_path {
                api.model_load_from_path
            } else {
                api.model_load
            };
            loader(
                raw_mut_model(&mut model),
                raw_mut_config(&mut model_config),
                load_spec.as_ptr(),
            )
        };

        if status != 0
            && !config.model.is_path()
            && let Some(path) = config.model.find_stock_model_file()
            && let Ok(spec) = CString::new(path.display().to_string())
        {
            tracing::info!(
                path = %path.display(),
                "the built-in VMAF model is unavailable; using the model file on disk"
            );
            load_spec = spec;
            // SAFETY: as above, with `load_spec` now holding the file path and the
            // caller having supplied a configuration libvmaf accepted.
            status = unsafe {
                (api.model_load_from_path)(
                    raw_mut_model(&mut model),
                    raw_mut_config(&mut model_config),
                    load_spec.as_ptr(),
                )
            };
        }

        if status != 0 {
            let searched = VmafModel::model_search_paths()
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join(", ");

            return Err(VmafError::ModelNotFound {
                model:  load_spec.to_string_lossy().into_owned(),
                reason: format!(
                    "loading the model returned status {status}. This libvmaf build appears to \
                     have no built-in models, and no matching model file was found. Pass an \
                     explicit model path, set VMAF_MODEL_PATH to the directory holding \
                     {version}.json, or search one of: {searched}",
                    version = config.model.stock_version().unwrap_or("the model"),
                ),
            });
        }

        // SAFETY: both handles came from libvmaf and the context is live.
        let status = unsafe { (api.use_features_from_model)(context.raw, model) };
        if status != 0 {
            return Err(VmafError::CallFailed {
                function: "vmaf_use_features_from_model",
                status,
            });
        }

        // Register any additional feature extractors the caller asked for. These are
        // optional extras, so a rejection is logged rather than fatal.
        for name in &extractor_names {
            // SAFETY: `name` is a valid NUL-terminated string, and a null options
            // dictionary selects the extractor's defaults.
            let status =
                unsafe { (api.use_feature)(context.raw, name.as_ptr(), std::ptr::null_mut()) };
            if status != 0 {
                tracing::warn!(
                    feature = name.to_string_lossy().into_owned(),
                    status,
                    "libvmaf rejected an additional feature extractor; ignoring it"
                );
            }
        }

        // Hold one picture pair for the whole run, as libvmaf's own CLI does.
        // Fetching per frame also works, but reusing matches the reference flow and
        // avoids a pool round-trip per frame.
        let reference_picture = context.pooled_picture()?;
        let distorted_picture = context.pooled_picture()?;

        Ok(Self {
            context,
            config,
            format,
            model,
            feature_names,
            _model_spec: model_spec,
            reference_picture,
            distorted_picture,
            scored: 0,
            _not_sync: PhantomData,
        })
    }

    /// The backend this scorer ended up on.
    #[inline]
    #[must_use]
    pub const fn backend(&self) -> Backend {
        self.context.backend
    }

    /// The configuration in use.
    #[inline]
    #[must_use]
    pub const fn config(&self) -> &VmafConfig {
        &self.config
    }

    /// The format being scored.
    #[inline]
    #[must_use]
    pub const fn format(&self) -> VideoFormat {
        self.format
    }

    /// Whether libvmaf is available on this system.
    ///
    /// Cheap to call: the library is resolved once and cached.
    #[inline]
    #[must_use]
    pub fn is_available() -> bool {
        ffi::VmafApi::load().is_ok()
    }

    /// The runtime libvmaf version string, if available.
    #[inline]
    #[must_use]
    pub fn libvmaf_version() -> Option<&'static str> {
        ffi::VmafApi::load().ok().map(|api| api.version)
    }

    /// Score every matching frame pair from the two decoders.
    ///
    /// Returns one [`FrameScore`] per frame. Scores are emitted as soon as the
    /// sliding window makes them final, so the final score only appears at the
    /// end.
    ///
    /// # Errors
    ///
    /// Returns [`VmafError::InputMismatch`] if the two clips disagree on
    /// format, [`VmafError::FrameCountMismatch`] if their lengths differ,
    /// and [`VmafError::Decode`] or [`VmafError::CallFailed`] on downstream
    /// failures.
    #[expect(
        clippy::missing_inline_in_public_items,
        reason = "inlining the scoring loop would duplicate the whole body"
    )]
    pub fn score_decoders<T: Pixel>(
        &mut self,
        reference: &mut Decoder,
        distorted: &mut Decoder,
        mut on_score: impl FnMut(usize, &FrameScore),
    ) -> Result<Vec<FrameScore>, VmafError> {
        let reference_details = *reference.get_video_details();
        let distorted_details = *distorted.get_video_details();

        if reference_details.width != distorted_details.width
            || reference_details.height != distorted_details.height
        {
            return Err(VmafError::InputMismatch {
                reason: format!(
                    "resolution differs: {}x{} vs {}x{}",
                    reference_details.width,
                    reference_details.height,
                    distorted_details.width,
                    distorted_details.height
                ),
            });
        }

        if reference_details.bit_depth != distorted_details.bit_depth {
            return Err(VmafError::InputMismatch {
                reason: format!(
                    "bit depth differs: {} vs {}",
                    reference_details.bit_depth, distorted_details.bit_depth
                ),
            });
        }

        if reference_details.chroma_sampling != distorted_details.chroma_sampling {
            return Err(VmafError::InputMismatch {
                reason: format!(
                    "chroma sampling differs: {:?} vs {:?}",
                    reference_details.chroma_sampling, distorted_details.chroma_sampling
                ),
            });
        }

        let mut scores = Vec::new();

        loop {
            // `av-decoders` signals end of stream through `DecoderError`, not an
            // `Option`, so a short read is only distinguishable via its error.
            let reference_frame = reference.read_video_frame::<T>();
            let distorted_frame = distorted.read_video_frame::<T>();

            let (reference_frame, distorted_frame) = match (reference_frame, distorted_frame) {
                (Ok(reference_frame), Ok(distorted_frame)) => (reference_frame, distorted_frame),
                (reference_result, distorted_result) => {
                    // `av-decoders` signals end of stream as an error variant.
                    let reference_done =
                        matches!(reference_result, Err(av_decoders::DecoderError::EndOfFile));
                    let distorted_done =
                        matches!(distorted_result, Err(av_decoders::DecoderError::EndOfFile));

                    if reference_done && distorted_done {
                        break;
                    }

                    if reference_done || distorted_done {
                        return Err(VmafError::FrameCountMismatch {
                            reference: self.scored + usize::from(!reference_done),
                            distorted: self.scored + usize::from(!distorted_done),
                        });
                    }

                    // A genuine decode failure on one or both sides; surface the
                    // reference's, falling back to the distorted's.
                    let error = match (reference_result, distorted_result) {
                        (Err(error), _) => error,
                        (_, Err(error)) => error,
                        // Unreachable: both arms above require an `Err`, and the
                        // Ok/Ok case was matched earlier.
                        (Ok(_), Ok(_)) => av_decoders::DecoderError::EndOfFile,
                    };
                    return Err(error.into());
                },
            };

            let index = self.scored as u32;
            self.submit(&reference_frame, &distorted_frame, index)?;
            self.scored += 1;
        }

        // Flushing runs the extractors to completion, after which every index has a
        // score. libvmaf extracts asynchronously, so scores are not reliably
        // available mid-stream: `vmaf_score_at_index` reports an error until the
        // features for that index have been computed. Collecting only after the
        // flush avoids depending on how far the worker threads have progressed.
        self.flush()?;

        let total = self.scored;
        scores.reserve(total);
        for index in 0..total as u32 {
            if let Some(score) = self.score_at(index)? {
                scores.push(score.clone());
                on_score(scores.len() - 1, &score);
            } else {
                // Compacting here would shift every later score down a slot and
                // silently misattribute it to the wrong frame, so a missing score
                // is an error rather than something to skip over.
                return Err(VmafError::MissingScore {
                    index,
                });
            }
        }

        Ok(scores)
    }

    /// Pool the per-frame scores into a single value.
    ///
    /// # Errors
    ///
    /// Returns [`VmafError::InvalidConfiguration`] if `scores` is empty.
    #[expect(
        clippy::missing_inline_in_public_items,
        reason = "inlining would duplicate the whole body for a cold path"
    )]
    pub fn pool(scores: &[FrameScore], method: PoolMethod) -> Result<f64, VmafError> {
        if scores.is_empty() {
            return Err(VmafError::InvalidConfiguration {
                reason: "cannot pool an empty score list".to_owned(),
            });
        }

        Ok(match method {
            PoolMethod::Mean => {
                scores.iter().map(|score| score.score).sum::<f64>() / scores.len() as f64
            },
            PoolMethod::Min => scores.iter().map(|score| score.score).fold(f64::INFINITY, f64::min),
            PoolMethod::Max => {
                scores.iter().map(|score| score.score).fold(f64::NEG_INFINITY, f64::max)
            },
            PoolMethod::HarmonicMean => {
                // n / sum(1/x_i), which for 10, 20, 30 is 3 / (0.1 + 0.05 + 0.0333)
                // = 18. Matches libvmaf's VMAF_POOL_METHOD_HARMONIC_MEAN.
                let count = scores.len() as f64;
                count / scores.iter().map(|score| 1.0 / score.score).sum::<f64>()
            },
        })
    }

    /// Submit one frame pair to libvmaf.
    ///
    /// The pictures are the ones pooled at construction, reused across frames
    /// as libvmaf's own CLI does. Only their pixel data is rewritten; the
    /// geometry and buffers stay libvmaf's, which is what makes this safe
    /// to call repeatedly.
    fn submit<T: Pixel>(
        &mut self,
        reference: &v_frame::frame::Frame<T>,
        distorted: &v_frame::frame::Frame<T>,
        index: u32,
    ) -> Result<(), VmafError> {
        let bytes_per_sample = size_of::<T>();

        // `Frame` exposes chroma planes as `Option`, since monochrome frames have
        // none. libvmaf requires chroma, so this is a validation failure.
        let (Some(u_plane), Some(v_plane)) = (&reference.u_plane, &reference.v_plane) else {
            return Err(VmafError::UnsupportedFormat {
                reason: "frame has no chroma planes; libvmaf requires 4:2:0, 4:2:2 or 4:4:4 \
                         content"
                    .to_owned(),
            });
        };
        let (Some(d_u), Some(d_v)) = (&distorted.u_plane, &distorted.v_plane) else {
            return Err(VmafError::UnsupportedFormat {
                reason: "frame has no chroma planes; libvmaf requires 4:2:0, 4:2:2 or 4:4:4 \
                         content"
                    .to_owned(),
            });
        };

        let reference_planes = [
            plane_source(&reference.y_plane, bytes_per_sample),
            plane_source(u_plane, bytes_per_sample),
            plane_source(v_plane, bytes_per_sample),
        ];
        let distorted_planes = [
            plane_source(&distorted.y_plane, bytes_per_sample),
            plane_source(d_u, bytes_per_sample),
            plane_source(d_v, bytes_per_sample),
        ];

        self.submit_planes(index, reference_planes, distorted_planes)
    }

    /// Copy plane sources into the pooled pictures and hand them to libvmaf.
    ///
    /// The single path every caller funnels through: both the `v_frame` and the
    /// raw-source entry points reduce to three [`PlaneSource`]s per side, so
    /// there is exactly one copy loop and one set of validation.
    fn submit_planes(
        &mut self,
        index: u32,
        reference: PlaneSet,
        distorted: PlaneSet,
    ) -> Result<(), VmafError> {
        Self::fill_picture(
            &mut self.reference_picture,
            &reference,
            self.format.pixel_format(),
            self.format.bit_depth,
        )?;
        Self::fill_picture(
            &mut self.distorted_picture,
            &distorted,
            self.format.pixel_format(),
            self.format.bit_depth,
        )?;

        // SAFETY: both pictures came from this context's pool and hold valid
        // buffers for the geometry configured at construction.
        let status = unsafe {
            (self.context.api().read_pictures)(
                self.context.raw,
                raw_mut(&mut self.reference_picture),
                raw_mut(&mut self.distorted_picture),
                index,
            )
        };

        // libvmaf retains submitted pictures internally, so they cannot be reused
        // verbatim for the next frame. Return them to the pool and take a fresh
        // pair. The pool is sized from the thread count so libvmaf can still be
        // reading the previous pair while this happens.
        self.context.release_picture(&mut self.reference_picture);
        self.context.release_picture(&mut self.distorted_picture);
        self.reference_picture = self.context.pooled_picture()?;
        self.distorted_picture = self.context.pooled_picture()?;

        ffi::check(status, "vmaf_read_pictures")
    }

    /// Copy plane sources into a pooled picture.
    ///
    /// The pooled picture `data`, `w`, `h` and `stride` are authoritative; only
    /// the pixels are written here.
    fn fill_picture(
        picture: &mut VmafPicture,
        planes: &PlaneSet,
        pixel_format: Option<VmafPixelFormat>,
        bit_depth: u32,
    ) -> Result<(), VmafError> {
        picture.pix_fmt = pixel_format.ok_or_else(|| VmafError::UnsupportedFormat {
            reason: "chroma sampling is outside libvmaf's supported set".to_owned(),
        })?;
        // Set explicitly rather than relying on the pool having copied it from
        // `vmaf_preallocate_pictures`. libvmaf normalises every sample by
        // `picture.bpc`, and its `validate_pic_params` cross-checks it against
        // `pic_params.bpc`, so a stale or zero value would silently score 10-bit
        // content as 8-bit. Writing it here keeps the picture self-describing.
        picture.bpc = bit_depth;

        for (index, source) in planes.iter().enumerate() {
            // The pooled picture is sized for the scorer's configured geometry, so
            // a mismatch means the frame disagrees with what the scorer expects.
            let expected_width = picture.w[index] as usize;
            let expected_height = picture.h[index] as usize;
            if source.width != expected_width || source.height != expected_height {
                return Err(VmafError::InputMismatch {
                    reason: format!(
                        "plane {index} is {}x{} but the scorer was configured for \
                         {expected_width}x{expected_height}",
                        source.width, source.height
                    ),
                });
            }

            let row_bytes =
                expected_width.checked_mul(source.bytes_per_sample).ok_or_else(|| {
                    VmafError::MalformedFrame {
                        reason: format!("plane {index} row length overflows"),
                    }
                })?;

            let picture_stride = picture.stride[index];
            if picture_stride < 0 || (picture_stride as usize) < row_bytes {
                return Err(VmafError::MalformedFrame {
                    reason: format!(
                        "libvmaf pool stride {picture_stride} is too small for the {row_bytes} \
                         bytes a visible row requires"
                    ),
                });
            }

            if picture.data[index].is_null() {
                return Err(VmafError::MalformedFrame {
                    reason: format!("libvmaf returned a null buffer for plane {index}"),
                });
            }

            // SAFETY: `data[index]` is a libvmaf-owned buffer sized for
            // `expected_height` rows of `picture_stride` bytes, the checks above
            // established that each source row is exactly `row_bytes` long and
            // that the destination stride accommodates it, and `PlaneSource::row`
            // returns a pointer to a visible row of the source.
            let destination = picture.data[index].cast::<u8>();
            let picture_stride = picture_stride as usize;

            for row in 0..expected_height {
                let Some(source_row) = source.row(row) else {
                    return Err(VmafError::MalformedFrame {
                        reason: format!("plane {index} does not have row {row}"),
                    });
                };

                // SAFETY: the checks above established that the source has this
                // row, that it is `row_bytes` long, and that the destination
                // allocation is `picture_stride` bytes per row.
                unsafe {
                    std::ptr::copy_nonoverlapping::<u8>(
                        source_row,
                        destination.add(row * picture_stride),
                        row_bytes,
                    );
                };
            }
        }

        Ok(())
    }

    /// Flush libvmaf's sliding window.
    fn flush(&mut self) -> Result<(), VmafError> {
        // SAFETY: libvmaf documents null pictures as the flush signal.
        let status = unsafe {
            (self.context.api().read_pictures)(
                self.context.raw,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                self.scored as u32,
            )
        };
        ffi::check(status, "vmaf_read_pictures")
    }

    /// Read the score for a frame index, if libvmaf has produced one.
    fn score_at(&self, index: u32) -> Result<Option<FrameScore>, VmafError> {
        let mut score = 0.0f64;
        // SAFETY: `self.model` is null, which selects the context's single
        // configured model, and `score` is a valid out-pointer.
        let status = unsafe {
            (self.context.api().score_at_index)(self.context.raw, self.model, &mut score, index)
        };
        if status != 0 {
            // A score that is not ready yet is expected before the flush, and is
            // reported as `None` rather than an error.
            return Ok(None);
        }

        Ok(Some(FrameScore {
            score,
            // Feature values are pooled over the clip rather than read per index,
            // because extractors do not expose a score for every frame. They are
            // available through `pooled_features` once scoring has finished.
            features: Vec::new(),
        }))
    }

    /// Score an explicit sequence of frame pairs, taken in the order given.
    ///
    /// This is the primitive for scoring a sparse set of frames. The caller
    /// decodes each frame itself and hands over the pair, so unselected frames
    /// are never decoded and never reach libvmaf. libvmaf scores index *i*
    /// from the pair at position *i*, so the returned vector lines up with
    /// `pairs` one-for-one.
    ///
    /// `pairs` must be non-empty, and every frame must match the geometry this
    /// scorer was built for.
    ///
    /// # Errors
    ///
    /// Returns [`VmafError::MalformedFrame`] if a frame's planes do not match
    /// the scorer's geometry, and [`VmafError::InvalidConfiguration`] if
    /// `pairs` is empty.
    #[expect(
        clippy::missing_inline_in_public_items,
        reason = "inlining the scoring loop would duplicate the whole body"
    )]
    pub fn score_frames<T: Pixel>(
        &mut self,
        pairs: &[(v_frame::frame::Frame<T>, v_frame::frame::Frame<T>)],
    ) -> Result<Vec<f64>, VmafError> {
        if pairs.is_empty() {
            return Err(VmafError::InvalidConfiguration {
                reason: "no frame pairs were given to score".to_owned(),
            });
        }

        for (reference, distorted) in pairs {
            self.submit_pair(reference, distorted)?;
        }

        self.finish()
    }

    /// Submit a single frame pair and return its index.
    ///
    /// This is the low-level primitive behind [`Self::score_frames`] and
    /// [`Self::score_decoders`]. Prefer those unless the caller needs to
    /// interleave decoding with submission, which is what makes memory use
    /// independent of clip length: only libvmaf's picture pool and the one pair
    /// in flight are live, rather than every decoded frame.
    ///
    /// The index must be sequential from zero. libvmaf's temporal extractors
    /// compare each frame against its predecessor, so the order of calls is the
    /// order the model sees.
    ///
    /// # Errors
    ///
    /// Returns an error if libvmaf rejects the pair.
    #[inline]
    pub fn submit_pair<T: Pixel>(
        &mut self,
        reference: &v_frame::frame::Frame<T>,
        distorted: &v_frame::frame::Frame<T>,
    ) -> Result<u32, VmafError> {
        let index = u32::try_from(self.scored).map_err(|_| VmafError::InvalidConfiguration {
            reason: "too many frame pairs to index".to_owned(),
        })?;

        self.submit(reference, distorted, index)?;
        self.scored += 1;

        Ok(index)
    }

    /// Finish a [`Self::submit_pair`] sequence and collect every score.
    ///
    /// Call this once, after the last pair has been submitted. Flushing runs
    /// the extractors to completion, after which every index has a score.
    ///
    /// # Errors
    ///
    /// Returns [`VmafError::MissingScore`] if libvmaf produced no score for a
    /// submitted index. That is reported rather than skipped, because omitting
    /// an entry would shift every later score into the wrong slot.
    ///
    /// [`VmafError::InvalidConfiguration`] if no pair was submitted.
    #[inline]
    pub fn finish(&mut self) -> Result<Vec<f64>, VmafError> {
        if self.scored == 0 {
            return Err(VmafError::InvalidConfiguration {
                reason: "no frame pairs were given to score".to_owned(),
            });
        }

        self.flush()?;

        let total = u32::try_from(self.scored).map_err(|_| VmafError::InvalidConfiguration {
            reason: "too many frame pairs to index".to_owned(),
        })?;

        let mut scores = Vec::with_capacity(self.scored);
        for index in 0..total {
            // A missing score is an error rather than a skipped entry: omitting it
            // would return a shorter vector and shift every later score into the
            // wrong slot.
            let Some(frame) = self.score_at(index)? else {
                return Err(VmafError::MissingScore {
                    index,
                });
            };
            scores.push(frame.score);
        }

        Ok(scores)
    }

    /// Collect every score libvmaf has finished computing, without flushing.
    ///
    /// libvmaf extracts on its own threads and scores an index once the
    /// features for it exist, so after a few submissions some scores are
    /// readable while others are not yet. This reports whichever have
    /// landed, in index order, starting from `from`. Each index is returned
    /// at most once: indices below `from` are not revisited, so repeatedly
    /// draining from the position the last call returned keeps every score
    /// and reports each exactly once.
    ///
    /// Scores that have not been computed yet are simply absent from the
    /// result. Use [`Self::finish`] once the last pair is submitted, which
    /// flushes and then requires a score for every index.
    ///
    /// # Errors
    ///
    /// Returns [`VmafError::CallFailed`] if libvmaf rejects the read.
    #[inline]
    pub fn drain_scores(&mut self, from: usize) -> Result<Vec<f64>, VmafError> {
        let total = u32::try_from(self.scored).map_err(|_| VmafError::InvalidConfiguration {
            reason: "too many frame pairs to index".to_owned(),
        })?;

        let mut scores = Vec::new();
        for index in u32::try_from(from).unwrap_or(u32::MAX)..total {
            // A score that is not ready is the normal case while extraction is
            // still in flight, so it ends the run rather than failing.
            let Some(frame) = self.score_at(index)? else {
                break;
            };
            scores.push(frame.score);
        }

        Ok(scores)
    }

    /// Submit one frame pair from raw plane sources.
    ///
    /// The entry point for callers whose frames are not `v_frame::Frame`s. A
    /// VapourSynth `Frame` is the case that matters: its planes are already
    /// strided buffers, so pointing at them directly avoids the per-plane copy
    /// that `av_decoders` performs to produce a `v_frame::Frame` -- roughly
    /// 5.9 MB of copying per 1080p frame, per side.
    ///
    /// This is the same path `v_frame` callers take: both reduce to three
    /// [`PlaneSource`]s, so there is one copy loop and one set of validation.
    ///
    /// # Errors
    ///
    /// Returns [`VmafError::InputMismatch`] if a plane's geometry disagrees
    /// with the scorer's configuration, and [`VmafError::CallFailed`] if
    /// libvmaf rejects the pair.
    #[inline]
    pub fn submit_pair_raw(
        &mut self,
        reference: PlaneSet,
        distorted: PlaneSet,
    ) -> Result<u32, VmafError> {
        let index = u32::try_from(self.scored).map_err(|_| VmafError::InvalidConfiguration {
            reason: "too many frame pairs to index".to_owned(),
        })?;

        self.submit_planes(index, reference, distorted)?;
        self.scored += 1;

        Ok(index)
    }

    /// Pooled value of each requested feature over the whole clip.
    ///
    /// Only valid after scoring has finished, since the features are produced
    /// as frames are read. Each requested feature that libvmaf could
    /// compute appears in the result; unsupported ones are omitted.
    ///
    /// # Errors
    ///
    /// Returns [`VmafError::CallFailed`] if libvmaf rejects the query.
    #[expect(
        clippy::missing_inline_in_public_items,
        reason = "a cold reporting path that would bloat every call site"
    )]
    pub fn pooled_features(&self) -> Result<Vec<(String, f64)>, VmafError> {
        if self.scored == 0 {
            return Ok(Vec::new());
        }

        let last = (self.scored - 1) as u32;
        let mut features = Vec::with_capacity(self.feature_names.len());

        for name in &self.feature_names {
            let mut value = 0.0f64;
            // SAFETY: `name` is a valid NUL-terminated string, the pool method is
            // valid, the index range covers every submitted frame, and `value` is a
            // valid out-pointer.
            let status = unsafe {
                (self.context.api().feature_score_pooled)(
                    self.context.raw,
                    name.as_ptr(),
                    PoolMethod::Mean.as_libvmaf_pool(),
                    &mut value,
                    0,
                    last,
                )
            };
            ffi::check(status, "vmaf_feature_score_pooled")?;
            features.push((name.to_string_lossy().into_owned(), value));
        }

        Ok(features)
    }
}

/// libvmaf takes a mutable pointer to the picture even though it only reads it.
#[inline]
fn raw_mut(picture: &mut VmafPicture) -> *mut VmafPicture {
    picture as *mut VmafPicture
}

/// Out-pointer for a model handle.
#[inline]
fn raw_mut_model(model: &mut *mut VmafModelHandle) -> *mut *mut VmafModelHandle {
    std::ptr::from_mut(model)
}

/// Mutable pointer to a model configuration.
#[inline]
fn raw_mut_config(config: &mut VmafModelConfig) -> *mut VmafModelConfig {
    config as *mut VmafModelConfig
}

#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    reason = "assertions read better with unwrap; a failure panics with a usable message"
)]
mod tests {
    use super::*;

    #[test]
    fn pixel_format_mapping_matches_libvmaf() {
        let format = |chroma| VideoFormat {
            width:           64,
            height:          64,
            bit_depth:       10,
            chroma_sampling: chroma,
        };

        assert_eq!(
            format(ChromaSubsampling::Yuv420).pixel_format(),
            Some(VmafPixelFormat::Yuv420p)
        );
        assert_eq!(
            format(ChromaSubsampling::Yuv422).pixel_format(),
            Some(VmafPixelFormat::Yuv422p)
        );
        assert_eq!(
            format(ChromaSubsampling::Yuv444).pixel_format(),
            Some(VmafPixelFormat::Yuv444p)
        );
        assert_eq!(format(ChromaSubsampling::Monochrome).pixel_format(), None);
    }

    #[test]
    fn validate_rejects_unsupported_bit_depths() {
        for bit_depth in [0, 1, 9, 11, 13, 14, 15, 32] {
            let format = VideoFormat {
                width: 64,
                height: 64,
                bit_depth,
                chroma_sampling: ChromaSubsampling::Yuv420,
            };
            assert!(matches!(
                format.validate(),
                Err(VmafError::UnsupportedFormat { .. })
            ));
        }
    }

    #[test]
    fn validate_accepts_every_supported_bit_depth() {
        for bit_depth in [8, 10, 12, 16] {
            let format = VideoFormat {
                width: 64,
                height: 64,
                bit_depth,
                chroma_sampling: ChromaSubsampling::Yuv420,
            };
            assert!(format.validate().is_ok());
        }
    }

    #[test]
    fn pooling_of_an_empty_slice_is_an_error() {
        assert!(matches!(
            VmafScorer::pool(&[], PoolMethod::Mean),
            Err(VmafError::InvalidConfiguration { .. })
        ));
    }

    #[test]
    fn pooling_matches_hand_computed_values() {
        let scores: Vec<FrameScore> = [10.0, 20.0, 30.0]
            .into_iter()
            .map(|score| FrameScore {
                score,
                features: Vec::new(),
            })
            .collect();

        assert_eq!(VmafScorer::pool(&scores, PoolMethod::Mean).unwrap(), 20.0);
        assert_eq!(VmafScorer::pool(&scores, PoolMethod::Min).unwrap(), 10.0);
        assert_eq!(VmafScorer::pool(&scores, PoolMethod::Max).unwrap(), 30.0);
        // n / sum(1/x_i) = 3 / (1/10 + 1/20 + 1/30) = 3 / (11/60) = 180/11.
        let harmonic = VmafScorer::pool(&scores, PoolMethod::HarmonicMean).unwrap();
        assert!(
            (harmonic - 180.0 / 11.0).abs() < 1e-9,
            "harmonic mean was {harmonic}"
        );
    }
}
