//! Configuration types for VMAF scoring.

use std::{
    ffi::c_int,
    fmt::{self, Display, Formatter},
    path::{Path, PathBuf},
};

use strum::{EnumString, IntoStaticStr};

use crate::{error::VmafError, ffi};

/// A VMAF model, either one of the stock models or a user-supplied path.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum VmafModel {
    /// `vmaf_v0.6.1`, the default model.
    #[default]
    Default,
    /// `vmaf_v0.6.1neg`, trained for negative-content scoring.
    Neg,
    /// `vmaf_4k_v0.6.1neg`, the 4K model for negative-content scoring.
    UhdNeg,
    /// `vmaf_b_v0.6.3`, the b-model used for confidence intervals.
    ///
    /// **Not loadable by libvmaf 3.2.1**, which removed the BOUND feature
    /// extractors and exposes no `enable_bound` option. Loading it fails with
    /// `EINVAL`, so this model is unusable on every stock build. Use
    /// [`Self::Uhd`] or an explicit [`Self::Path`] instead.
    Weighted,
    /// `vmaf_4k_v0.6.1`, trained at 4K.
    Uhd,
    /// An explicit model file on disk.
    Path(PathBuf),
}

impl VmafModel {
    /// Every stock model, for enumerating what a libvmaf build can load.
    ///
    /// [`Self::Path`] is deliberately excluded: a custom model is a per-run
    /// choice, not a property of the installation.
    ///
    /// [`Self::Weighted`] is included even though libvmaf 3.2.1 cannot load
    /// it, so that a report walking this list shows the model's real state
    /// rather than quietly omitting one of the five stock models.
    pub const STOCK: [Self; 5] =
        [Self::Default, Self::Neg, Self::UhdNeg, Self::Weighted, Self::Uhd];

    /// Resolve this model to the string libvmaf expects.
    ///
    /// `vmaf_model_load` takes a bare version string for built-in models, or a
    /// filesystem path for a custom model. It does *not* accept the `version=`
    /// or `path=` key/value form that the `vmaf` CLI accepts, so those prefixes
    /// must not be added here.
    #[inline]
    pub fn as_libvmaf_model(&self) -> Result<String, VmafError> {
        match self {
            Self::Default => Ok("vmaf_v0.6.1".to_owned()),
            Self::Neg => Ok("vmaf_v0.6.1neg".to_owned()),
            Self::UhdNeg => Ok("vmaf_4k_v0.6.1neg".to_owned()),
            Self::Weighted => Ok("vmaf_b_v0.6.3".to_owned()),
            Self::Uhd => Ok("vmaf_4k_v0.6.1".to_owned()),
            Self::Path(path) => {
                if !path.is_file() {
                    return Err(VmafError::ModelFileMissing {
                        path: path.clone()
                    });
                }
                Ok(path.display().to_string())
            },
        }
    }

    /// Whether this model refers to a file on disk rather than a built-in name.
    ///
    /// libvmaf exposes separate loaders for the two, and each rejects the
    /// other's input, so the scorer must know which to call.
    #[inline]
    #[must_use]
    pub const fn is_path(&self) -> bool {
        matches!(self, Self::Path(_))
    }

