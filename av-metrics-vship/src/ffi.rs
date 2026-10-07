//! Hand-written declarations for the libvship C API.
//!
//! libvship is opened with [`libloading`] rather than linked, so the crate has
//! no build-time dependency on it and compiles on machines where it is absent.
//! No `bindgen` is used: the surface needed here is small and stable, and
//! declaring it by hand keeps the build free of a libclang dependency.
//!
//! Every struct layout, enum discriminant and function signature below was
//! transcribed from `src/VshipAPI.h` and `src/VshipColor.h` in the Vship
//! repository at tag v5.1.2, not inferred.
//!
//! # Loading strategy
//!
//! The [`Library`] is deliberately leaked by [`VshipApi::load`], so the code
//! its function pointers refer to stays mapped for the life of the process.
//! Only the pointers are kept in [`VshipApi`]; since raw `fn` pointers are
//! `Send + Sync`, the resolved API can then be cached in a `OnceLock` and
//! shared freely.
//!
//! # Backends
//!
//! libvship is compiled per-backend. Since v5.1.2, Vulkan initialization is
//! lazy and a missing driver is reported through the library's API; other
//! backends may still fail to load when their runtime libraries are missing.

use std::{
    ffi::{c_char, c_int, c_void},
    path::{Path, PathBuf},
    sync::OnceLock,
};

use libloading::Library;

use crate::error::VshipError;

/// A libvship metric handler: an opaque pointer allocated by
/// `Vship_InitHandler`.
pub type VshipHandle = *mut c_void;

/// Pixel sample type.
///
/// The discriminants are **non-contiguous**: 9, 10, 12, 14 and 16 bits occupy
/// values 3, 5, 7, 9 and 11, leaving 4, 6, 8 and 10 for formats the C header
/// does not name. Every variant therefore carries an explicit value, and none
/// of these types may be assigned a `#[repr(C)]` sequential conversion.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VshipSample {
    /// 32-bit float samples.
    Float = 0,
    /// 16-bit half-float samples.
    Half = 1,
    /// 8-bit unsigned integer samples.
    Uint8 = 2,
    /// 9-bit unsigned integer samples.
    Uint9 = 3,
    /// 10-bit unsigned integer samples.
    Uint10 = 5,
    /// 12-bit unsigned integer samples.
    Uint12 = 7,
    /// 14-bit unsigned integer samples.
    Uint14 = 9,
    /// 16-bit unsigned integer samples.
    Uint16 = 11,
}

/// Luma/chroma sample range.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VshipRange {
    /// Limited (studio) range.
    Limited = 0,
    /// Full range.
    Full = 1,
}

/// Horizontal and vertical chroma decimation, as exponents.
///
/// `{1, 1}` is 4:2:0, `{1, 0}` is 4:2:2 and `{0, 0}` is 4:4:4.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct VshipChromaSubsample {
    /// Horizontal decimation exponent: chroma width is `width >> subw`.
    pub subw: c_int,
    /// Vertical decimation exponent: chroma height is `height >> subh`.
    pub subh: c_int,
}

impl VshipChromaSubsample {
    /// 4:2:0 chroma.
    #[inline]
    #[must_use]
    pub const fn yuv420() -> Self {
        Self {
            subw: 1, subh: 1
        }
    }

    /// 4:2:2 chroma.
    #[inline]
    #[must_use]
    pub const fn yuv422() -> Self {
        Self {
            subw: 1, subh: 0
        }
    }

    /// 4:4:4 chroma.
    #[inline]
    #[must_use]
    pub const fn yuv444() -> Self {
        Self {
            subw: 0, subh: 0
        }
    }
}

/// Chroma siting relative to the luma samples.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VshipChromaLocation {
    /// Left-sited, as MPEG-2 specifies.
    Left = 0,
    /// Centred.
    Center = 1,
    /// Top-left.
    TopLeft = 2,
    /// Top, as JPEG/MPEG-1 specifies.
    Top = 3,
}

/// Colour family.
///
/// The C header marks this deprecated and states that `YUVMatrix` alone decides
/// the family, with matrix 0 meaning RGB. It is still present in the struct, so
/// it is declared here and always set to [`Self::YUV`].
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VshipColorFamily {
    /// YUV content, which is all this crate produces.
    YUV = 0,
    /// RGB content.
    RGB = 1,
}

/// YUV to RGB matrix coefficients.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code, reason = "variants mirror the C enum for FFI fidelity")]
pub enum VshipYuvMatrix {
    /// Identity: the samples are already RGB.
    Rgb = 0,
    /// BT.709, correct for HD and above.
    Bt709 = 1,
    /// BT.470 system B/G, a.k.a. BT.601.
    Bt470Bg = 5,
    /// SMPTE 170M, identical coefficients to [`Self::Bt470Bg`].
    St170M = 6,
    /// YCgCo, as used by HEVC RExt.
    Ycgco = 8,
    /// BT.2020 non-constant luminance.
    Bt2020Ncl = 9,
    /// BT.2020 constant luminance.
    Bt2020Cl = 10,
    /// BT.2100 ICTCP.
    Bt2100Ictcp = 14,
    /// YCgCo-Re, as used by Dolby Vision RPU level 6.
    YcgcoRe = 16,
    /// YCgCo-Ro, as used by Dolby Vision RPU level 7.
    YcgcoRo = 17,
}

/// Electro-optical transfer function.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code, reason = "variants mirror the C enum for FFI fidelity")]
pub enum VshipTransferFunction {
    /// BT.709, whose curve also serves BT.601 and BT.2020 at 8 bits.
    Bt709 = 1,
    /// BT.470 system M (NTSC).
    Bt470M = 4,
    /// BT.470 system B/G.
    Bt470Bg = 5,
    /// BT.601.
    Bt601 = 6,
    /// SMPTE ST 240M.
    St240M = 7,
    /// Linear light.
    Linear = 8,
    /// sRGB.
    SRgb = 13,
    /// SMPTE ST 2084 (PQ).
    Pq = 16,
    /// SMPTE ST 428.
    St428 = 17,
    /// ARIB STD-B67 (HLG).
    Hlg = 18,
}

/// Colorimetry primaries.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VshipPrimaries {
    /// XYZ, libvship's internal working space.
    Internal = -1,
    /// BT.709, a.k.a. sRGB and HDTV.
    Bt709 = 1,
    /// BT.470 system M (NTSC).
    Bt470M = 4,
    /// BT.470 system B/G.
    Bt470Bg = 5,
    /// SMPTE 170M (NTSC).
    St170M = 6,
    /// SMPTE ST 240M, identical coefficients to [`Self::St170M`].
    St240M = 7,
    /// BT.2020.
    Bt2020 = 9,
    /// DCI-P3 with D65 white point.
    DisplayP3 = 12,
}

/// Region excluded from processing, in luma rows and columns.
///
/// Applied *after* any scaling, so a non-zero crop reduces the final picture
/// dimensions.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct VshipCropRectangle {
    /// Rows removed from the top.
    pub top:    c_int,
    /// Rows removed from the bottom.
    pub bottom: c_int,
    /// Columns removed from the left.
    pub left:   c_int,
    /// Columns removed from the right.
    pub right:  c_int,
}

