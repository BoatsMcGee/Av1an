use std::{collections::HashMap, path::PathBuf};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Output {
    pub path: PathBuf,
    /// Explicit consent to replace a distinct final output after successful muxing.
    #[serde(default)]
    pub overwrite: bool,
    pub tags: HashMap<String, String>,
    pub video_tags: HashMap<String, String>,
}
