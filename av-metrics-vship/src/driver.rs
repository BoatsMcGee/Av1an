//! Whether a GPU driver is present, established before libvship is opened.
//!
//! A Vulkan build of libvship declares `VkDeviceManagerList
//! globalVulkanInstance`, whose constructor calls `vkCreateInstance` and lets
//! upstream's failure `throw`. That runs during `dlopen`, and an exception
//! escaping a global constructor calls `std::terminate` -- so the process
//! aborts before any Rust code runs, and `is_available()` never gets to report
//! unavailability. With no driver: `Aborted (exit 134)`.
//!
//! So the loader is asked the same question first. It reports failure as a
//! return value rather than throwing, so a driverless machine yields a clean
//! "unavailable" and libvship is never opened. `Present` means the call
//! libvship is about to make will succeed, which is why the probe requests the
//! same API version libvship does: the loader downgrades a version it cannot
//! provide rather than failing, so a lower request would pass a driver libvship
//! rejects.
//!
//! The loader is asked rather than the filesystem because discovery is
//! per-platform (manifest directories on Linux, the registry on Windows,
//! MoltenVK bundles on macOS) and reimplementing it would drift from the
//! loader. It exports `vkCreateInstance`, so no Vulkan headers or build
//! dependency are needed.
//!
//! The CUDA and HIP builds do not throw from a global initialiser, so a machine
//! without Vulkan is not by itself grounds for refusing; see
//! [`DriverProbe::Unknown`].

use std::{
    ffi::{c_char, c_int, c_void},
    path::{Path, PathBuf},
    sync::OnceLock,
};

use libloading::Library;

/// `VK_STRUCTURE_TYPE_APPLICATION_INFO`.
const VK_STRUCTURE_TYPE_APPLICATION_INFO: c_int = 0;

/// `VK_STRUCTURE_TYPE_INSTANCE_CREATE_INFO`.
const VK_STRUCTURE_TYPE_INSTANCE_CREATE_INFO: c_int = 1;

/// `VK_SUCCESS`.
const VK_SUCCESS: c_int = 0;

/// `VK_API_VERSION_1_3`, matching libvship's request.
const VK_API_VERSION_1_3: u32 = (1 << 22) | (3 << 12);

/// Whether libvship may be opened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DriverProbe {
    /// An instance was created, so libvship's constructor will succeed.
    Present,

    /// A loader was found but refused to create an instance.
    Absent,

    /// No loader could be opened, so nothing was established. Permits the load:
    /// a machine with no Vulkan stack may still have a working CUDA build.
    Unknown,
}

impl DriverProbe {
    /// Whether libvship may be opened.
    #[inline]
    #[must_use]
    pub(crate) const fn permits_load(self) -> bool {
        !matches!(self, Self::Absent)
    }
}

// ABI-frozen by the spec, declared by hand as `ffi.rs` declares libvship's.
// Unnamed fields stay zeroed, which the spec defines as "not supplied".

/// `VkApplicationInfo`.
#[repr(C)]
struct VkApplicationInfo {
    s_type:              c_int,
    p_next:              *mut c_void,
    p_application_name:  *const c_char,
    application_version: u32,
    p_engine_name:       *const c_char,
    engine_version:      u32,
    api_version:         u32,
}

/// `VkInstanceCreateInfo`.
#[repr(C)]
struct VkInstanceCreateInfo {
    s_type:                     c_int,
    p_next:                     *mut c_void,
    flags:                      u32,
    p_application_info:         *const VkApplicationInfo,
    enabled_layer_count:        u32,
    pp_enabled_layer_names:     *const *const c_char,
    enabled_extension_count:    u32,
    pp_enabled_extension_names: *const *const c_char,
}

/// `vkCreateInstance`.
///
/// Three parameters, and the middle one is easy to omit, which shifts every
/// argument left and makes the loader write the instance through stack garbage.
/// Returns `VkResult`, not the handle: `VK_ERROR_*` is a small negative number,
/// which as a pointer is non-null and reads as success.
type PfnCreateInstance = unsafe extern "system" fn(
    *const VkInstanceCreateInfo,
    *const c_void,
    *mut *mut c_void,
) -> c_int;

/// `vkDestroyInstance`.
type PfnDestroyInstance = unsafe extern "system" fn(*mut c_void, *const c_void);

/// Loader names for this platform.
///
/// `VK_DRIVER_FILES` and `VK_ICD_FILENAMES` are not expanded: they name ICD
/// manifest files, not directories, and the loader reads them itself.
#[inline]
#[must_use]
fn loader_candidates() -> Vec<PathBuf> {
    let names: &[&str] = if cfg!(target_os = "windows") {
        &["vulkan-1.dll"]
    } else if cfg!(target_os = "macos") {
        &["libvulkan.1.dylib", "libMoltenVK.dylib", "libvulkan.dylib"]
    } else {
        &["libvulkan.so.1", "libvulkan.so"]
    };

    names.iter().map(PathBuf::from).collect()
}