/// A complete description of one input picture.
///
/// Passed by value across the FFI boundary and holding only integers and enums,
/// so `Copy` is sound.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VshipColorspace {
    /// Luma width in pixels.
    pub width:             i64,
    /// Luma height in pixels.
    pub height:            i64,
    /// Width to scale the picture to before cropping, or -1 for no scaling.
    pub target_width:      i64,
    /// Height to scale the picture to before cropping, or -1 for no scaling.
    pub target_height:     i64,
    /// Sample type.
    pub sample:            VshipSample,
    /// Luma/chroma range.
    pub range:             VshipRange,
    /// Chroma subsampling.
    pub subsampling:       VshipChromaSubsample,
    /// Chroma siting.
    pub chroma_location:   VshipChromaLocation,
    /// Colour family. Deprecated in the C header; [`Self::yuv_matrix`] decides.
    pub color_family:      VshipColorFamily,
    /// YUV to RGB matrix.
    pub yuv_matrix:        VshipYuvMatrix,
    /// Transfer function.
    pub transfer_function: VshipTransferFunction,
    /// Primaries.
    pub primaries:         VshipPrimaries,
    /// Cropping applied after any scaling.
    pub crop:              VshipCropRectangle,
}

impl VshipColorspace {
    /// A BT.709 limited-range 8-bit 4:2:0 space of the given size, which is the
    /// C header's own documented default.
    #[inline]
    #[must_use]
    pub const fn bt709_default(width: i64, height: i64) -> Self {
        Self {
            width,
            height,
            target_width: -1,
            target_height: -1,
            sample: VshipSample::Uint8,
            range: VshipRange::Limited,
            subsampling: VshipChromaSubsample {
                subw: 1, subh: 1
            },
            chroma_location: VshipChromaLocation::Left,
            color_family: VshipColorFamily::YUV,
            yuv_matrix: VshipYuvMatrix::Bt709,
            transfer_function: VshipTransferFunction::Bt709,
            primaries: VshipPrimaries::Bt709,
            crop: VshipCropRectangle {
                top:    0,
                bottom: 0,
                left:   0,
                right:  0,
            },
        }
    }
}

/// The compute backend libvship was compiled for.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VshipBackend {
    /// AMD ROCm.
    Hip = 0,
    /// NVIDIA CUDA.
    Cuda = 1,
    /// Vulkan.
    Vulkan = 2,
    /// CPU only.
    Cpu = 3,
}

impl VshipBackend {
    /// The backend's name.
    #[inline]
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Hip => "HIP",
            Self::Cuda => "CUDA",
            Self::Vulkan => "Vulkan",
            Self::Cpu => "CPU",
        }
    }
}

/// The libvship version and the backend it was built against.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VshipVersion {
    /// Major version.
    pub major:       c_int,
    /// Minor version.
    pub minor:       c_int,
    /// Patch version.
    pub minor_minor: c_int,
    /// Backend this build targets.
    pub backend:     VshipBackend,
}

/// A status code returned by every libvship entry point.
///
/// The C enum is grouped by cause but its numeric values are scattered, so
/// every variant carries an explicit value.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code, reason = "variants mirror the C enum for FFI fidelity")]
pub enum VshipException {
    /// Success.
    NoError = 0,
    /// A VRAM allocation failed.
    OutOfVram = 1,
    /// A RAM allocation failed.
    OutOfRam = 2,
    /// The input colorspace names an unknown display model.
    BadDisplayModel = 3,
    /// The two input colorspaces are incompatible with each other.
    DifferingInputType = 4,
    /// A non-RGBS input survived conversion, which should be impossible.
    NonRgbsInput = 5,
    /// Enumerating devices failed.
    DeviceCountError = 6,
    /// No usable device was found.
    NoDeviceDetected = 7,
    /// The device index is out of range.
    BadDeviceArgument = 8,
    /// The device is of an unusable kind.
    BadDeviceCode = 9,
    /// The handler is invalid.
    BadHandler = 10,
    /// A required pointer was null.
    BadPointer = 11,
    /// A ROCm error occurred.
    HipError = 12,
    /// A filesystem path was invalid.
    BadPath = 13,
    /// A JSON document was invalid.
    BadJson = 14,
    /// The operation is unsupported on this backend.
    NotSupported = 15,
    /// A struct's discriminator did not match the function that received it.
    BadInputStructType = 16,
    /// An error value outside the documented range was passed in.
    BadErrorType = 17,
}

impl VshipException {
    /// Whether this status indicates success.
    #[inline]
    #[must_use]
    pub const fn is_ok(self) -> bool {
        matches!(self, Self::NoError)
    }
}

/// Discriminator selecting which init or score struct is being passed.
///
/// `Vship_InitHandler` and `Vship_ComputeHandler` are metric-agnostic: the
/// `structType` field of the struct behind their `void *` selects the variant.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code, reason = "variants mirror the C enum for FFI fidelity")]
pub enum VshipStructType {
    /// Reserved, never passed.
    Null = 0,
    /// `Vship_InitSSIMULACRA2_1`.
    InitSsimulacra2 = 1,
    /// `Vship_InitButteraugli_1`.
    InitButteraugli = 2,
    /// `Vship_InitCVVDP_1`.
    InitCvvdp = 3,
    /// `Vship_ScoreSSIMULACRA2`.
    ScoreSsimulacra2 = 4,
    /// `Vship_ScoreButteraugli`.
    ScoreButteraugli = 5,
    /// `Vship_ScoreCVVDP`.
    ScoreCvvdp = 6,
}

/// Initialisation arguments for SSIMULACRA2.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct VshipInitSsimulacra2_1 {
    /// Must be [`VshipStructType::InitSsimulacra2`].
    pub struct_type:    VshipStructType,
    /// Colorspace of the reference frames.
    pub src_colorspace: VshipColorspace,
    /// Colorspace of the distorted frames.
    pub dis_colorspace: VshipColorspace,
    /// Device index, as accepted by `Vship_GPUFullCheck`.
    pub gpu_id:         c_int,
}

/// Initialisation arguments for Butteraugli.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct VshipInitButteraugli_1 {
    /// Must be [`VshipStructType::InitButteraugli`].
    pub struct_type:          VshipStructType,
    /// Colorspace of the reference frames.
    pub src_colorspace:       VshipColorspace,
    /// Colorspace of the distorted frames.
    pub dis_colorspace:       VshipColorspace,
    /// Exponent of the norm to minimise. 2 by convention.
    pub q_norm:               c_int,
    /// Peak display brightness in nits, which scales the metric's intensity
    /// weighting.
    pub intensity_multiplier: f32,
    /// Device index, as accepted by `Vship_GPUFullCheck`.
    pub gpu_id:               c_int,
}

/// Initialisation arguments for CVVDP.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct VshipInitCvvdp_1 {
    /// Must be [`VshipStructType::InitCvvdp`].
    pub struct_type:            VshipStructType,
    /// Colorspace of the reference frames.
    pub src_colorspace:         VshipColorspace,
    /// Colorspace of the distorted frames.
    pub dis_colorspace:         VshipColorspace,
    /// Frame rate in frames per second, which sets the motion speed the
    /// temporal model assumes.
    pub fps:                    f32,
    /// Whether to scale to the display resolution named by the model.
    pub resize_to_display:      bool,
    /// NUL-terminated model name, e.g. `standard_fhd`.
    pub model_key_cstr:         *const c_char,
    /// NUL-terminated path to a JSON display-configuration override, or null
    /// for none.
    pub model_config_json_cstr: *const c_char,
    /// Device index, as accepted by `Vship_GPUFullCheck`.
    pub gpu_id:                 c_int,
}

