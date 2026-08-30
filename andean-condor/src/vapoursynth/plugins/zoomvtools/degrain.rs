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

/// ZooMVTools Degrain1 plugin (implemented as Degrain) - motion-compensated
/// temporal denoiser using a single backward/forward vector pair.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Degrain {
    /// Only used for script generation
    pub clip_name:       Option<NodeVariableName>,
    /// Only used for script generation
    pub super_clip_name: Option<NodeVariableName>,
    /// Only used for script generation
    pub mvbw_name:       Option<NodeVariableName>,
    /// Only used for script generation
    pub mvfw_name:       Option<NodeVariableName>,
    /// SAD at which a reference block's weight reaches zero. Default 400.
    pub thsad:           Option<u32>,
    /// Chroma SAD threshold. Defaults to thsad.
    pub thsadc:          Option<u32>,
    /// Which planes to process: -1=all, 0=luma, 1=chroma U, 2=chroma V.
    /// Default -1.
    pub plane:           Option<i32>,
    /// Maximum absolute change per pixel (luma). Default 0.
    pub limit:           Option<u32>,
    /// Maximum absolute change per pixel (chroma). Defaults to limit.
    pub limitc:          Option<u32>,
    /// Scene-change SAD threshold. Default 130.
    pub thscd1:          Option<u32>,
    /// Percentage of blocks that must be "changed" for scene change. Default
    /// 18.
    pub thscd2:          Option<u32>,
    /// CPU optimizations. Default 4.
    pub opt:             Option<u32>,
}

impl Plugin for Degrain {
    const PLUGIN_NAME: &'static str = NAME;
    const PLUGIN_ID: &'static str = ID;
    const PLUGIN_DOCS: Option<&'static str> = Some(DOCS);
}

impl PluginFunction for Degrain {
    const FUNCTION_NAME: &'static str = "Degrain1";
    const FUNCTION_DOCS: Option<&'static str> =
        Some("https://gitlab.com/shssoichiro/vapoursynth-zoomvtools/-/blob/main/USAGE.md?ref_type=heads#degrain1");
    const REQUIRED_ARGUMENTS: &'static [(&'static str, &'static ValueType)] = &[
        ("clip", &ValueType::VideoNode),
        ("super_clip", &ValueType::VideoNode),
        ("mvbw", &ValueType::VideoNode),
        ("mvfw", &ValueType::VideoNode),
    ];
    const OPTIONAL_ARGUMENTS: &'static [(&'static str, &'static ValueType)] = &[
        ("thsad", &ValueType::Int),
        ("thsadc", &ValueType::Int),
        ("plane", &ValueType::Int),
        ("limit", &ValueType::Int),
        ("limitc", &ValueType::Int),
        ("thscd1", &ValueType::Int),
        ("thscd2", &ValueType::Int),
        ("opt", &ValueType::Int),
    ];
}

impl Degrain {
    #[inline]
    pub fn invoke<'core>(
        self,
        core: CoreRef<'core>,
        clip: &Node<'core>,
        super_clip: &Node<'core>,
        mvbw: &Node<'core>,
        mvfw: &Node<'core>,
    ) -> Result<Node<'core>, VapourSynthError> {
        let mut arguments = Self::arguments()?;
        arguments
            .set_node("clip", clip)
            .map_err(|e| VapourSynthError::PluginArgumentsError {
                plugin:   Self::PLUGIN_NAME.to_owned(),
                argument: "clip".to_owned(),
                message:  e.to_string(),
            })?;
        arguments.set_node("super_clip", super_clip).map_err(|e| {
            VapourSynthError::PluginArgumentsError {
                plugin:   Self::PLUGIN_NAME.to_owned(),
                argument: "super_clip".to_owned(),
                message:  e.to_string(),
            }
        })?;
        arguments
            .set_node("mvbw", mvbw)
            .map_err(|e| VapourSynthError::PluginArgumentsError {
                plugin:   Self::PLUGIN_NAME.to_owned(),
                argument: "mvbw".to_owned(),
                message:  e.to_string(),
            })?;
        arguments
            .set_node("mvfw", mvfw)
            .map_err(|e| VapourSynthError::PluginArgumentsError {
                plugin:   Self::PLUGIN_NAME.to_owned(),
                argument: "mvfw".to_owned(),
                message:  e.to_string(),
            })?;
        Self::argument_set_ints(&mut arguments, vec![
            ("thsad", self.thsad),
            ("thsadc", self.thsadc),
            ("limit", self.limit),
            ("limitc", self.limitc),
            ("thscd1", self.thscd1),
            ("thscd2", self.thscd2),
            ("opt", self.opt),
        ])?;
        // plane is signed (-1 = all)
        Self::argument_set_int(&mut arguments, "plane", self.plane.map(|d| d as i64))?;
        let node = Self::invoke_and_get_node(core, arguments, Some("clip"))?;
        Ok(node)
    }
}

impl VapourSynthPluginScript for Degrain {
    #[inline]
    fn generate_script(&self, node_name: NodeVariableName) -> Result<(Option<Imports>, Vec<Line>)> {
        let mut lines = vec![];

        let line = {
            let mut line = String::new();
            let clip_name = self.clip_name.as_ref().unwrap_or(&node_name);
            let super_clip_name = self.super_clip_name.as_ref().unwrap_or(&node_name);
            let mvbw_name = self.mvbw_name.as_ref().unwrap_or(&node_name);
            let mvfw_name = self.mvfw_name.as_ref().unwrap_or(&node_name);
            write!(
                &mut line,
                "core.zoomv.Degrain1(clip = {}, super_clip = {}, mvbw = {}, mvfw = {}",
                clip_name, super_clip_name, mvbw_name, mvfw_name
            )?;
            if let Some(thsad) = self.thsad {
                write!(&mut line, ", thsad = {}", thsad as i64)?;
            }
            if let Some(thsadc) = self.thsadc {
                write!(&mut line, ", thsadc = {}", thsadc as i64)?;
            }
            if let Some(plane) = self.plane {
                write!(&mut line, ", plane = {}", plane)?;
            }
            if let Some(limit) = self.limit {
                write!(&mut line, ", limit = {}", limit as i64)?;
            }
            if let Some(limitc) = self.limitc {
                write!(&mut line, ", limitc = {}", limitc as i64)?;
            }
            if let Some(scd1) = self.thscd1 {
                write!(&mut line, ", thscd1 = {}", scd1 as i64)?;
            }
            if let Some(scd2) = self.thscd2 {
                write!(&mut line, ", thscd2 = {}", scd2 as i64)?;
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