/// Ask the loader whether a driver is usable.
fn probe_with(candidates: &[PathBuf]) -> DriverProbe {
    // Tracked separately: a loader that would not open says nothing about
    // drivers, while one that opened and declined does.
    let mut declinations = Vec::new();
    let mut open_failures = Vec::new();

    for candidate in candidates {
        // SAFETY: a Vulkan loader is a C library with no initialiser that can
        // throw, and every candidate is one of its names.
        let library = match unsafe { Library::new(candidate) } {
            Ok(library) => library,
            Err(error) => {
                open_failures.push(format!("{}: {error}", candidate.display()));
                continue;
            },
        };

        // Leaked: unloading the loader tears down its ICDs and then reaches into
        // them, which segfaults even after the instance is destroyed.
        let library = Box::leak(Box::new(library));

        // Every candidate is tried, so one unusable loader cannot shadow a
        // working one.
        match try_create_instance(library) {
            Ok(()) => return DriverProbe::Present,
            Err(reason) => declinations.push(format!("{}: {reason}", candidate.display())),
        }
    }

    if declinations.is_empty() {
        tracing::debug!(
            ?open_failures,
            "no Vulkan loader could be opened; libvship load is not gated"
        );
        return DriverProbe::Unknown;
    }

    tracing::debug!(?declinations, "the Vulkan loader found no usable driver");
    DriverProbe::Absent
}

/// [`probe_with`] over this platform's loader names.
#[inline]
#[must_use]
fn probe() -> DriverProbe {
    probe_with(&loader_candidates())
}

/// Create and destroy an instance through the loader.
///
/// `library` must outlive this call, which `probe_with` guarantees by leaking
/// it.
fn try_create_instance(library: &'static Library) -> Result<(), String> {
    // SAFETY: the signature is that of `vkCreateInstance`.
    let Ok(create) =
        (unsafe { library.get::<PfnCreateInstance>(b"vkCreateInstance\0").map(|symbol| *symbol) })
    else {
        return Err("no vkCreateInstance in the library".to_owned());
    };

    // NUL-terminated: Vulkan reads these as C strings. Both outlive the call.
    let application_name = b"av-metrics-vship\0";
    let engine_name = b"av-metrics-vship\0";

    let application_info = VkApplicationInfo {
        s_type:              VK_STRUCTURE_TYPE_APPLICATION_INFO,
        p_next:              std::ptr::null_mut(),
        p_application_name:  application_name.as_ptr().cast(),
        application_version: 0,
        p_engine_name:       engine_name.as_ptr().cast(),
        engine_version:      0,
        api_version:         VK_API_VERSION_1_3,
    };
    let create_info = VkInstanceCreateInfo {
        s_type:                     VK_STRUCTURE_TYPE_INSTANCE_CREATE_INFO,
        p_next:                     std::ptr::null_mut(),
        flags:                      0,
        p_application_info:         &application_info,
        enabled_layer_count:        0,
        pp_enabled_layer_names:     std::ptr::null(),
        enabled_extension_count:    0,
        pp_enabled_extension_names: std::ptr::null(),
    };

    let mut instance: *mut c_void = std::ptr::null_mut();
    // SAFETY: `create_info` is initialised and outlives the call, a null
    // allocator selects the loader's default, and the out-pointer is writable.
    let status = unsafe { create(&raw const create_info, std::ptr::null(), &raw mut instance) };

    if status != VK_SUCCESS {
        return Err(format!("vkCreateInstance returned VkResult {status}"));
    }
    if instance.is_null() {
        return Err("vkCreateInstance reported success but produced no instance".to_owned());
    }

    // Destroyed rather than leaked, so the probe does not hold driver resources
    // alongside libvship's own instance.
    //
    // SAFETY: `instance` came from `vkCreateInstance` and is destroyed once, with
    // `library` still mapped.
    unsafe {
        if let Ok(destroy) =
            library.get::<PfnDestroyInstance>(b"vkDestroyInstance\0").map(|symbol| *symbol)
        {
            destroy(instance, std::ptr::null());
        }
    }

    Ok(())
}

/// Whether `path` is a Vulkan build of libvship.
///
/// The backend cannot be read from a library without opening it, which is the
/// thing that may abort, so this looks for the loader's name in the file's
/// import table instead. A substring search over the file is conservative in
/// the safe direction: a Vulkan build always names the loader, and a false
/// positive only means the gate applies when it need not.
#[inline]
#[must_use]
pub(crate) fn is_vulkan_build(path: &Path) -> bool {
    let markers: &[&[u8]] = if cfg!(target_os = "windows") {
        &[b"vulkan-1.dll"]
    } else if cfg!(target_os = "macos") {
        &[b"libvulkan", b"libMoltenVK"]
    } else {
        &[b"libvulkan"]
    };

    // Unreadable means unopenable, so the ordinary open reports the real problem.
    let Ok(bytes) = std::fs::read(path) else {
        return false;
    };

    markers
        .iter()
        .any(|marker| bytes.windows(marker.len()).any(|window| window == *marker))
}