/// Output of `Vship_ComputeHandler` for SSIMULACRA2.
///
/// Populated by the library.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct VshipScoreSsimulacra2 {
    /// Must be [`VshipStructType::ScoreSsimulacra2`]. The library checks it.
    pub struct_type: VshipStructType,
    /// Written by libvship. Higher is better, and 1.0 is the maximum for
    /// identical inputs.
    pub score:       f64,
}

impl Default for VshipScoreSsimulacra2 {
    #[inline]
    fn default() -> Self {
        Self {
            struct_type: VshipStructType::ScoreSsimulacra2,
            score:       0.0,
        }
    }
}

/// Output of `Vship_ComputeHandler` for Butteraugli.
///
/// The distortion map is never retrieved: `dstp` stays null, which the C header
/// defines as meaning the map is not copied back from the device.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct VshipScoreButteraugli {
    /// Must be [`VshipStructType::ScoreButteraugli`]. The library checks it.
    pub struct_type: VshipStructType,
    /// Written by libvship. The score for the configured `Qnorm`.
    pub norm_q:      f64,
    /// Written by libvship. The L3 norm of the difference.
    pub norm_3:      f64,
    /// Written by libvship. The Linfinity norm of the difference.
    pub norm_inf:    f64,
    /// Always null: this crate does not retrieve distortion maps.
    pub dstp:        *const u8,
    /// Unused while `dstp` is null.
    pub dst_stride:  i64,
}

impl Default for VshipScoreButteraugli {
    #[inline]
    fn default() -> Self {
        Self {
            struct_type: VshipStructType::ScoreButteraugli,
            norm_q:      0.0,
            norm_3:      0.0,
            norm_inf:    0.0,
            dstp:        std::ptr::null(),
            dst_stride:  0,
        }
    }
}

/// Output of `Vship_ComputeHandler` for CVVDP.
///
/// The CVVDP score accumulates over every frame the handler has seen, so it is
/// a running mean rather than a per-frame value. The distortion map is never
/// retrieved: `dstp` stays null.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct VshipScoreCvvdp {
    /// Must be [`VshipStructType::ScoreCvvdp`]. The library checks it.
    pub struct_type: VshipStructType,
    /// Written by libvship. Higher is better.
    pub score:       f64,
    /// Always null: this crate does not retrieve distortion maps.
    pub dstp:        *const u8,
    /// Unused while `dstp` is null.
    pub dst_stride:  i64,
}

impl Default for VshipScoreCvvdp {
    #[inline]
    fn default() -> Self {
        Self {
            struct_type: VshipStructType::ScoreCvvdp,
            score:       0.0,
            dstp:        std::ptr::null(),
            dst_stride:  0,
        }
    }
}

type GetVersion = unsafe extern "C" fn() -> VshipVersion;
type GetDeviceCount = unsafe extern "C" fn(*mut c_int) -> VshipException;
type GetDeviceInfo = unsafe extern "C" fn(*mut VshipDeviceInfo, c_int) -> VshipException;
type GpuFullCheck = unsafe extern "C" fn(c_int) -> VshipException;
type GetErrorMessage = unsafe extern "C" fn(VshipException, *mut c_char, c_int) -> c_int;
type GetDetailedLastError = unsafe extern "C" fn(*mut c_char, c_int) -> c_int;
type GetDetailedLastErrorHandler = unsafe extern "C" fn(VshipHandle, *mut c_char, c_int) -> c_int;
type InitHandler = unsafe extern "C" fn(*mut VshipHandle, *mut c_void) -> VshipException;
type FreeHandler = unsafe extern "C" fn(VshipHandle) -> VshipException;
type ComputeHandler = unsafe extern "C" fn(
    VshipHandle,
    *mut c_void,
    *const *const u8,
    *const *const u8,
    *const i64,
    *const i64,
) -> VshipException;
type Reset = unsafe extern "C" fn(VshipHandle) -> VshipException;
type ResetScore = unsafe extern "C" fn(VshipHandle) -> VshipException;

/// Allocates host-pinned memory for plane uploads.
///
/// A buffer allocated this way is only worth it when it is reused across
/// frames, since the pinning itself is the expensive part.
type PinnedMalloc2 = unsafe extern "C" fn(*mut *mut c_void, u64, c_int) -> VshipException;

/// Releases memory from [`PinnedMalloc2`].
type PinnedFree2 = unsafe extern "C" fn(*mut c_void, c_int) -> VshipException;

/// Vulkan-specific device capabilities.
///
/// Declared because it is embedded by value in [`VshipDeviceInfo`], so the
/// trailing padding it induces is part of that struct's size.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
#[allow(
    dead_code,
    reason = "only the layout matters; the C struct carries reserved fields"
)]
pub struct VshipVulkanFeatureMatrix {
    /// 16-bit storage buffer access.
    pub storage_buffer_16_bit_access: bool,
    /// 16-bit uniform and storage buffer access.
    pub uniform_and_storage_buffer_16_bit_access: bool,
    /// Variable pointers in storage buffers.
    pub variable_pointers_storage_buffer: bool,
    /// Variable pointers in shaders.
    pub variable_pointers: bool,
    /// 8-bit storage push constants.
    pub storage_push_constant_8: bool,
    /// 8-bit storage buffer access.
    pub storage_buffer_8_bit_access: bool,
    /// 8-bit uniform and storage buffer access.
    pub uniform_and_storage_buffer_8_bit_access: bool,
    /// Float16 shaders.
    pub shader_float_16: bool,
    /// Float64 shaders.
    pub shader_float_64: bool,
    /// Int8 shaders.
    pub shader_int_8: bool,
    /// Int16 shaders.
    pub shader_int_16: bool,
    /// Int64 shaders.
    pub shader_int_64: bool,
    /// Buffer device address.
    pub buffer_device_address: bool,
    /// Dynamic indexing of shader storage buffer arrays.
    pub shader_storage_buffer_array_dynamic_indexing: bool,
    /// Synchronization2.
    pub synchronization_2: bool,
    /// LogicOp.
    pub logic_op: bool,
    /// Reserved for future use.
    pub future_features: [bool; 10],
}

impl Default for VshipVulkanFeatureMatrix {
    #[inline]
    fn default() -> Self {
        Self {
            storage_buffer_16_bit_access: false,
            uniform_and_storage_buffer_16_bit_access: false,
            variable_pointers_storage_buffer: false,
            variable_pointers: false,
            storage_push_constant_8: false,
            storage_buffer_8_bit_access: false,
            uniform_and_storage_buffer_8_bit_access: false,
            shader_float_16: false,
            shader_float_64: false,
            shader_int_8: false,
            shader_int_16: false,
            shader_int_64: false,
            buffer_device_address: false,
            shader_storage_buffer_array_dynamic_indexing: false,
            synchronization_2: false,
            logic_op: false,
            future_features: [false; 10],
        }
    }
}