    /// The bare libvmaf version string for this model, when it names a stock
    /// model.
    ///
    /// Used to locate a model file on disk when libvmaf was built without the
    /// built-in models compiled in.
    #[inline]
    #[must_use]
    pub const fn stock_version(&self) -> Option<&'static str> {
        match self {
            Self::Default => Some("vmaf_v0.6.1"),
            Self::Neg => Some("vmaf_v0.6.1neg"),
            Self::UhdNeg => Some("vmaf_4k_v0.6.1neg"),
            Self::Weighted => Some("vmaf_b_v0.6.3"),
            Self::Uhd => Some("vmaf_4k_v0.6.1"),
            Self::Path(_) => None,
        }
    }

    /// Directories searched for a stock model file.
    ///
    /// Not every libvmaf build compiles the models in. Arch's `vmaf` package
    /// and MSYS2's `mingw-w64-*-vmaf` both ship without them, and
    /// `vmaf_model_load` then rejects a bare version with `EINVAL`.
    /// Distributions that do not embed the models install them under one of
    /// these prefixes instead.
    ///
    /// System directories are returned before directories merely *near* the
    /// binary, so a stray `vmaf_v0.6.1.json` in a shared or downloads-adjacent
    /// directory cannot silently override the model the distribution intends.
    #[inline]
    #[must_use]
    pub fn model_search_paths() -> Vec<PathBuf> {
        let paths = model_search_paths_for(ffi::loaded_library_directory());
        let mut unique: Vec<PathBuf> = Vec::with_capacity(paths.len());
        for path in paths {
            if !unique.contains(&path) {
                unique.push(path);
            }
        }
        unique
    }

    /// Locate a stock model file for this model, if one is installed on disk.
    ///
    /// Returns the filesystem path when a `<version>.json` exists in one of
    /// [`Self::model_search_paths`].
    #[inline]
    #[must_use]
    pub fn find_stock_model_file(&self) -> Option<PathBuf> {
        let version = self.stock_version()?;
        let filename = format!("{version}.json");

        Self::model_search_paths()
            .into_iter()
            .map(|directory| directory.join(&filename))
            .find(|candidate| candidate.is_file())
    }

    /// Whether this model can be used on the CUDA backend.
    ///
    /// libvmaf's CUDA feature extractors currently cover integer ADM, integer
    /// motion and integer VIF only. The stock models additionally require PSNR
    /// and MS-SSIM, which have no CUDA implementation, so none of them are
    /// expected to initialise on CUDA. The scorer still probes at runtime
    /// rather than trusting this.
    #[inline]
    #[must_use]
    pub const fn cuda_supported(&self) -> bool {
        false
    }
}

/// Model directories under an MSYS2 installation rooted at `root`, paired with
/// whether the distribution owns them.
///
/// Only `<prefix>/share/vmaf/model` is distribution-owned.
/// `<prefix>/share/model` is the Arch-style sibling and `<prefix>/model` is
/// where a release tarball may place them, and neither sits in a location a
/// package manager is guaranteed to own, so both rank with the paths merely
/// near the binary.
#[cfg(target_os = "windows")]
fn msys2_model_directories(root: &Path) -> Vec<(PathBuf, bool)> {
    ["mingw64", "ucrt64"]
        .into_iter()
        .flat_map(|prefix| {
            let prefix = root.join(prefix);
            [
                (prefix.join("share").join("vmaf").join("model"), true),
                (prefix.join("share").join("model"), false),
                (prefix.join("model"), false),
            ]
        })
        .collect()
}

/// The MSYS2 root to search for models under.
///
/// The candidate MSYS2 model directories under a root inferred from where
/// libvmaf was loaded.
///
/// MSYS2 defaults to `C:\msys64`, but it can be installed on any drive, so the
/// root is discovered rather than assumed: `MSYS2_ROOT` when the machine sets
/// it, otherwise the prefix above the directory libvmaf was loaded from,
/// because the `mingw-w64-*-vmaf` package installs to `<root>/<prefix>/bin`.
#[cfg(target_os = "windows")]
fn msys2_root(loaded_from: Option<&Path>) -> Option<PathBuf> {
    std::env::var("MSYS2_ROOT").ok().filter(|root| !root.is_empty()).map_or_else(
        || dll_implied_root(loaded_from),
        |root| Some(PathBuf::from(root)),
    )
}

/// The MSYS2 root implied by where libvmaf was loaded from.
///
/// The package installs to `<root>/<prefix>/bin`, so the load directory is
/// `bin` and the root is two levels above it. A library resolved by the loader
/// from a bare file name records no directory, and therefore yields no root;
/// neither does a path too shallow to have an MSYS2 layout above it.
#[cfg(target_os = "windows")]
fn dll_implied_root(loaded_from: Option<&Path>) -> Option<PathBuf> {
    let bin = loaded_from?;
    let root = bin.parent()?.parent()?;
    // A root such as `C:\` has nothing above it, so there is no layout to derive.
    root.parent()?;
    Some(root.to_path_buf())
}

/// MSYS2 is not a package prefix on other platforms.
#[cfg(not(target_os = "windows"))]
fn msys2_root(_loaded_from: Option<&Path>) -> Option<PathBuf> {
    None
}

/// Model directories to search under an MSYS2 root, paired with whether the
/// distribution owns them.
///
/// There is no MSYS2 layout off Windows, so nothing is added there.
#[cfg(not(target_os = "windows"))]
fn msys2_model_directories(_root: &Path) -> Vec<(PathBuf, bool)> {
    Vec::new()
}

