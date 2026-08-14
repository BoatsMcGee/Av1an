use std::fmt::Write;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use vapoursynth::{core::CoreRef, map::ValueType, node::Node};

use crate::vapoursynth::{
    VapourSynthError,
    plugins::{
        Plugin,
        PluginFunction,
        zoomvtools::{DOCS, ID, NAME},
    },
    script_builder::{
        NodeVariableName,
        VapourSynthPluginScript,
        script::{Imports, Line},
    },
};

/// ZooMVTools Super plugin - prepares a clip for motion estimation by
/// padding frames, optionally generating sub-pixel planes, and building
/// the hierarchical pyramid used by Analyse.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ZMVSuper {
    /// Horizontal padding in pixels. Default 16.
    pub hpad:         Option<u32>,
    /// Vertical padding in pixels. Default 16.
    pub vpad:         Option<u32>,
    /// Sub-pixel accuracy: 1=full, 2=half, 4=quarter. Default 2.
    pub pel:          Option<u32>,
    /// Number of hierarchical levels to use. 0 = all. Default 0.
    pub levels:       Option<u32>,
    /// Include chroma planes. Default True.
    pub chroma:       Option<bool>,
    /// Sub-pixel interpolation for pel>1: 0=bilinear, 1=bicubic, 2=Wiener.
    /// Default 2.
    pub sharp:        Option<u32>,
    /// Pyramid downscale filter: 0=simple average, 1=bilinear, 2=cubic. Default
    /// 1.
    pub rfilter:      Option<u32>,
    /// Only used for script generation
    pub pelclip_name: Option<NodeVariableName>,
    /// CPU optimizations. Default 4.
    pub opt:          Option<u32>,
}

impl Plugin for ZMVSuper {
    const PLUGIN_NAME: &'static str = NAME;
    const PLUGIN_ID: &'static str = ID;
    const PLUGIN_DOCS: Option<&'static str> = Some(DOCS);
}

impl PluginFunction for ZMVSuper {
    const FUNCTION_NAME: &'static str = "Super";
    const FUNCTION_DOCS: Option<&'static str> = Some("https://gitlab.com/shssoichiro/vapoursynth-zoomvtools/-/blob/main/USAGE.md?ref_type=heads#super");
    const REQUIRED_ARGUMENTS: &'static [(&'static str, &'static ValueType)] =
        &[("clip", &ValueType::VideoNode)];
    const OPTIONAL_ARGUMENTS: &'static [(&'static str, &'static ValueType)] = &[
        ("hpad", &ValueType::Int),
        ("vpad", &ValueType::Int),
        ("pel", &ValueType::Int),
        ("levels", &ValueType::Int),
        ("chroma", &ValueType::Int),
        ("sharp", &ValueType::Int),
        ("rfilter", &ValueType::Int),
        ("pelclip", &ValueType::VideoNode),
        ("opt", &ValueType::Int),
    ];
}

impl ZMVSuper {
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
        Self::argument_set_ints(&mut arguments, vec![
            ("hpad", self.hpad),
            ("vpad", self.vpad),
            ("pel", self.pel),
            ("levels", self.levels),
            ("chroma", self.chroma.map(|b| if b { 1 } else { 0 })),
            ("sharp", self.sharp),
            ("rfilter", self.rfilter),
            ("opt", self.opt),
        ])?;
        let node = Self::invoke_and_get_node(core, arguments, Some("clip"))?;
        Ok(node)
    }
}

impl VapourSynthPluginScript for ZMVSuper {
    #[inline]
    fn generate_script(&self, node_name: NodeVariableName) -> Result<(Option<Imports>, Vec<Line>)> {
        let mut lines = vec![];

        let line = {
            let mut line = String::new();
            write!(&mut line, "core.zoomv.Super(clip = {}", node_name)?;
            if let Some(hpad) = self.hpad {
                write!(&mut line, ", hpad = {}", hpad as i64)?;
            }
            if let Some(vpad) = self.vpad {
                write!(&mut line, ", vpad = {}", vpad as i64)?;
            }
            if let Some(pel) = self.pel {
                write!(&mut line, ", pel = {}", pel as i64)?;
            }
            if let Some(levels) = self.levels {
                write!(&mut line, ", levels = {}", levels as i64)?;
            }
            if let Some(chroma) = self.chroma {
                write!(&mut line, ", chroma = {}", chroma as i64)?;
            }
            if let Some(sharp) = self.sharp {
                write!(&mut line, ", sharp = {}", sharp as i64)?;
            }
            if let Some(rfilter) = self.rfilter {
                write!(&mut line, ", rfilter = {}", rfilter as i64)?;
            }
            if let Some(pelclip_name) = &self.pelclip_name {
                write!(&mut line, ", pelclip = {}", pelclip_name)?;
            }
            if let Some(opt) = self.opt {
                write!(&mut line, ", opt = {}", opt as i64)?;
            }
            write!(&mut line, ")")?;
            line
        };

        lines.push(Line::Expression(node_name, line));

        Ok((None, lines))
    }
}
