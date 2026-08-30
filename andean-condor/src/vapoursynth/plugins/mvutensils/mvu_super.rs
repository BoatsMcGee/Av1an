use std::fmt::Write;

use anyhow::Result;
use itertools::Itertools;
use serde::{Deserialize, Serialize};
use vapoursynth::{core::CoreRef, map::ValueType, node::Node};

use crate::vapoursynth::{
    VapourSynthError,
    plugins::{
        Plugin,
        PluginFunction,
        mvutensils::{DOCS, ID, NAME},
    },
    script_builder::{
        NodeVariableName,
        VapourSynthPluginScript,
        script::{Imports, Line},
    },
};

/// MVUtensils Super plugin - prepares a clip for motion estimation by
/// padding frames, optionally generating sub-pixel planes, and building
/// the hierarchical pyramid used by Analyse.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MVUSuper {
    /// Block size [h, v]. A single value sets both.
    /// Used to pad the frame so right/bottom edges are fully covered. Default
    /// [8, 8].
    pub blksize:      Option<Vec<u32>>,
    /// Block overlap [h, v], must be ≤ blksize/2. Default [4, 4].
    pub overlap:      Option<Vec<u32>>,
    /// Border padding [h, v] in pixels. Default [16, 16].
    pub pad:          Option<Vec<u32>>,
    /// Sub-pixel accuracy: 1=full, 2=half, 4=quarter. Default 2.
    pub pel:          Option<u32>,
    /// Sub-pixel interpolation for pel>1: 0=bilinear, 1=bicubic, 2=Wiener.
    /// Default 2.
    pub sharp:        Option<u32>,
    /// Pyramid downscale filter: 0=simple average, 1=bilinear, 2=cubic. Default
    /// 1.
    pub rfilter:      Option<u32>,
    /// If True, generate only one level (saves memory). Default False.
    pub onelevel:     Option<bool>,
    /// Only used for script generation
    pub pelclip_name: Option<NodeVariableName>,
    /// Prefix for frame properties. Default "MVUtensils".
    pub prefix:       Option<String>,
}

impl Plugin for MVUSuper {
    const PLUGIN_NAME: &'static str = NAME;
    const PLUGIN_ID: &'static str = ID;
    const PLUGIN_DOCS: Option<&'static str> = Some(DOCS);
}

impl PluginFunction for MVUSuper {
    const FUNCTION_NAME: &'static str = "Super";
    const FUNCTION_DOCS: Option<&'static str> =
        Some("https://github.com/myrsloik/mvutensils#super");
    const REQUIRED_ARGUMENTS: &'static [(&'static str, &'static ValueType)] = &[
        ("clip", &ValueType::VideoNode),
        ("blksize", &ValueType::Int),
        ("overlap", &ValueType::Int),
    ];
    const OPTIONAL_ARGUMENTS: &'static [(&'static str, &'static ValueType)] = &[
        ("pad", &ValueType::Int),
        ("pel", &ValueType::Int),
        ("sharp", &ValueType::Int),
        ("rfilter", &ValueType::Int),
        ("onelevel", &ValueType::Int),
        ("pelclip", &ValueType::VideoNode),
        ("prefix", &ValueType::Data),
    ];
}

impl MVUSuper {
    #[inline]
    pub fn invoke<'core>(
        self,
        core: CoreRef<'core>,
        node: &Node<'core>,
        pelclip: Option<&Node<'core>>,
    ) -> Result<Node<'core>, VapourSynthError> {
        let mut arguments = Self::arguments()?;
        arguments
            .set_node("clip", node)
            .map_err(|e| VapourSynthError::PluginArgumentsError {
                plugin:   Self::PLUGIN_NAME.to_owned(),
                argument: "clip".to_owned(),
                message:  e.to_string(),
            })?;
        if let Some(pelclip) = pelclip {
            arguments.set_node("pelclip", pelclip).map_err(|e| {
                VapourSynthError::PluginArgumentsError {
                    plugin:   Self::PLUGIN_NAME.to_owned(),
                    argument: "pelclip".to_owned(),
                    message:  e.to_string(),
                }
            })?;
        }
        Self::argument_set_int_arrays(&mut arguments, vec![
            ("blksize", self.blksize.or_else(|| Some(vec![8]))),
            ("overlap", self.overlap.or_else(|| Some(vec![4]))),
            ("pad", self.pad),
        ])?;
        Self::argument_set_ints(&mut arguments, vec![
            ("pel", self.pel),
            ("sharp", self.sharp),
            ("rfilter", self.rfilter),
            ("onelevel", self.onelevel.map(|b| if b { 1 } else { 0 })),
        ])?;
        Self::arguments_set(&mut arguments, vec![("prefix", self.prefix)])?;
        let node = Self::invoke_and_get_node(core, arguments, Some("clip"))?;
        Ok(node)
    }
}

impl VapourSynthPluginScript for MVUSuper {
    #[inline]
    fn generate_script(&self, node_name: NodeVariableName) -> Result<(Option<Imports>, Vec<Line>)> {
        let mut lines = vec![];

        let line = {
            let mut line = String::new();
            write!(&mut line, "core.mvu.Super(clip = {}", node_name)?;
            let blksize = self.blksize.clone().unwrap_or_else(|| vec![8]);
            let overlap = self.overlap.clone().unwrap_or_else(|| vec![4]);
            write!(&mut line, ", blksize = [{}]", blksize.iter().join(", "))?;
            write!(&mut line, ", overlap = [{}]", overlap.iter().join(", "))?;
            if let Some(pad) = &self.pad {
                write!(&mut line, ", pad = [{}]", pad.iter().join(", "))?;
            }
            if let Some(pel) = self.pel {
                write!(&mut line, ", pel = {}", pel as i64)?;
            }
            if let Some(sharp) = self.sharp {
                write!(&mut line, ", sharp = {}", sharp as i64)?;
            }
            if let Some(rfilter) = self.rfilter {
                write!(&mut line, ", rfilter = {}", rfilter as i64)?;
            }
            if let Some(onelevel) = self.onelevel {
                write!(&mut line, ", onelevel = {}", onelevel as i64)?;
            }
            if let Some(pelclip_name) = &self.pelclip_name {
                write!(&mut line, ", pelclip = {}", pelclip_name)?;
            }
            if let Some(prefix) = &self.prefix {
                write!(&mut line, ", prefix = \"{}\"", prefix)?;
            }
            write!(&mut line, ")")?;
            line
        };

        lines.push(Line::Expression(node_name, line));

        Ok((None, lines))
    }
}