/// Collect every candidate search path, given the directory the library loaded
/// from.
///
/// The directory is a parameter so the loaded-library inference can be
/// exercised without a real installation, which is what keeps its ordering
/// covered on machines that have no libvmaf at all.
fn model_search_paths_for(loaded_from: Option<&Path>) -> Vec<PathBuf> {
    let mut system = Vec::new();
    let mut adjacent = Vec::new();

    if let Ok(directory) = std::env::var("VMAF_MODEL_PATH") {
        adjacent.push(PathBuf::from(directory));
    }

    // System-wide installs. These are the paths a package manager controls, so
    // a model found here is the one the distribution intends, and it takes
    // precedence over anything merely *near* the binary.
    if cfg!(target_os = "windows") {
        if let Some(root) = msys2_root(loaded_from) {
            for (path, is_system) in msys2_model_directories(&root) {
                if is_system {
                    system.push(path);
                } else {
                    adjacent.push(path);
                }
            }
        }
    } else if cfg!(target_os = "macos") {
        system.push(PathBuf::from("/opt/homebrew/share/vmaf/model"));
        system.push(PathBuf::from("/usr/local/share/vmaf/model"));
    } else {
        // Arch's `vmaf` package installs to `/usr/share/model`, Debian and
        // Fedora use `/usr/share/vmaf/model`, and a source build commonly lands
        // in `/usr/local/share/model`.
        system.push(PathBuf::from("/usr/share/model"));
        system.push(PathBuf::from("/usr/share/vmaf/model"));
        system.push(PathBuf::from("/usr/local/share/model"));
        system.push(PathBuf::from("/usr/local/share/vmaf/model"));
    }

    // Beside the running executable. This is what a release layout relies on:
    // `condor.exe` and `model/vmaf_v0.6.1.json` shipped together, with no
    // environment variable set.
    if let Ok(executable) = std::env::current_exe()
        && let Some(directory) = executable.parent()
    {
        adjacent.push(directory.join("model"));
    }

    // Beside the library that was actually loaded. This is what makes a
    // relocated MSYS2 work: the recorded directory is wherever the load
    // succeeded, so `.../bin/libvmaf.dll` implies `.../model` and
    // `.../share/vmaf/model` without any prefix being assumed.
    if let Some(directory) = loaded_from {
        if let Some(prefix) = directory.parent() {
            adjacent.push(prefix.join("model"));
            system.push(prefix.join("share").join("vmaf").join("model"));
        }
        adjacent.push(directory.join("model"));
    }

    // The same inference from `VMAF_LIB_DIR`, for when the library was found on
    // the loader's search path rather than by explicit directory.
    if let Ok(directory) = std::env::var("VMAF_LIB_DIR")
        && !directory.is_empty()
    {
        let directory = PathBuf::from(directory);
        if let Some(prefix) = directory.parent() {
            adjacent.push(prefix.join("model"));
            system.push(prefix.join("share").join("vmaf").join("model"));
        }
        adjacent.push(directory.join("model"));
    }

    // The sources above overlap freely — an explicit `VMAF_LIB_DIR` is also
    // where the load came from — and a repeated directory only makes the
    // "search these" hint in an error message longer. The two groups are joined
    // only here, so no source above can emit a system path behind an adjacent
    // one regardless of the order the sources are appended in.
    let mut paths = system;
    paths.extend(adjacent);
    paths
}

/// Whether a search path is controlled by the platform or the distribution.
///
/// True for the prefixes a package manager owns, so a model found there is the
/// one the distribution intends. A path carries the separator of the platform
/// it was built on, and a Windows path assembled from a POSIX-looking root
/// mixes the two, so the `share/vmaf/model` suffix is matched in every spelling
/// rather than assuming the path was constructed consistently.
#[cfg(test)]
fn is_system_model_path(path: &str) -> bool {
    path.starts_with("/usr/")
        || path.starts_with("/opt/")
        || [
            "share/vmaf/model",
            "share/vmaf\\model",
            "share\\vmaf/model",
            "share\\vmaf\\model",
        ]
        .iter()
        .any(|suffix| path.contains(suffix))
}

impl Display for VmafModel {
    #[inline]
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Default => f.write_str("vmaf_v0.6.1"),
            Self::Neg => f.write_str("vmaf_v0.6.1neg"),
            Self::UhdNeg => f.write_str("vmaf_4k_v0.6.1neg"),
            Self::Weighted => f.write_str("vmaf_b_v0.6.3"),
            Self::Uhd => f.write_str("vmaf_4k_v0.6.1"),
            Self::Path(path) => write!(f, "{}", path.display()),
        }
    }
}