/// What libvship reports about a compute device.
///
/// `Default` is implemented by hand because `[c_char; 256]` exceeds the array
/// length `Default` covers for arbitrary `T`.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct VshipDeviceInfo {
    /// Device name, NUL-padded.
    pub name:                  [c_char; 256],
    /// VRAM size in bytes.
    pub vram_size:             u64,
    /// Non-zero when the device is integrated.
    pub integrated:            c_int,
    /// Streaming multiprocessor count.
    pub multi_processor_count: c_int,
    /// Warp size.
    pub warp_size:             c_int,
    /// Only populated for the Vulkan backend.
    pub vulkan_feature_matrix: VshipVulkanFeatureMatrix,
}

impl VshipDeviceInfo {
    /// The device name with its NUL padding trimmed.
    ///
    /// The C header declares `name` as `char`, so each element is a byte in
    /// practice; a name that is not valid UTF-8 is lossily converted rather
    /// than treated as an error, since it is diagnostic output only.
    #[inline]
    #[must_use]
    pub fn name(&self) -> String {
        let end = self.name.iter().position(|&byte| byte == 0).unwrap_or(self.name.len());

        // The C `char` is signed, so reinterpreting each element as a byte is
        // required rather than optional; a name byte above 0x7F is negative as
        // `c_char`.
        let bytes: Vec<u8> = self.name[..end].iter().map(|&byte| byte as u8).collect();

        String::from_utf8_lossy(&bytes).into_owned()
    }
}

/// A `VshipDeviceInfo` before libvship has filled one in.
///
/// Hand-written because `[c_char; 256]` exceeds the array length `Default`
/// covers for an arbitrary `T`.
impl Default for VshipDeviceInfo {
    #[inline]
    fn default() -> Self {
        Self {
            // NUL-filled, which `name` trims to an empty string.
            name:                  [0; 256],
            vram_size:             0,
            integrated:            0,
            multi_processor_count: 0,
            warp_size:             0,
            vulkan_feature_matrix: VshipVulkanFeatureMatrix::default(),
        }
    }
}

/// Resolved libvship entry points.
///
/// Contains only function pointers and copyable data, so it is `Send + Sync`
/// and can be cached process-wide. The [`Library`] backing these pointers is
/// leaked by [`VshipApi::load`] and never unloaded.
#[derive(Clone, Copy)]
pub struct VshipApi {
    /// Reports the library version and backend.
    pub get_version:                     GetVersion,
    /// Counts the usable compute devices.
    pub get_device_count:                GetDeviceCount,
    /// Describes one compute device.
    pub get_device_info:                 GetDeviceInfo,
    /// Verifies a device can actually run libvship.
    pub gpu_full_check:                  GpuFullCheck,
    /// Expands a status code into its message.
    pub get_error_message:               GetErrorMessage,
    /// Retrieves the process-wide last error.
    pub get_detailed_last_error:         GetDetailedLastError,
    /// Retrieves one handler's last error.
    pub get_detailed_last_error_handler: GetDetailedLastErrorHandler,
    /// Creates a metric handler for any metric.
    pub init_handler:                    InitHandler,
    /// Destroys a metric handler.
    pub free_handler:                    FreeHandler,
    /// Scores one reference/distorted pair.
    pub compute_handler:                 ComputeHandler,
    /// Clears a temporal metric's frame history.
    pub reset:                           Reset,
    /// Clears a temporal metric's accumulated score.
    pub reset_score:                     ResetScore,
    /// Allocates host-pinned memory for plane uploads.
    pub pinned_malloc2:                  Option<PinnedMalloc2>,
    /// Releases memory from `pinned_malloc2`.
    pub pinned_free2:                    Option<PinnedFree2>,
}

impl VshipApi {
    /// The libvship version, formatted as `major.minor.minorMinor`.
    #[inline]
    #[must_use]
    pub fn version_string(&self) -> String {
        // SAFETY: `get_version` takes no arguments and only reads the library's
        // own constant version data.
        let version = unsafe { (self.get_version)() };
        format!(
            "{}.{}.{}",
            version.major, version.minor, version.minor_minor
        )
    }

    /// The backend this libvship build targets.
    #[inline]
    #[must_use]
    pub fn backend(&self) -> VshipBackend {
        // SAFETY: as `version_string`.
        unsafe { (self.get_version)() }.backend
    }

    /// The number of compute devices libvship can see.
    ///
    /// # Errors
    ///
    /// Returns [`VshipError::CallFailed`] if enumeration fails, which libvship
    /// reports rather than leaving the count meaningful.
    #[inline]
    pub fn device_count(&self) -> Result<u32, VshipError> {
        let mut count: c_int = 0;
        // SAFETY: `count` is a writable out-pointer for one `c_int`, which is
        // exactly what `Vship_GetDeviceCount` writes.
        let status = unsafe { (self.get_device_count)(&raw mut count) };
        check(status, "Vship_GetDeviceCount")?;

        u32::try_from(count).map_err(|_| VshipError::DeviceCount {
            count,
        })
    }

    /// What libvship reports about one device.
    ///
    /// # Errors
    ///
    /// Returns [`VshipError::CallFailed`] if `gpu_id` is not a valid index.
    #[inline]
    pub fn device_info(&self, gpu_id: u32) -> Result<VshipDeviceInfo, VshipError> {
        let mut info = VshipDeviceInfo::default();
        // SAFETY: `info` is a writable `VshipDeviceInfo` the library fills in
        // place, and `gpu_id` is narrowed from a value libvship enumerated.
        let status = unsafe { (self.get_device_info)(&raw mut info, gpu_id as c_int) };
        check(status, "Vship_GetDeviceInfo")?;

        Ok(info)
    }

    /// Whether a device can actually run libvship.
    ///
    /// This is the C API's own combined check: the header describes it as the
    /// useful function for seeing whether vship will work at all, with several
    /// distinct failures possible. It confirms the device after the library
    /// loads.
    ///
    /// # Errors
    ///
    /// Returns [`VshipError::CallFailed`] carrying libvship's own message when
    /// the device is unusable.
    #[inline]
    pub fn gpu_full_check(&self, gpu_id: u32) -> Result<(), VshipError> {
        // SAFETY: `gpu_id` was narrowed from an enumerated index, and the
        // function takes no pointers.
        let status = unsafe { (self.gpu_full_check)(gpu_id as c_int) };
        check(status, "Vship_GPUFullCheck")
    }

    /// Load libvship and resolve the entry points the scorer needs.
    ///
    /// The `Library` is intentionally leaked: libvship has no meaningful global
    /// teardown, and the pointers stored in the returned [`VshipApi`] must
    /// remain valid for the life of the process.
    ///
    /// The result is cached, so repeated calls are cheap.
    ///
    /// # Errors
    ///
    /// Returns [`VshipError::Load`] when no candidate library could be opened
    /// or a required entry point is missing, the latter indicating an
    /// incompatible libvship build.
    #[inline]
    pub fn load() -> Result<&'static Self, VshipError> {
        static API: OnceLock<Result<VshipApi, VshipException>> = OnceLock::new();

