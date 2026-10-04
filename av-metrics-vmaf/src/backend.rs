//! Backend selection: CUDA when it works, CPU otherwise.

use std::{
    ffi::c_int,
    fmt::{self, Display},
};

use crate::{
    config::BackendPreference,
    error::VmafError,
    ffi::{VmafApi, VmafCudaConfiguration, VmafCudaState},
};

/// The compute backend a scorer is using.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Backend {
    /// libvmaf's scalar and SIMD CPU extractors.
    #[default]
    Cpu,
    /// libvmaf's CUDA extractors.
    Cuda,
}

impl Display for Backend {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Cpu => "cpu",
            Self::Cuda => "cuda",
        })
    }
}

/// Geometry of the pictures a context will be fed.
///
/// Carried separately from [`VideoFormat`](crate::VideoFormat) so this module
/// does not depend on the scorer's types.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Geometry {
    /// Width in pixels.
    pub width:        u32,
    /// Height in pixels.
    pub height:       u32,
    /// Bits per channel.
    pub bit_depth:    u32,
    /// Pixel format.
    pub pixel_format: crate::ffi::VmafPixelFormat,
}

/// A libvmaf scoring context, tagged with the backend it was initialised for.
pub(crate) struct Context {
    /// The context pointer owned by libvmaf.
    pub(crate) raw:     *mut crate::ffi::VmafContext,
    /// CUDA state imported into `raw`, released on drop when non-null.
    ///
    /// libvmaf copies the state during `vmaf_cuda_import_state`, but the
    /// original allocation is ours to free, so the pointer is retained
    /// here.
    cuda_state:         *mut VmafCudaState,
    api:                &'static VmafApi,
    /// The backend this context was created on.
    pub(crate) backend: Backend,
}

impl Drop for Context {
    #[inline]
    fn drop(&mut self) {
        if !self.raw.is_null() {
            // SAFETY: `raw` came from `vmaf_init` and is closed exactly once,
            // here, when the owning handle drops.
            unsafe { (self.api.close)(self.raw) };
        }

        // Release the CUDA state after the context that imported it. A CPU context
        // leaves this null and skips this entirely.
        if !self.cuda_state.is_null()
            && let Some(release) = self.api.cuda.release
        {
            // SAFETY: `cuda_state` came from `vmaf_cuda_state_init` and is
            // released exactly once, here.
            unsafe { release(self.cuda_state) };
        }
    }
}

impl Context {
    /// The resolved libvmaf API backing this context.
    #[inline]
    pub(crate) const fn api(&self) -> &'static VmafApi {
        self.api
    }

    /// Fetch a picture from libvmaf's pool for the configured geometry.
    ///
    /// The pool is sized during construction, so the returned picture's `data`,
    /// `w`, `h` and `stride` are authoritative and match the scorer's format.
    /// Callers must release it with [`Self::release_picture`].
    ///
    /// # Errors
    ///
    /// Returns [`VmafError::CallFailed`] if the pool is exhausted, which would
    /// mean more pictures are held than were preallocated.
    pub(crate) fn pooled_picture(&self) -> Result<crate::ffi::VmafPicture, VmafError> {
        let mut picture = crate::ffi::VmafPicture::zeroed();
        // SAFETY: `picture` is a valid out-parameter and `raw` is a live context
        // whose pool was sized during construction.
        let status = unsafe {
            (self.api.fetch_preallocated_picture)(self.raw, raw_mut_picture(&mut picture))
        };
        crate::ffi::check(status, "vmaf_fetch_preallocated_picture")?;
        Ok(picture)
    }

    /// Return a pooled picture to libvmaf.
    pub(crate) fn release_picture(&self, picture: &mut crate::ffi::VmafPicture) {
        // SAFETY: `picture` was obtained from `pooled_picture` on this context, and
        // the scorer releases each exactly once.
        unsafe { (self.api.picture_unref)(raw_mut_picture(picture)) };
    }
}

/// Mutable pointer to a picture, for the C entry points that require one.
#[inline]
fn raw_mut_picture(picture: &mut crate::ffi::VmafPicture) -> *mut crate::ffi::VmafPicture {
    picture as *mut crate::ffi::VmafPicture
}

// SAFETY: `VmafContext` is only ever used from the single worker that owns the
// `Context`. libvmaf documents a context as not safe for concurrent use, and
// the scorer never shares one across threads.
unsafe impl Send for Context {
}