/// Optional additional feature extractors to compute alongside the model score.
#[derive(Debug, Clone, Copy, PartialEq, Eq, strum::Display, EnumString, IntoStaticStr, Default)]
pub enum VmafFeature {
    /// No additional features.
    #[default]
    #[strum(serialize = "default")]
    Default,
    /// PSNR.
    #[strum(serialize = "psnr")]
    Psnr,
    /// PSNR weighted for human visual system sensitivity.
    #[strum(serialize = "psnr_hvs")]
    PsnrHvs,
    /// Structural similarity.
    #[strum(serialize = "ssim")]
    Ssim,
    /// Multi-scale structural similarity.
    #[strum(serialize = "ms_ssim")]
    MsSsim,
    /// CAMBI, the banding detector.
    #[strum(serialize = "cambi")]
    Cambi,
}

impl VmafFeature {
    /// The name libvmaf uses to *register* this extractor via
    /// `vmaf_use_feature`.
    ///
    /// These are the extractor names, e.g. `psnr`. The per-plane values they
    /// emit (`psnr_y`, `psnr_cb`, `psnr_cr`) are separate keys used when
    /// reading scores back, so the two are deliberately distinct.
    #[inline]
    #[must_use]
    pub const fn as_libvmaf_extractor(self) -> Option<&'static str> {
        match self {
            Self::Default => None,
            Self::Psnr => Some("psnr"),
            Self::PsnrHvs => Some("psnr_hvs"),
            Self::Ssim => Some("float_ssim"),
            Self::MsSsim => Some("float_ms_ssim"),
            Self::Cambi => Some("cambi"),
        }
    }

    /// The feature key libvmaf reports this extractor's primary value under.
    ///
    /// Used with `vmaf_feature_score_pooled`. PSNR emits one value per plane,
    /// so the luma plane is used as the representative score.
    #[inline]
    #[must_use]
    pub const fn as_libvmaf_feature(self) -> Option<&'static str> {
        match self {
            Self::Default => None,
            Self::Psnr => Some("psnr_y"),
            Self::PsnrHvs => Some("psnr_hvs_y"),
            Self::Ssim => Some("float_ssim"),
            Self::MsSsim => Some("float_ms_ssim"),
            Self::Cambi => Some("cambi"),
        }
    }
}

/// How VMAF scores are pooled into a single value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PoolMethod {
    /// Arithmetic mean. This is libvmaf's default pooling.
    #[default]
    Mean,
    /// Minimum.
    Min,
    /// Maximum.
    Max,
    /// Harmonic mean, which VMAF recommends when scenes have mixed quality.
    HarmonicMean,
}

impl PoolMethod {
    /// The `VmafPoolingMethod` discriminant libvmaf expects.
    ///
    /// Verified against libvmaf 3.2.1, where the enum is
    /// `UNKNOWN = 0, MIN, MAX, MEAN, HARMONIC_MEAN`. Note `MEAN` is 3, not 0 —
    /// libvmaf treats 0 as an invalid method and rejects it with `EINVAL`.
    #[inline]
    #[must_use]
    pub const fn as_libvmaf_pool(self) -> c_int {
        match self {
            Self::Min => 1,
            Self::Max => 2,
            Self::Mean => 3,
            Self::HarmonicMean => 4,
        }
    }
}

/// How the scorer should choose a compute backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BackendPreference {
    /// Try CUDA if available, otherwise CPU.
    #[default]
    Auto,
    /// Require CPU, never attempt CUDA.
    CpuOnly,
    /// Require CUDA and fail if it is unavailable.
    RequireCuda,
}

/// Configuration for a VMAF scoring session.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct VmafConfig {
    /// The model to score against.
    pub model:     VmafModel,
    /// Additional feature extractors to compute.
    pub features:  Vec<VmafFeature>,
    /// Number of worker threads libvmaf should use. `0` lets libvmaf decide.
    pub n_threads: u32,
    /// Backend selection strategy.
    pub backend:   BackendPreference,
}

impl VmafConfig {
    /// A configuration using the default model and libvmaf's own thread count.
    #[inline]
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the model.
    #[inline]
    #[must_use]
    pub fn with_model(mut self, model: VmafModel) -> Self {
        self.model = model;
        self
    }

    /// Set the model from an explicit path.
    #[inline]
    pub fn with_model_path(self, path: impl AsRef<Path>) -> Self {
        self.with_model(VmafModel::Path(path.as_ref().to_path_buf()))
    }

    /// Add a feature extractor.
    #[inline]
    #[must_use]
    pub fn with_feature(mut self, feature: VmafFeature) -> Self {
        self.features.push(feature);
        self
    }