        match API.get_or_init(resolve) {
            Ok(api) => Ok(api),
            Err(exception) => Err(VshipError::LibraryNotFound {
                reason: format!(
                    "libvship could not be loaded ({}); tried {}",
                    *exception as c_int,
                    library_candidates()
                        .iter()
                        .map(|candidate| candidate.display().to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            }),
        }
    }
}

/// Check a libvship status code, mapping a failure to
/// [`VshipError::CallFailed`].
#[inline]
pub fn check(status: VshipException, function: &'static str) -> Result<(), VshipError> {
    if status.is_ok() {
        Ok(())
    } else {
        Err(VshipError::CallFailed {
            function,
            status: status as c_int,
            message: error_message(status),
        })
    }
}

/// The short message libvship associates with a status code.
///
/// The header's two-buffer protocol, where a zero `len` reports the required
/// size, exists to size a caller-owned buffer. A fixed one is used here, so a
/// message longer than the buffer is simply truncated.
#[inline]
#[must_use]
pub fn error_message(exception: VshipException) -> String {
    /// Buffer size for a status code's message.
    const CAPACITY: usize = 1024;

    // A loaded library is required to translate the code, so an unloadable one
    // falls back to the numeric status.
    let Ok(api) = VshipApi::load() else {
        return fallback_message(exception);
    };

    let mut buffer = vec![0 as c_char; CAPACITY];
    // SAFETY: `exception` is a valid status code, and `buffer` is a writable
    // `CAPACITY`-element array matching the length passed alongside it.
    let written =
        unsafe { (api.get_error_message)(exception, buffer.as_mut_ptr(), CAPACITY as c_int) };
    if written <= 0 {
        return fallback_message(exception);
    }

    // SAFETY: a non-positive return means libvship wrote nothing, so the buffer
    // is untouched; otherwise it wrote a NUL-terminated string into the
    // capacity it was given.
    let text = unsafe { std::ffi::CStr::from_ptr(buffer.as_ptr()) }
        .to_string_lossy()
        .into_owned();

    if text.is_empty() {
        fallback_message(exception)
    } else {
        text
    }
}

/// The message used when libvship cannot be asked about a status code.
#[inline]
#[must_use]
fn fallback_message(exception: VshipException) -> String {
    format!("libvship error {}", exception as c_int)
}

/// Resolve one of the candidate libvship libraries.
fn resolve() -> Result<VshipApi, VshipException> {
    let library = open_library()?;
    // Keep the mapping alive for the life of the process. The pointers resolved
    // below are only valid while it is loaded.
    let library = Box::leak(Box::new(library));

    // Each `required` call checks for the symbol's presence before its result is
    // dereferenced, and `T` is written to match the C signature of `name`. The
    // leaked `library` above keeps the code these point at mapped.
    Ok(VshipApi {
        get_version:                     required(library, b"Vship_GetVersion\0")?,
        get_device_count:                required(library, b"Vship_GetDeviceCount\0")?,
        get_device_info:                 required(library, b"Vship_GetDeviceInfo\0")?,
        gpu_full_check:                  required(library, b"Vship_GPUFullCheck\0")?,
        get_error_message:               required(library, b"Vship_GetErrorMessage\0")?,
        get_detailed_last_error:         required(library, b"Vship_GetDetailedLastError\0")?,
        get_detailed_last_error_handler: required(library, b"Vship_GetDetailedLastErrorHandler\0")?,
        init_handler:                    required(library, b"Vship_InitHandler\0")?,
        free_handler:                    required(library, b"Vship_FreeHandler\0")?,
        compute_handler:                 required(library, b"Vship_ComputeHandler\0")?,
        reset:                           required(library, b"Vship_Reset\0")?,
        reset_score:                     required(library, b"Vship_ResetScore\0")?,
        // Optional: an older or trimmed build may not export the pinned-memory
        // entry points, in which case planes are uploaded from decoder memory.
        pinned_malloc2:                  optional(library, b"Vship_PinnedMalloc2\0"),
        pinned_free2:                    optional(library, b"Vship_PinnedFree2\0"),
    })
}

/// Resolve a required symbol.
fn required<T: Copy>(library: &Library, name: &[u8]) -> Result<T, VshipException> {
    // SAFETY: the caller guarantees `T` matches the C signature for `name`.
    unsafe { library.get(name) }
        .map(|symbol| *symbol)
        .map_err(|_| VshipException::NotSupported)
}

/// Resolve an optional symbol, yielding `None` when the build omits it.
fn optional<T: Copy>(library: &Library, name: &[u8]) -> Option<T> {
    // SAFETY: the caller guarantees `T` matches the C signature for `name`.
    unsafe { library.get(name) }.ok().map(|symbol| *symbol)
}

/// Open the first libvship candidate that loads.
fn open_library() -> Result<Library, VshipException> {
    let mut failures = Vec::new();

    for candidate in library_candidates() {
        // SAFETY: loading an arbitrary shared library executes its initialisers.
        // This is the intended way to use libvship, and the candidate paths are
        // constrained to libvship file names.
        match unsafe { Library::new(&candidate) } {
            Ok(library) => return Ok(library),
            Err(error) => failures.push(format!("{}: {error}", candidate.display())),
        }
    }

    tracing::debug!(?failures, "no libvship candidate could be loaded");
    Err(VshipException::NoDeviceDetected)
}

/// Candidate libvship paths, most specific first.
///
/// The order is deliberate. Where libvship works it is the VapourSynth plugin
/// `vapoursynth/plugins/libvship.dll`, a sibling of `vsscript.dll` and on no
/// loader search path, so every environment-derived directory must be offered
/// before the bare names that let the platform loader search its own paths.
/// That is what makes "the VS plugin works" imply "the C API loads".
#[inline]
#[must_use]
pub fn library_candidates() -> Vec<PathBuf> {
    let names = library_names();

    let mut candidates = Vec::with_capacity(names.len() * 8);

    // 1. The user's explicit overrides.
    for variable in ["VSHIP_PLUGIN_PATH", "VSHIP_LIB_DIR"] {
        if let Some(directory) = env_directory(variable) {
            candidates.extend(names.iter().map(|name| directory.join(name)));
        }
    }

    // 2. The VapourSynth runtime that owns the plugin. `VSSCRIPT_PATH` names the
    //    script DLL, so the plugin directory is its sibling `plugins`.
    if let Ok(script) = std::env::var("VSSCRIPT_PATH")
        && !script.is_empty()
        && let Some(runtime) = Path::new(&script).parent()
    {
        candidates.extend(names.iter().map(|name| runtime.join("plugins").join(name)));
        candidates.extend(names.iter().map(|name| runtime.join(name)));
    }

    // 3. VapourSynth's own extra-plugin path list, which is `;`-separated on
    //    Windows and `:`-separated elsewhere.
    if let Ok(extra) = std::env::var("VAPOURSYNTH_EXTRA_PLUGIN_PATH") {
        let separator = if cfg!(target_os = "windows") {
            ';'
        } else {
            ':'
        };
        for entry in extra.split(separator).filter(|path| !path.is_empty()) {
            let directory = PathBuf::from(entry);
            candidates.extend(names.iter().map(|name| directory.join(name)));
        }
    }

    // 4. Platform system directories.
    for directory in system_directories() {
        candidates.extend(names.iter().map(|name| directory.join(name)));
    }

    // 5. Bare names, so the platform loader's own search path applies. A parentless
    //    candidate is the signal that the loader, not this crate, decides where the
    //    library is.
    candidates.extend(names.iter().map(PathBuf::from));

    candidates
}

/// Directories a system-wide install would use on this host.
///
/// On Windows these are asked of the OS rather than written down. `C:\Windows`
/// is not a safe constant: Windows can be installed to another drive or
/// relocated, and a hardcoded path then names a directory that does not exist.
/// `GetSystemDirectoryW` and its WOW64 counterpart report what this process can
/// actually load from, which also gets the WoW64 redirection right without
/// reasoning about the process bitness here.
///
/// Nothing is returned on other platforms, where the bare names already reach
/// every directory the loader searches.
#[inline]
#[must_use]
fn system_directories() -> Vec<PathBuf> {
    if !cfg!(target_os = "windows") {
        return Vec::new();
    }

    // Native System32 first, then the WOW64 directory. A 32-bit process asking for
    // System32 is redirected to SysWOW64, which is the directory it can really load
    // from, so both entry points are worth asking.
    let mut directories = Vec::with_capacity(2);
    directories.extend(windows_system_directory(true));
    directories.extend(windows_system_directory(false));
    directories.extend(windows_directory("SysWOW64"));
    directories.dedup();

    directories
}

/// One of Windows' system directories, as the OS reports it.
///
/// `native_system` selects `GetSystemDirectoryW` over
/// `GetSystemWow64DirectoryW`.
#[cfg(target_os = "windows")]
fn windows_system_directory(native_system: bool) -> Option<PathBuf> {
    use std::os::windows::ffi::OsStringExt;

    unsafe extern "system" {
        fn GetSystemDirectoryW(buffer: *mut u16, size: u32) -> u32;
        fn GetSystemWow64DirectoryW(buffer: *mut u16, size: u32) -> u32;
    }

    let entry: unsafe extern "system" fn(*mut u16, u32) -> u32 = if native_system {
        GetSystemDirectoryW
    } else {
        GetSystemWow64DirectoryW
    };

    // A return equal to the buffer size means the path was truncated, so grow and
    // ask again rather than using a partial directory.
    let mut buffer = vec![0u16; 260];
    loop {
        let capacity = buffer.len();
        // SAFETY: `buffer` is writable for `capacity` units, which is the size
        // passed alongside it.
        let written = unsafe { entry(buffer.as_mut_ptr(), capacity as u32) };
        if written == 0 {
            return None;
        }
        if (written as usize) < capacity {
            buffer.truncate(written as usize);
            break;
        }
        buffer.resize(capacity * 2, 0);
    }

    Some(PathBuf::from(std::ffi::OsString::from_wide(&buffer)))
}

/// Nothing to ask off Windows.
#[cfg(not(target_os = "windows"))]
#[inline]
#[must_use]
const fn windows_system_directory(_native_system: bool) -> Option<PathBuf> {
    None
}

/// A directory beside `%SystemRoot%`.
#[cfg(target_os = "windows")]
fn windows_directory(subdirectory: &str) -> Option<PathBuf> {
    env_directory("SystemRoot").map(|root| root.join(subdirectory))
}

/// Nothing to ask off Windows.
#[cfg(not(target_os = "windows"))]
#[inline]
#[must_use]
fn windows_directory(_subdirectory: &str) -> Option<PathBuf> {
    None
}

/// Candidate library file names for the host platform.
#[inline]
fn library_names() -> &'static [&'static str] {
    if cfg!(target_os = "windows") {
        &["libvship.dll", "vship.dll"]
    } else if cfg!(target_os = "macos") {
        &["libvship.dylib", "libvship.2.dylib"]
    } else {
        &["libvship.so", "libvship.so.2"]
    }
}