/// [`probe`], resolved once: a machine's driver configuration cannot change
/// while it runs, and `is_available` may be called repeatedly.
#[inline]
#[must_use]
pub(crate) fn probe_once() -> DriverProbe {
    static PROBE: OnceLock<DriverProbe> = OnceLock::new();
    *PROBE.get_or_init(probe)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Creating an instance wedges the loader at process exit, so the tests
    /// that do it are opt-in. Set this to run them on a machine with a
    /// driver.
    fn should_create_instance() -> bool {
        std::env::var_os("AV_METRICS_VSHIP_TEST_VULKAN_PROBE").is_some()
    }

    #[test]
    fn only_absence_refuses_the_load() {
        assert!(DriverProbe::Present.permits_load());
        assert!(DriverProbe::Unknown.permits_load());
        assert!(!DriverProbe::Absent.permits_load());
    }

    #[test]
    fn candidates_are_non_empty_on_every_platform() {
        // Naming no candidate would report every machine as driverless.
        assert!(!loader_candidates().is_empty());
    }

    #[test]
    fn a_missing_loader_is_unknown_and_permits_the_load() {
        let missing = vec![PathBuf::from("/nonexistent/libvulkan.so.999")];
        assert_eq!(probe_with(&missing), DriverProbe::Unknown);
        assert!(DriverProbe::Unknown.permits_load());
    }

    #[test]
    fn probing_is_total_and_cached() {
        if !should_create_instance() {
            return;
        }

        let first = probe_once();
        assert_eq!(probe_once(), first);
    }

    #[test]
    fn a_loader_without_a_driver_is_reported_absent() {
        if !should_create_instance() {
            return;
        }

        let loader_present = loader_candidates().iter().any(|candidate| {
            // SAFETY: the candidates are Vulkan loader names only.
            unsafe { Library::new(candidate) }.is_ok()
        });
        if !loader_present {
            return;
        }

        match probe() {
            // A loader was found, so the verdict must be definite.
            DriverProbe::Present | DriverProbe::Absent => {},
            DriverProbe::Unknown => panic!("a loader was found, so the probe must be definite"),
        }
    }

    #[test]
    fn only_a_library_naming_the_loader_counts_as_a_vulkan_build() {
        let directory = tempfile::tempdir().expect("a temporary directory");

        let marker: &[u8] = if cfg!(target_os = "windows") {
            b"vulkan-1.dll"
        } else if cfg!(target_os = "macos") {
            b"libMoltenVK"
        } else {
            b"libvulkan.so.1"
        };

        let vulkan = directory.path().join("libvship.so");
        let mut vulkan_bytes = b"\x7fELF pretend shared object importing ".to_vec();
        vulkan_bytes.extend_from_slice(marker);
        std::fs::write(&vulkan, &vulkan_bytes).expect("write");
        assert!(
            is_vulkan_build(&vulkan),
            "a library importing the loader is Vulkan"
        );

        let cuda = directory.path().join("libvship-cuda.so");
        std::fs::write(
            &cuda,
            b"\x7fELF pretend shared object importing libcudart.so.12",
        )
        .expect("write");
        assert!(
            !is_vulkan_build(&cuda),
            "a CUDA build must not be gated on a missing Vulkan driver"
        );

        assert!(!is_vulkan_build(Path::new("/nonexistent/libvship.so")));
    }

    #[test]
    fn the_structures_match_the_abi_the_loader_expects() {
        // Offsets, not sizes: the total also encodes padding that says nothing
        // about field order.
        assert_eq!(std::mem::offset_of!(VkApplicationInfo, s_type), 0);
        assert_eq!(std::mem::offset_of!(VkApplicationInfo, p_next), 8);
        assert_eq!(
            std::mem::offset_of!(VkApplicationInfo, p_application_name),
            16
        );
        assert_eq!(
            std::mem::offset_of!(VkApplicationInfo, application_version),
            24
        );
        assert_eq!(std::mem::offset_of!(VkApplicationInfo, p_engine_name), 32);
        assert_eq!(std::mem::offset_of!(VkApplicationInfo, engine_version), 40);
        assert_eq!(std::mem::offset_of!(VkApplicationInfo, api_version), 44);

        assert_eq!(std::mem::offset_of!(VkInstanceCreateInfo, s_type), 0);
        assert_eq!(std::mem::offset_of!(VkInstanceCreateInfo, p_next), 8);
        assert_eq!(std::mem::offset_of!(VkInstanceCreateInfo, flags), 16);
        assert_eq!(
            std::mem::offset_of!(VkInstanceCreateInfo, p_application_info),
            24
        );
        assert_eq!(
            std::mem::offset_of!(VkInstanceCreateInfo, enabled_layer_count),
            32
        );
        assert_eq!(
            std::mem::offset_of!(VkInstanceCreateInfo, pp_enabled_layer_names),
            40
        );
        assert_eq!(
            std::mem::offset_of!(VkInstanceCreateInfo, enabled_extension_count),
            48
        );
        assert_eq!(
            std::mem::offset_of!(VkInstanceCreateInfo, pp_enabled_extension_names),
            56
        );
    }
}