    /// Set libvmaf's worker thread count.
    #[inline]
    #[must_use]
    pub fn with_threads(mut self, n_threads: u32) -> Self {
        self.n_threads = n_threads;
        self
    }

    /// Set the backend selection strategy.
    #[inline]
    #[must_use]
    pub fn with_backend(mut self, backend: BackendPreference) -> Self {
        self.backend = backend;
        self
    }

    /// The extractor names to register with libvmaf, in order.
    ///
    /// `Default` contributes nothing and duplicates are dropped while
    /// preserving the caller's ordering.
    #[inline]
    #[must_use]
    pub fn extractor_names(&self) -> Vec<&'static str> {
        let mut names = Vec::with_capacity(self.features.len());
        for feature in &self.features {
            if let Some(name) = feature.as_libvmaf_extractor()
                && !names.contains(&name)
            {
                names.push(name);
            }
        }
        names
    }

    /// The feature keys to read pooled values for, in order.
    ///
    /// `Default` contributes nothing and duplicates are dropped while
    /// preserving the caller's ordering.
    #[inline]
    #[must_use]
    pub fn feature_names(&self) -> Vec<&'static str> {
        let mut names = Vec::with_capacity(self.features.len());
        for feature in &self.features {
            if let Some(name) = feature.as_libvmaf_feature()
                && !names.contains(&name)
            {
                names.push(name);
            }
        }
        names
    }
}

#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    reason = "assertions read better with unwrap; a failure panics with a usable message"
)]
mod tests {
    use super::*;

    #[test]
    fn stock_models_resolve_to_libvmaf_version_strings() {
        assert_eq!(
            VmafModel::Default.as_libvmaf_model().unwrap(),
            "vmaf_v0.6.1"
        );
        assert_eq!(VmafModel::Neg.as_libvmaf_model().unwrap(), "vmaf_v0.6.1neg");
        assert_eq!(
            VmafModel::UhdNeg.as_libvmaf_model().unwrap(),
            "vmaf_4k_v0.6.1neg"
        );
        assert_eq!(
            VmafModel::Weighted.as_libvmaf_model().unwrap(),
            "vmaf_b_v0.6.3"
        );
        assert_eq!(VmafModel::Uhd.as_libvmaf_model().unwrap(), "vmaf_4k_v0.6.1");
    }

    #[test]
    fn missing_model_path_is_an_error() {
        let model = VmafModel::Path(PathBuf::from("/nonexistent/model.json"));
        assert!(matches!(
            model.as_libvmaf_model(),
            Err(VmafError::ModelFileMissing { .. })
        ));
    }

    #[test]
    fn default_feature_contributes_no_name() {
        let config = VmafConfig::new().with_feature(VmafFeature::Default);
        assert!(config.feature_names().is_empty());
    }

    #[test]
    fn duplicate_features_are_deduplicated_in_order() {
        let config = VmafConfig::new()
            .with_feature(VmafFeature::Psnr)
            .with_feature(VmafFeature::Ssim)
            .with_feature(VmafFeature::Psnr);
        assert_eq!(config.feature_names(), vec!["psnr_y", "float_ssim"]);
    }

    #[test]
    fn a_stock_model_names_the_file_libvmaf_expects() {
        assert_eq!(VmafModel::Default.stock_version(), Some("vmaf_v0.6.1"));
        assert_eq!(VmafModel::Neg.stock_version(), Some("vmaf_v0.6.1neg"));
        assert_eq!(VmafModel::Weighted.stock_version(), Some("vmaf_b_v0.6.3"));
        assert_eq!(VmafModel::Uhd.stock_version(), Some("vmaf_4k_v0.6.1"));
        assert_eq!(VmafModel::UhdNeg.stock_version(), Some("vmaf_4k_v0.6.1neg"));
        assert_eq!(
            VmafModel::Path(PathBuf::from("/x.json")).stock_version(),
            None,
            "an explicit path is not a stock model"
        );
    }