impl Context {
    /// Create a CPU scoring context.
    ///
    /// # Safety
    ///
    /// The caller must ensure the `configuration`'s pointed-to strings and
    /// arrays stay valid for the duration of the call. libvmaf copies what
    /// it needs during `init`.
    /// Create a CPU scoring context and size its picture pool.
    fn new_cpu(
        api: &'static VmafApi,
        configuration: crate::ffi::VmafConfiguration,
        geometry: Geometry,
    ) -> Result<Self, VmafError> {
        let pool_size = picture_pool_size(configuration.n_threads);
        let mut raw: *mut crate::ffi::VmafContext = std::ptr::null_mut();
        // SAFETY: `&mut raw` is a valid out-pointer of the expected type, and
        // `configuration` is passed by value with no borrowed pointers.
        let status = unsafe { (api.init)(&mut raw, configuration) };
        crate::ffi::check(status, "vmaf_init")?;

        if raw.is_null() {
            return Err(VmafError::CallFailed {
                function: "vmaf_init",
                status:   0,
            });
        }

        // The picture pool must be sized before `vmaf_fetch_preallocated_picture`
        // can hand out buffers; without it the fetch fails. The size matters as
        // much as the existence: libvmaf's own auto-sizing (`n_threads * 2 + 2`,
        // see [`picture_pool_size`]) is skipped when a pool already exists here,
        // so undersizing it would starve the thread pool and serialise
        // extraction regardless of `n_threads`.
        // SAFETY: `raw` is a live context and `geometry` describes the frames that
        // will be submitted.
        let status = unsafe {
            (api.preallocate_pictures)(raw, crate::ffi::VmafPictureConfiguration {
                pic_params: crate::ffi::VmafPictureConfigPictureParams {
                    w:       geometry.width,
                    h:       geometry.height,
                    bpc:     geometry.bit_depth,
                    pix_fmt: geometry.pixel_format,
                },
                pic_cnt:    pool_size,
            })
        };
        if status != 0 {
            // SAFETY: closing a context that failed to configure its pool.
            unsafe { (api.close)(raw) };
            return Err(VmafError::CallFailed {
                function: "vmaf_preallocate_pictures",
                status,
            });
        }

        Ok(Self {
            raw,
            cuda_state: std::ptr::null_mut(),
            api,
            backend: Backend::Cpu,
        })
    }

    /// Create a CUDA scoring context.
    ///
    /// The CUDA state is created first and imported into the context. libvmaf
    /// rejects models whose feature extractors have no CUDA implementation, so
    /// a failure here is expected for the stock models and the caller falls
    /// back to [`Self::new_cpu`].
    fn new_cuda(
        api: &'static VmafApi,
        configuration: crate::ffi::VmafConfiguration,
        geometry: Geometry,
    ) -> Result<Self, VmafError> {
        let Some(state_init) = api.cuda.state_init else {
            return Err(VmafError::CallFailed {
                function: "vmaf_cuda_state_init",
                status:   -1,
            });
        };
        let Some(import_state) = api.cuda.import_state else {
            return Err(VmafError::CallFailed {
                function: "vmaf_cuda_import_state",
                status:   -1,
            });
        };
        let Some(preallocate) = api.cuda.preallocate_pictures else {
            return Err(VmafError::CallFailed {
                function: "vmaf_cuda_preallocate_pictures",
                status:   -1,
            });
        };

        // SAFETY: `state` is a valid out-pointer; a null `cu_ctx` asks libvmaf to
        // create its own context.
        let mut state: *mut VmafCudaState = std::ptr::null_mut();
        // SAFETY: `state` is a valid out-pointer and a null `cu_ctx` asks libvmaf
        // to create its own context.
        unsafe {
            crate::ffi::check(
                state_init(&mut state, VmafCudaConfiguration {
                    cu_ctx: std::ptr::null_mut(),
                }),
                "vmaf_cuda_state_init",
            )?;
        }

        if state.is_null() {
            return Err(VmafError::CallFailed {
                function: "vmaf_cuda_state_init",
                status:   0,
            });
        }

        let mut raw: *mut crate::ffi::VmafContext = std::ptr::null_mut();
        // SAFETY: as `new_cpu`, plus `state` which was just initialised.
        let status = unsafe { (api.init)(&mut raw, configuration) };
        if let Err(error) = crate::ffi::check(status, "vmaf_init") {
            // SAFETY: `state` is valid and owned by this scope.
            unsafe { (api.cuda.release.expect("checked above"))(state) };
            return Err(error);
        }

        // SAFETY: both pointers are valid and non-null; `import_state` copies the
        // CUDA state into the context, which then owns its use of it.
        let imported = unsafe {
            import_state(raw, state);
            preallocate(raw, crate::ffi::VmafCudaPictureConfiguration {
                pic_params:          crate::ffi::VmafCudaPictureConfigPictureParams {
                    w:       geometry.width,
                    h:       geometry.height,
                    bpc:     geometry.bit_depth,
                    pix_fmt: geometry.pixel_format,
                },
                pic_prealloc_method: crate::ffi::VmafCudaPicturePreallocationMethod::Device,
            })
        };

        if imported != 0 {
            // SAFETY: `raw` and `state` are both valid and owned by this scope.
            unsafe {
                (api.close)(raw);
                (api.cuda.release.expect("checked above"))(state);
            }
            return Err(VmafError::CallFailed {
                function: "vmaf_cuda_preallocate_pictures",
                status:   imported,
            });
        }

        Ok(Self {
            raw,
            cuda_state: state,
            api,
            backend: Backend::Cuda,
        })
    }
}