/// A directory named by an environment variable, if it is set and non-empty.
#[inline]
#[must_use]
fn env_directory(variable: &str) -> Option<PathBuf> {
    std::env::var(variable)
        .ok()
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

/// The directory the loaded libvship module was mapped from.
///
/// Asks the operating system rather than inferring it from the candidate list,
/// so it is correct however the library was found: an explicit
/// `VSHIP_PLUGIN_PATH`, the VapourSynth plugin directory, the platform loader's
/// search path, or an already-mapped module.
#[inline]
#[must_use]
pub fn loaded_library_directory() -> Option<&'static Path> {
    static MODULE_DIRECTORY: OnceLock<Option<PathBuf>> = OnceLock::new();

    MODULE_DIRECTORY
        .get_or_init(|| {
            let Ok(api) = VshipApi::load() else {
                return None;
            };

            // `get_version` is a valid function pointer once the library is
            // loaded, and its address identifies the module holding it.
            windows_module_directory_from(api.get_version as *const c_void)
        })
        .as_deref()
}

/// The directory the OS reports for the mapped libvship module.
#[cfg(target_os = "windows")]
fn windows_module_directory_from(address: *const c_void) -> Option<PathBuf> {
    use std::os::windows::ffi::OsStringExt;

    unsafe extern "system" {
        fn GetModuleHandleExW(
            flags: u32,
            module: *const c_void,
            module_handle: *mut *mut c_void,
        ) -> i32;
        fn GetModuleFileNameW(module: *mut c_void, buffer: *mut u16, size: u32) -> u32;
    }

    /// Ask for the module containing an address.
    const FROM_ADDRESS: u32 = 0x0000_0004;
    /// Report the module without changing its reference count, so the caller
    /// does not have to free it.
    const UNCHANGED_REFCOUNT: u32 = 0x0000_0002;

    let mut handle: *mut c_void = std::ptr::null_mut();
    // SAFETY: `address` is a valid code address inside a loaded module, and
    // `handle` is a writable out-pointer.
    if unsafe { GetModuleHandleExW(FROM_ADDRESS | UNCHANGED_REFCOUNT, address, &raw mut handle) }
        == 0
    {
        return None;
    }

    // A return equal to the buffer size means the name was truncated, so grow
    // the buffer and ask again.
    let mut buffer = vec![0u16; 512];
    loop {
        let capacity = buffer.len();
        // SAFETY: `buffer` is writable for `buffer.len()` units, which is the
        // capacity passed alongside it.
        let written = unsafe { GetModuleFileNameW(handle, buffer.as_mut_ptr(), capacity as u32) };
        if written == 0 {
            return None;
        }
        if written < capacity as u32 {
            buffer.truncate(written as usize);
            break;
        }
        buffer.resize(capacity * 2, 0);
    }

    let path =
        std::ffi::OsString::from_wide(buffer.split(|&unit| unit == 0).next().unwrap_or_default());

    // The API answers with a file name; callers want the directory it sits in.
    let path = PathBuf::from(path);
    Some(path.parent()?.to_path_buf())
}

/// No module introspection is available off Windows.
#[cfg(not(target_os = "windows"))]
fn windows_module_directory_from(_address: *const c_void) -> Option<PathBuf> {
    None
}

#[cfg(test)]
mod tests {
    use std::mem::{align_of, size_of};

    use super::*;

    /// Guards the FFI layouts against accidental reordering. A wrong field
    /// order still compiles, and surfaces only as an opaque status from
    /// libvship at runtime, which is exactly the bug these tests exist to
    /// prevent.
    #[test]
    fn colorspace_layout_matches_the_c_header() {
        // Mirrors `typedef struct Vship_Colorspace_t` in VshipColor.h.
        #[repr(C)]
        struct Expected {
            width:             i64,
            height:            i64,
            target_width:      i64,
            target_height:     i64,
            sample:            VshipSample,
            range:             VshipRange,
            subsampling:       VshipChromaSubsample,
            chroma_location:   VshipChromaLocation,
            color_family:      VshipColorFamily,
            yuv_matrix:        VshipYuvMatrix,
            transfer_function: VshipTransferFunction,
            primaries:         VshipPrimaries,
            crop:              VshipCropRectangle,
        }

        assert_eq!(size_of::<VshipColorspace>(), size_of::<Expected>());
        assert_eq!(align_of::<VshipColorspace>(), align_of::<Expected>());
    }

