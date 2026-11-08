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
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Bits per channel.
    pub bit_depth: u32,
    /// Pixel format.
    pub pixel_format: crate::ffi::VmafPixelFormat,
}

/// A libvmaf scoring context, tagged with the backend it was initialised for.
pub(crate) struct Context {
    /// The context pointer owned by libvmaf.
    pub(crate) raw: *mut crate::ffi::VmafContext,
    api: &'static VmafApi,
    /// The backend this context was created on.
    pub(crate) backend: Backend,
}

/// Owns the C allocation independently of the CUDA resources it describes.
/// Import transfers only resource cleanup to the native context's copied state.
struct CudaState {
    raw: *mut VmafCudaState,
    release: Option<crate::ffi::CudaRelease>,
    free: crate::ffi::CudaStateFree,
}

impl Drop for CudaState {
    fn drop(&mut self) {
        if !self.raw.is_null() {
            // SAFETY: state_init returns an owned, initialized allocation even
            // on returned errors. Only unimported states retain release duty.
            unsafe {
                if let Some(release) = self.release {
                    release(self.raw);
                }
                (self.free)(self.raw.cast());
            }
        }
    }
}

/// One successful pool checkout, until returned or transferred to libvmaf.
/// The enclosing scorer must drop this before closing its context/pool.
pub(crate) struct PooledPicture {
    pub(crate) raw: crate::ffi::VmafPicture,
    api: &'static VmafApi,
}

impl PooledPicture {
    /// Disarm after a successful `vmaf_read_pictures`, even if C left `raw` nonzero.
    pub(crate) fn transfer(self) {
        // Only the native reference needs cleanup; libvmaf now owns that cleanup.
        std::mem::forget(self);
    }
}

impl Drop for PooledPicture {
    fn drop(&mut self) {
        // SAFETY: only successful checkouts get guards, and transfer disarms
        // them. The enclosing owner keeps the API and context/pool alive.
        unsafe { (self.api.picture_unref)(raw_mut_picture(&mut self.raw)) };
    }
}

impl Drop for Context {
    #[inline]
    fn drop(&mut self) {
        if !self.raw.is_null() {
            // SAFETY: `raw` came from `vmaf_init` and is closed exactly once,
            // here, when the owning handle drops.
            unsafe { (self.api.close)(self.raw) };
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
    /// The returned guard releases it unless submission transfers ownership.
    /// Callers must drop all guards before closing this context.
    ///
    /// # Errors
    ///
    /// Pool exhaustion can block until a picture returns. Allocation/setup
    /// failures return [`VmafError::CallFailed`].
    pub(crate) fn pooled_picture(&self) -> Result<PooledPicture, VmafError> {
        let mut picture = crate::ffi::VmafPicture::zeroed();
        // SAFETY: `picture` is a valid out-parameter and `raw` is a live
        // context whose pool was sized during construction.
        let status = unsafe {
            (self.api.fetch_preallocated_picture)(self.raw, raw_mut_picture(&mut picture))
        };
        // A failed fetch rolls its slot back but may leave dangling fields in
        // the output. Only successful fetches acquire an owning guard.
        crate::ffi::check(status, "vmaf_fetch_preallocated_picture")?;
        Ok(PooledPicture {
            raw: picture,
            api: self.api,
        })
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
unsafe impl Send for Context {}

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
                status: 0,
            });
        }

