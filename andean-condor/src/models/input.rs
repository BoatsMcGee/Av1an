use std::{collections::HashMap, path::PathBuf};

use serde::{Deserialize, Serialize};
use strum::{Display, EnumString, IntoStaticStr};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Input {
    Video {
        path:          PathBuf,
        import_method: ImportMethod,
    },
    VapourSynth {
        path:          PathBuf,
        import_method: VapourSynthImportMethod,
        cache_path:    Option<PathBuf>,
    },
    VapourSynthScript {
        source:    VapourSynthScriptSource,
        variables: HashMap<String, String>,
        index:     u8,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, EnumString, IntoStaticStr, Display)]
pub enum VapourSynthImportMethod {
    /// [L-SMASH-Works](https://github.com/HomeOfAviSynthPlusEvolution/L-SMASH-Works)
    LSMASHWorks {
        // plugin_path: Option<PathBuf>,
        index: Option<u8>,
        // cache_path: Option<PathBuf>,
    },
    /// [DGDecodeNV](https://www.rationalqm.us/dgdecnv/dgdecnv.html)
    DGDecNV {
        // plugin_path:          Option<PathBuf>,
        // cache_path:           Option<PathBuf>,
        dgindexnv_executable: Option<PathBuf>,
    },
    /// [FFmpegSource](https://github.com/ffms/ffms2)
    FFMS2 {
        // plugin_path: Option<PathBuf>,
        index: Option<u8>,
        // cache_path: Option<PathBuf>,
    },
    /// [BestSource](https://github.com/vapoursynth/bestsource).
    BestSource {
        // plugin_path: Option<PathBuf>,
        index: Option<u8>,
        // cache_path: Option<PathBuf>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, EnumString, IntoStaticStr, Display)]
pub enum ImportMethod {
    // FFmpeg {}, // Unsupported
    FFMS2 { index: Option<u8> },
}

#[derive(Debug, Clone, Serialize, Deserialize, EnumString, IntoStaticStr, Display)]
pub enum VapourSynthScriptSource {
    Path(PathBuf),
    Text(String),
}