    /// The stock model must be findable on the platforms we ship for.
    ///
    /// Arch's `vmaf` package installs models to `/usr/share/model`, not the
    /// `/usr/share/vmaf/model` a source build would use. Missing that path
    /// silently disables the metric on the CI image and in Docker, where
    /// the library is present but the model cannot be loaded.
    #[test]
    fn the_model_search_paths_cover_known_packaging_layouts() {
        let paths: Vec<String> = VmafModel::model_search_paths()
            .iter()
            .map(|path| path.display().to_string())
            .collect();

        let required: &[&str] = if cfg!(target_os = "macos") {
            &["/opt/homebrew/share/vmaf/model"]
        } else if cfg!(target_os = "windows") {
            // Windows resolves the MSYS2 root from the machine, so there is no
            // fixed path to require — and libvmaf may not even be loaded yet,
            // in which case no prefix can be derived and none is claimed.
            return;
        } else {
            &["/usr/share/model", "/usr/share/vmaf/model", "/usr/local/share/model"]
        };

        for path in required {
            assert!(
                paths.iter().any(|candidate| candidate == path),
                "`{path}` should be searched; candidates are {paths:?}"
            );
        }
    }

    /// Windows must never assume MSYS2 lives on `C:`. The root comes from the
    /// machine, so the layouts are derived from whatever root is supplied.
    #[cfg(target_os = "windows")]
    #[test]
    fn the_msys2_layouts_follow_the_root_the_machine_reports() {
        for root in [r"D:\msys64", r"E:\tools\msys2", r"\\server\share\msys64"] {
            let paths: Vec<String> = msys2_model_directories(Path::new(root))
                .into_iter()
                .map(|(path, _)| path.display().to_string())
                .collect();

            for prefix in ["mingw64", "ucrt64"] {
                let expected = Path::new(root)
                    .join(prefix)
                    .join("share")
                    .join("vmaf")
                    .join("model")
                    .display()
                    .to_string();
                assert!(
                    paths.contains(&expected),
                    "`{expected}` should be searched for root `{root}`; candidates are {paths:?}"
                );
            }
        }
    }

    /// System paths must be searched before anything near the executable, so a
    /// stray `vmaf_v0.6.1.json` in a shared or downloads-adjacent binary
    /// directory cannot silently override the distribution's model.
    #[test]
    fn system_model_paths_take_precedence_over_executable_adjacent_ones() {
        // `VMAF_MODEL_PATH` is an explicit override and is deliberately searched
        // first, so drop it before comparing orderings. Comparing by value rather
        // than by position keeps this correct if it is unset, which is the case
        // in CI.
        let override_path = std::env::var("VMAF_MODEL_PATH").ok();

        let paths: Vec<String> = VmafModel::model_search_paths()
            .into_iter()
            .filter(|path| Some(&path.display().to_string()) != override_path.as_ref())
            .map(|path| path.display().to_string())
            .collect();

        let last_system = paths.iter().rposition(|path| is_system_model_path(path));
        let first_adjacent = paths.iter().position(|path| !is_system_model_path(path));

        if let (Some(last_system), Some(first_adjacent)) = (last_system, first_adjacent) {
            assert!(
                last_system < first_adjacent,
                "every system path must precede every other path; a system path at {last_system} \
                 follows a non-system path at {first_adjacent}. Candidates are {paths:?}"
            );
        }
    }

    /// A library loaded from an MSYS2 prefix emits a system path and an
    /// adjacent path from the same directory, and the adjacent one must not
    /// precede it.
    ///
    /// This is the case that was previously only reachable with a real MSYS2
    /// install, which is why it never ran in CI: the loaded-library block
    /// contributed nothing when libvmaf was absent. The directory is built by
    /// hand here so the regression is caught on any machine.
    #[test]
    fn a_library_prefix_places_its_system_path_before_its_adjacent_ones() {
        let root = PathBuf::from("/vmaf-test-root");
        let loaded_from = root.join("mingw64").join("bin");
        let paths: Vec<String> = model_search_paths_for(Some(&loaded_from))
            .into_iter()
            .map(|path| path.display().to_string())
            .collect();

        // The invariant the two buckets exist to guarantee: once a path that is
        // not system-controlled has been emitted, no system path may follow.
        let last_system = paths.iter().rposition(|path| is_system_model_path(path));
        let first_adjacent = paths.iter().position(|path| !is_system_model_path(path));

        if let (Some(last_system), Some(first_adjacent)) = (last_system, first_adjacent) {
            assert!(
                last_system < first_adjacent,
                "a library prefix emitted a system path at {last_system} behind an adjacent one \
                 at {first_adjacent}. Candidates are {paths:?}"
            );
        }

        // The specific inference that regressed: the loaded directory names both a
        // `share/vmaf/model` and a `model` directory, and the former must win.
        assert!(
            paths.iter().any(|path| path.contains("share")),
            "the loaded directory should imply a `share/vmaf/model` directory. Candidates are \
             {paths:?}"
        );
    }
}