    #[test]
    fn colorspace_field_offsets_match_the_c_header() {
        let colorspace = VshipColorspace::bt709_default(1920, 1080);
        let base = std::ptr::from_ref(&colorspace).cast::<u8>() as usize;
        let offset = |field: *const u8| field as usize - base;

        assert_eq!(
            offset(std::ptr::from_ref(&colorspace.width).cast::<u8>()),
            0
        );
        assert_eq!(
            offset(std::ptr::from_ref(&colorspace.height).cast::<u8>()),
            8
        );
        assert_eq!(
            offset(std::ptr::from_ref(&colorspace.target_width).cast::<u8>()),
            16
        );
        assert_eq!(
            offset(std::ptr::from_ref(&colorspace.target_height).cast::<u8>()),
            24
        );
        // Four int64 fields precede the first enum, so the enums begin at 32.
        assert_eq!(
            offset(std::ptr::from_ref(&colorspace.sample).cast::<u8>()),
            32
        );
    }

    /// `Vship_Sample_t` is the one enum whose values are not contiguous, so a
    /// wrong transcription would silently reinterpret every pixel buffer.
    #[test]
    fn system_directories_are_real_and_not_assumed_to_be_on_c() {
        // Windows can be installed to another drive or relocated, so the system
        // directory has to come from the OS. A hardcoded `C:\Windows` silently
        // names a directory that does not exist there, and the failure looks like
        // "libvship is not installed" rather than "the search path was wrong".
        for directory in system_directories() {
            assert!(directory.is_absolute(), "{directory:?} should be absolute");
            assert!(
                directory.is_dir(),
                "the reported system directory {directory:?} does not exist"
            );
        }

        // Whatever the OS says, a candidate that does not exist only costs a
        // failed `dlopen` attempt, so the set must be small and free of blanks.
        assert!(
            system_directories().iter().all(|directory| !directory.as_os_str().is_empty()),
            "an empty system directory would produce a candidate of just the file name"
        );
    }

    #[test]
    fn sample_discriminants_match_the_c_header() {
        assert_eq!(VshipSample::Float as c_int, 0);
        assert_eq!(VshipSample::Half as c_int, 1);
        assert_eq!(VshipSample::Uint8 as c_int, 2);
        assert_eq!(VshipSample::Uint9 as c_int, 3);
        assert_eq!(VshipSample::Uint10 as c_int, 5);
        assert_eq!(VshipSample::Uint12 as c_int, 7);
        assert_eq!(VshipSample::Uint14 as c_int, 9);
        assert_eq!(VshipSample::Uint16 as c_int, 11);
    }

    #[test]
    fn enum_discriminants_match_the_c_header() {
        assert_eq!(VshipBackend::Hip as c_int, 0);
        assert_eq!(VshipBackend::Cuda as c_int, 1);
        assert_eq!(VshipBackend::Vulkan as c_int, 2);
        assert_eq!(VshipBackend::Cpu as c_int, 3);

        assert_eq!(VshipRange::Limited as c_int, 0);
        assert_eq!(VshipRange::Full as c_int, 1);

        assert_eq!(VshipChromaLocation::Left as c_int, 0);
        assert_eq!(VshipChromaLocation::Center as c_int, 1);
        assert_eq!(VshipChromaLocation::TopLeft as c_int, 2);
        assert_eq!(VshipChromaLocation::Top as c_int, 3);

        assert_eq!(VshipColorFamily::YUV as c_int, 0);
        assert_eq!(VshipColorFamily::RGB as c_int, 1);

        assert_eq!(VshipYuvMatrix::Rgb as c_int, 0);
        assert_eq!(VshipYuvMatrix::Bt709 as c_int, 1);
        assert_eq!(VshipYuvMatrix::Bt470Bg as c_int, 5);
        assert_eq!(VshipYuvMatrix::St170M as c_int, 6);
        assert_eq!(VshipYuvMatrix::Ycgco as c_int, 8);
        assert_eq!(VshipYuvMatrix::Bt2020Ncl as c_int, 9);
        assert_eq!(VshipYuvMatrix::Bt2020Cl as c_int, 10);
        assert_eq!(VshipYuvMatrix::Bt2100Ictcp as c_int, 14);
        assert_eq!(VshipYuvMatrix::YcgcoRe as c_int, 16);
        assert_eq!(VshipYuvMatrix::YcgcoRo as c_int, 17);

        assert_eq!(VshipTransferFunction::Bt709 as c_int, 1);
        assert_eq!(VshipTransferFunction::Linear as c_int, 8);
        assert_eq!(VshipTransferFunction::Pq as c_int, 16);
        assert_eq!(VshipTransferFunction::Hlg as c_int, 18);

        assert_eq!(VshipPrimaries::Internal as c_int, -1);
        assert_eq!(VshipPrimaries::Bt709 as c_int, 1);
        assert_eq!(VshipPrimaries::Bt2020 as c_int, 9);
        assert_eq!(VshipPrimaries::DisplayP3 as c_int, 12);
    }

    #[test]
    fn exception_discriminants_match_the_c_header() {
        assert_eq!(VshipException::NoError as c_int, 0);
        assert_eq!(VshipException::OutOfVram as c_int, 1);
        assert_eq!(VshipException::OutOfRam as c_int, 2);
        assert_eq!(VshipException::BadDisplayModel as c_int, 3);
        assert_eq!(VshipException::DifferingInputType as c_int, 4);
        assert_eq!(VshipException::NonRgbsInput as c_int, 5);
        assert_eq!(VshipException::DeviceCountError as c_int, 6);
        assert_eq!(VshipException::NoDeviceDetected as c_int, 7);
        assert_eq!(VshipException::BadDeviceArgument as c_int, 8);
        assert_eq!(VshipException::BadDeviceCode as c_int, 9);
        assert_eq!(VshipException::BadHandler as c_int, 10);
        assert_eq!(VshipException::BadPointer as c_int, 11);
        assert_eq!(VshipException::HipError as c_int, 12);
        assert_eq!(VshipException::BadPath as c_int, 13);
        assert_eq!(VshipException::BadJson as c_int, 14);
        assert_eq!(VshipException::NotSupported as c_int, 15);
        assert_eq!(VshipException::BadInputStructType as c_int, 16);
        assert_eq!(VshipException::BadErrorType as c_int, 17);

        assert!(VshipException::NoError.is_ok());
        assert!(!VshipException::BadHandler.is_ok());
    }

    #[test]
    fn struct_type_discriminants_match_the_c_header() {
        assert_eq!(VshipStructType::Null as c_int, 0);
        assert_eq!(VshipStructType::InitSsimulacra2 as c_int, 1);
        assert_eq!(VshipStructType::InitButteraugli as c_int, 2);
        assert_eq!(VshipStructType::InitCvvdp as c_int, 3);
        assert_eq!(VshipStructType::ScoreSsimulacra2 as c_int, 4);
        assert_eq!(VshipStructType::ScoreButteraugli as c_int, 5);
        assert_eq!(VshipStructType::ScoreCvvdp as c_int, 6);
    }