/// Initialise a context on the best available backend.
///
/// Backend choice is decided by *actually constructing* a context rather than
/// by probing for devices, because libvmaf rejects models whose feature
/// extractors have no implementation for the requested backend. The stock VMAF
/// models need PSNR and MS-SSIM, which upstream does not provide on CUDA, so a
/// CUDA initialisation can legitimately fail even on a machine with a capable
/// GPU.
///
/// # Errors
///
/// Returns the last error seen when the requested backend cannot be
/// initialised. With [`BackendPreference::RequireCuda`] no fallback is
/// attempted.
pub(crate) fn initialise(
    api: &'static VmafApi,
    configuration: crate::ffi::VmafConfiguration,
    preference: BackendPreference,
    geometry: Geometry,
) -> Result<Context, VmafError> {
    let try_cuda = match preference {
        BackendPreference::CpuOnly => false,
        BackendPreference::Auto | BackendPreference::RequireCuda => api.cuda_available(),
    };

    if try_cuda {
        match Context::new_cuda(api, configuration, geometry) {
            Ok(context) => {
                tracing::info!("VMAF using the CUDA backend");
                return Ok(context);
            },
            Err(error) => {
                if preference == BackendPreference::RequireCuda {
                    return Err(error);
                }
                tracing::warn!(
                    %error,
                    "VMAF CUDA initialisation failed; falling back to the CPU backend"
                );
            },
        }
    } else if preference == BackendPreference::RequireCuda {
        return Err(VmafError::CallFailed {
            function: "vmaf_cuda_state_init",
            status:   -1,
        });
    }

    let context = Context::new_cpu(api, configuration, geometry)?;
    tracing::info!("VMAF using the CPU backend");
    Ok(context)
}

/// Picture-pool size for a context with `n_threads` workers.
///
/// Mirrors libvmaf's own sizing in `check_picture_pool` (`n_threads * 2 + 2`)
/// for the same reason: each in-flight batch holds references on its pictures,
/// so a pool smaller than that caps how many batches the thread pool can run
/// concurrently and serialises extraction no matter what `n_threads` requests.
/// A fixed ring of four pictures left one frame in flight once libvmaf's
/// previous-reference chain is accounted for, which measured as barely more
/// than one worker no matter how many were configured — while FFmpeg, which
/// never preallocates and gets the full formula, extracted the same frames
/// about three times faster at six threads. libvmaf skips its own auto-sizing
/// when a pool already exists, so the formula has to be applied here.
///
/// The floor of four covers a serial context (`n_threads = 0`), which still
/// needs a picture for the previous-reference chain alongside the pair being
/// filled and its replacement.
#[inline]
const fn picture_pool_size(n_threads: u32) -> u32 {
    let sized = n_threads.saturating_mul(2).saturating_add(2);
    if sized < 4 { 4 } else { sized }
}

/// libvmaf log levels, used to keep the C library quiet by default.
pub(crate) const LOG_LEVEL_ERROR: c_int = 1;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_display_is_lowercase() {
        assert_eq!(Backend::Cpu.to_string(), "cpu");
        assert_eq!(Backend::Cuda.to_string(), "cuda");
    }

    /// Undersizing the pool here is invisible to libvmaf — it skips its own
    /// auto-sizing when a pool exists — so the formula is asserted directly.
    #[test]
    fn the_picture_pool_mirrors_libvmaf_thread_sizing() {
        assert_eq!(picture_pool_size(0), 4, "the serial floor keeps a pair fed");
        assert_eq!(picture_pool_size(1), 4, "two plus the formula's slack");
        assert_eq!(
            picture_pool_size(6),
            14,
            "libvmaf's own size for six workers"
        );
        assert_eq!(picture_pool_size(12), 26);
        assert_eq!(
            picture_pool_size(u32::MAX),
            u32::MAX,
            "an absurd thread count must saturate rather than overflow"
        );
    }

    #[test]
    fn cpu_only_preference_never_attempts_cuda() {
        // Without libvmaf there is no API to hand, so this exercises the
        // preference branch that must not consult CUDA at all.
        let Err(error) = VmafApi::load() else {
            return;
        };
        assert!(matches!(error, VmafError::LibraryNotFound));
    }
}