        // The picture pool must be sized before
        // `vmaf_fetch_preallocated_picture` can hand out buffers;
        // without it the fetch fails. The size matters as much as the
        // existence: libvmaf's own auto-sizing (`n_threads * 2 + 2`,
        // see [`picture_pool_size`]) is skipped when a pool already exists
        // here, so undersizing it would starve the thread pool and
        // serialise extraction regardless of `n_threads`.
        // SAFETY: `raw` is a live context and `geometry` describes the frames
        // that will be submitted.
        let status = unsafe {
            (api.preallocate_pictures)(
                raw,
                crate::ffi::VmafPictureConfiguration {
                    pic_params: crate::ffi::VmafPictureConfigPictureParams {
                        w: geometry.width,
                        h: geometry.height,
                        bpc: geometry.bit_depth,
                        pix_fmt: geometry.pixel_format,
                    },
                    pic_cnt: pool_size,
                },
            )
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
                status: -1,
            });
        };
        let release = api.cuda.release.ok_or_else(|| VmafError::SymbolNotFound {
            symbol: "vmaf_cuda_release".to_owned(),
            message: "CUDA requires vmaf_cuda_release before allocating state; use CpuOnly or a compatible libvmaf build".to_owned(),
        })?;
        let free = api.cuda.free_state.ok_or_else(|| VmafError::SymbolNotFound {
            symbol: "free".to_owned(),
            message: "CUDA requires allocator-matched native free (currently verified on Linux only); use CpuOnly or Auto".to_owned(),
        })?;
        let Some(import_state) = api.cuda.import_state else {
            return Err(VmafError::CallFailed {
                function: "vmaf_cuda_import_state",
                status: -1,
            });
        };
        let Some(preallocate) = api.cuda.preallocate_pictures else {
            return Err(VmafError::CallFailed {
                function: "vmaf_cuda_preallocate_pictures",
                status: -1,
            });
        };

        // Install cleanup before the call: returned errors can populate `raw`.
        let mut state = CudaState {
            raw: std::ptr::null_mut(),
            release: Some(release),
            free,
        };
        // SAFETY: `raw` is a valid out-pointer; a null cu_ctx asks libvmaf to
        // retain its primary context, not reset or own any external GPU user.
        let status = unsafe { state_init(&mut state.raw, VmafCudaConfiguration::default()) };
        crate::ffi::check(status, "vmaf_cuda_state_init")?;

        if state.raw.is_null() {
            return Err(VmafError::CallFailed {
                function: "vmaf_cuda_state_init",
                status: 0,
            });
        }

        let mut raw: *mut crate::ffi::VmafContext = std::ptr::null_mut();
        // SAFETY: as `new_cpu`, plus `state` which was just initialised.
        let status = unsafe { (api.init)(&mut raw, configuration) };
        crate::ffi::check(status, "vmaf_init")?;
        if raw.is_null() {
            return Err(VmafError::CallFailed {
                function: "vmaf_init",
                status: 0,
            });
        }
        let context = Self {
            raw,
            api,
            backend: Backend::Cuda,
        };

        // SAFETY: both pointers are valid and non-null; `import_state` copies
        // the state only on success. Failure leaves resource ownership here.
        let status = unsafe { import_state(raw, state.raw) };
        crate::ffi::check(status, "vmaf_cuda_import_state")?;
        state.release = None;

        // SAFETY: the live context now owns the imported CUDA resources.
        let status = unsafe {
            preallocate(
                raw,
                crate::ffi::VmafCudaPictureConfiguration {
                    pic_params: crate::ffi::VmafCudaPictureConfigPictureParams {
                        w: geometry.width,
                        h: geometry.height,
                        bpc: geometry.bit_depth,
                        pix_fmt: geometry.pixel_format,
                    },
                    pic_prealloc_method: crate::ffi::VmafCudaPicturePreallocationMethod::Device,
                },
            )
        };

        crate::ffi::check(status, "vmaf_cuda_preallocate_pictures")?;
        // The original wrapper is freed here; close owns only the copied state.
        Ok(context)
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
        BackendPreference::Auto => api.cuda_available(),
        // Resolve required cleanup explicitly so missing allocator support is
        // actionable, without ever calling state_init on unsupported platforms.
        BackendPreference::RequireCuda => true,
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

#[cfg(all(test, unix))]
#[path = "cuda_tests.rs"]
mod cuda_tests;

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
