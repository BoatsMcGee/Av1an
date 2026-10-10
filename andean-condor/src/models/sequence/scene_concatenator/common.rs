use std::collections::HashMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Serde default for the `copy` fields: output settings copy the source's
/// tracks, chapters, and metadata unless a user opts out.
#[inline]
#[must_use]
pub fn default_true() -> bool {
    true
}

/// Global metadata written to the output.
///
/// `None` on [`super::mkvmerge::MkvmergeConfig::metadata`] means "copy the
/// source's global tags" (the default). When present, `copy` controls whether
/// the source's own global tags are kept, and `title`/`tags` describe the
/// output's tags.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Metadata {
    /// Whether to copy global tags from the source. Defaults to `true`.
    #[serde(default = "default_true")]
    pub copy:  bool,
    /// Global title for the output.
    #[serde(default)]
    pub title: Option<String>,
    /// Global tags as key/value pairs.
    #[serde(default)]
    pub tags:  HashMap<String, String>,
}

/// Chapter handling.
///
/// `None` means "copy the source's chapters" (the default). When present,
/// `copy` controls whether source chapters are kept, and `language`/`charset`
/// override where mkvmerge would otherwise guess.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Chapters {
    /// Whether to copy chapters from the source. Defaults to `true`.
    #[serde(default = "default_true")]
    pub copy:     bool,
    /// ISO 639-2 language code written for every chapter entry.
    #[serde(default)]
    pub language: Option<String>,
    /// Character set used to decode simple chapter files.
    #[serde(default)]
    pub charset:  Option<String>,
}