    #[test]
    fn init_ssimulacra2_layout_matches_the_c_header() {
        // Mirrors `typedef struct Vship_InitSSIMULACRA2_1` in VshipAPI.h.
        #[repr(C)]
        struct Expected {
            struct_type:    VshipStructType,
            src_colorspace: VshipColorspace,
            dis_colorspace: VshipColorspace,
            gpu_id:         c_int,
        }

        assert_eq!(size_of::<VshipInitSsimulacra2_1>(), size_of::<Expected>());
        assert_eq!(align_of::<VshipInitSsimulacra2_1>(), align_of::<Expected>());
    }

    #[test]
    fn init_butteraugli_layout_matches_the_c_header() {
        // Mirrors `typedef struct Vship_InitButteraugli_1` in VshipAPI.h.
        #[repr(C)]
        struct Expected {
            struct_type:          VshipStructType,
            src_colorspace:       VshipColorspace,
            dis_colorspace:       VshipColorspace,
            q_norm:               c_int,
            intensity_multiplier: f32,
            gpu_id:               c_int,
        }

        assert_eq!(size_of::<VshipInitButteraugli_1>(), size_of::<Expected>());
        assert_eq!(align_of::<VshipInitButteraugli_1>(), align_of::<Expected>());
    }

    #[test]
    fn init_cvvdp_layout_matches_the_c_header() {
        // Mirrors `typedef struct Vship_InitCVVDP_1` in VshipAPI.h.
        #[repr(C)]
        struct Expected {
            struct_type:            VshipStructType,
            src_colorspace:         VshipColorspace,
            dis_colorspace:         VshipColorspace,
            fps:                    f32,
            resize_to_display:      bool,
            model_key_cstr:         *const c_char,
            model_config_json_cstr: *const c_char,
            gpu_id:                 c_int,
        }

        assert_eq!(size_of::<VshipInitCvvdp_1>(), size_of::<Expected>());
        assert_eq!(align_of::<VshipInitCvvdp_1>(), align_of::<Expected>());
    }

    #[test]
    fn score_ssimulacra2_layout_matches_the_c_header() {
        // Mirrors `typedef struct Vship_ScoreSSIMULACRA2` in VshipAPI.h.
        #[repr(C)]
        struct Expected {
            struct_type: VshipStructType,
            score:       f64,
        }

        assert_eq!(size_of::<VshipScoreSsimulacra2>(), size_of::<Expected>());
        assert_eq!(align_of::<VshipScoreSsimulacra2>(), align_of::<Expected>());
    }

    #[test]
    fn score_butteraugli_layout_matches_the_c_header() {
        // Mirrors `typedef struct Vship_ScoreButteraugli` in VshipAPI.h.
        #[repr(C)]
        struct Expected {
            struct_type: VshipStructType,
            norm_q:      f64,
            norm_3:      f64,
            norm_inf:    f64,
            dstp:        *const u8,
            dst_stride:  i64,
        }

        assert_eq!(size_of::<VshipScoreButteraugli>(), size_of::<Expected>());
        assert_eq!(align_of::<VshipScoreButteraugli>(), align_of::<Expected>());
    }

    #[test]
    fn score_cvvdp_layout_matches_the_c_header() {
        // Mirrors `typedef struct Vship_ScoreCVVDP` in VshipAPI.h.
        #[repr(C)]
        struct Expected {
            struct_type: VshipStructType,
            score:       f64,
            dstp:        *const u8,
            dst_stride:  i64,
        }

        assert_eq!(size_of::<VshipScoreCvvdp>(), size_of::<Expected>());
        assert_eq!(align_of::<VshipScoreCvvdp>(), align_of::<Expected>());
    }

    #[test]
    fn version_layout_matches_the_c_header() {
        // Mirrors the anonymous `Vship_Version` struct in VshipAPI.h.
        #[repr(C)]
        struct Expected {
            major:       c_int,
            minor:       c_int,
            minor_minor: c_int,
            backend:     VshipBackend,
        }

        assert_eq!(size_of::<VshipVersion>(), size_of::<Expected>());
        assert_eq!(align_of::<VshipVersion>(), align_of::<Expected>());
    }

    #[test]
    fn device_info_layout_matches_the_c_header() {
        // Mirrors `typedef struct Vship_DeviceInfo` in VshipAPI.h.
        #[repr(C)]
        struct Expected {
            name:                  [c_char; 256],
            vram_size:             u64,
            integrated:            c_int,
            multi_processor_count: c_int,
            warp_size:             c_int,
            vulkan_feature_matrix: VshipVulkanFeatureMatrix,
        }

        assert_eq!(size_of::<VshipDeviceInfo>(), size_of::<Expected>());
        assert_eq!(align_of::<VshipDeviceInfo>(), align_of::<Expected>());
    }

    #[test]
    fn device_info_name_stops_at_the_nul() {
        let mut info = VshipDeviceInfo::default();
        // The C header declares `name` as `char`, which is signed, so each ASCII
        // byte must be reinterpreted rather than assigned directly.
        for (slot, byte) in info.name[..5].iter_mut().zip(*b"GeFoo") {
            *slot = byte as c_char;
        }
        assert_eq!(info.name(), "GeFoo");
        assert_eq!(VshipDeviceInfo::default().name(), "");
    }

    /// A machine without libvship must be reported as unavailable rather than
    /// panicking, which is what keeps CI green with no GPU and no binary.
    #[test]
    fn loading_is_total_and_never_panics() {
        let outcome = VshipApi::load();
        match outcome {
            Ok(api) => assert!(!api.version_string().is_empty()),
            Err(error) => assert!(!error.to_string().is_empty()),
        }
    }

    /// The VapourSynth plugin directory must precede the bare names, since a
    /// working install puts libvship there and on no loader search path.
    #[test]
    fn vapoursynth_plugin_directory_precedes_the_bare_names() {
        let candidates = library_candidates();
        let first_bare = candidates
            .iter()
            .position(|path| path.parent().is_none_or(|parent| parent.as_os_str().is_empty()))
            .expect("a bare name is always offered");

        let Ok(script) = std::env::var("VSSCRIPT_PATH") else {
            return;
        };
        let Some(runtime) = Path::new(&script).parent() else {
            return;
        };
        let plugins = runtime.join("plugins");

        let plugin_candidates = candidates
            .iter()
            .filter(|path| path.parent() == Some(plugins.as_path()))
            .count();
        if plugin_candidates == 0 {
            return;
        }

        assert!(
            candidates
                .iter()
                .take(first_bare)
                .any(|path| path.parent() == Some(plugins.as_path())),
            "plugin candidates must be offered before the bare names"
        );
    }

    #[test]
    fn check_accepts_no_error_and_rejects_the_rest() {
        assert!(check(VshipException::NoError, "Vship_InitHandler").is_ok());
        assert!(matches!(
            check(VshipException::BadHandler, "Vship_InitHandler"),
            Err(VshipError::CallFailed {
                function: "Vship_InitHandler",
                status: 10,
                ..
            })
        ));
    }
}
