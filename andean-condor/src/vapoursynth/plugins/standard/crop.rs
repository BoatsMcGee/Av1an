use std::fmt::Write;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use vapoursynth::{core::CoreRef, map::ValueType, node::Node};

use crate::vapoursynth::{
    VapourSynthError,
    plugins::{
        Plugin,
        PluginFunction,
        standard::{DOCS, ID, NAME},
    },
    script_builder::{
        NodeVariableName,
        VapourSynthPluginScript,
        script::{Imports, Line},
    },
};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Crop {
    pub top:    Option<u32>,
    pub bottom: Option<u32>,
    pub left:   Option<u32>,
    pub right:  Option<u32>,
}

impl Plugin for Crop {
    const PLUGIN_NAME: &'static str = NAME;
    const PLUGIN_ID: &'static str = ID;
    const PLUGIN_DOCS: Option<&'static str> = Some(DOCS);
}

impl PluginFunction for Crop {
    const FUNCTION_NAME: &'static str = "Crop";
    const FUNCTION_DOCS: Option<&'static str> =
        Some("https://www.vapoursynth.com/doc/functions/video/crop_cropabs.html#std.Crop");
    const REQUIRED_ARGUMENTS: &'static [(&'static str, &'static ValueType)] =
        &[("clip", &ValueType::VideoNode)];
    const OPTIONAL_ARGUMENTS: &'static [(&'static str, &'static ValueType)] = &[
        ("top", &ValueType::Int),
        ("bottom", &ValueType::Int),
        ("left", &ValueType::Int),
        ("right", &ValueType::Int),
    ];
}

impl Crop {
    #[inline]
    pub fn invoke<'core>(
        self,
        core: CoreRef<'core>,
        node: &Node<'core>,
    ) -> Result<Node<'core>, VapourSynthError> {
        let mut arguments = Self::arguments()?;
        arguments
            .set_node("clip", node)
            .map_err(|e| VapourSynthError::PluginArgumentsError {
                plugin:   Self::PLUGIN_NAME.to_owned(),
                argument: "clip".to_owned(),
                message:  e.to_string(),
            })?;
        Self::argument_set_ints(&mut arguments, vec![
            ("top", self.top),
            ("bottom", self.bottom),
            ("left", self.left),
            ("right", self.right),
        ])?;
        let node = Self::invoke_and_get_node(core, arguments, Some("clip"))?;

        Ok(node)
    }
}

impl VapourSynthPluginScript for Crop {
    #[inline]
    fn generate_script(&self, node_name: NodeVariableName) -> Result<(Option<Imports>, Vec<Line>)> {
        let mut lines = vec![];

        let line = {
            let mut line = String::new();
            write!(&mut line, "core.std.Crop(clip = {}", node_name)?;
            if let Some(top) = self.top {
                write!(&mut line, ", top = {}", top)?;
            }
            if let Some(bottom) = self.bottom {
                write!(&mut line, ", bottom = {}", bottom)?;
            }
            if let Some(left) = self.left {
                write!(&mut line, ", left = {}", left)?;
            }
            if let Some(right) = self.right {
                write!(&mut line, ", right = {}", right)?;
            }
            write!(&mut line, ")")?;
            line
        };

        lines.push(Line::Expression(node_name, line));

        Ok((None, lines))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CropAbs {
    width:    u32,
    height:   u32,
    pub left: Option<u32>,
    pub top:  Option<u32>,
}

impl Plugin for CropAbs {
    const PLUGIN_NAME: &'static str = NAME;
    const PLUGIN_ID: &'static str = ID;
    const PLUGIN_DOCS: Option<&'static str> = Some(DOCS);
}

impl PluginFunction for CropAbs {
    const FUNCTION_NAME: &'static str = "CropAbs";
    const FUNCTION_DOCS: Option<&'static str> =
        Some("https://www.vapoursynth.com/doc/functions/video/crop_cropabs.html#std.CropAbs");
    const REQUIRED_ARGUMENTS: &'static [(&'static str, &'static ValueType)] = &[
        ("clip", &ValueType::VideoNode),
        ("width", &ValueType::Int),
        ("height", &ValueType::Int),
    ];
    const OPTIONAL_ARGUMENTS: &'static [(&'static str, &'static ValueType)] =
        &[("left", &ValueType::Int), ("top", &ValueType::Int)];
}

impl CropAbs {
    #[inline]
    pub fn invoke<'core>(
        self,
        core: CoreRef<'core>,
        node: &Node<'core>,
    ) -> Result<Node<'core>, VapourSynthError> {
        let mut arguments = Self::arguments()?;
        arguments
            .set_node("clip", node)
            .map_err(|e| VapourSynthError::PluginArgumentsError {
                plugin:   Self::PLUGIN_NAME.to_owned(),
                argument: "clip".to_owned(),
                message:  e.to_string(),
            })?;
        Self::argument_set_ints(&mut arguments, vec![
            ("width", Some(self.width)),
            ("height", Some(self.height)),
            ("left", self.left),
            ("top", self.top),
        ])?;
        let node = Self::invoke_and_get_node(core, arguments, Some("clip"))?;

        Ok(node)
    }
}

impl VapourSynthPluginScript for CropAbs {
    #[inline]
    fn generate_script(&self, node_name: NodeVariableName) -> Result<(Option<Imports>, Vec<Line>)> {
        let mut lines = vec![];

        let line = {
            let mut line = String::new();
            write!(&mut line, "core.std.CropAbs(clip = {}", node_name)?;
            write!(&mut line, ", width = {}", self.width)?;
            write!(&mut line, ", height = {}", self.height)?;
            if let Some(left) = self.left {
                write!(&mut line, ", left = {}", left)?;
            }
            if let Some(top) = self.top {
                write!(&mut line, ", top = {}", top)?;
            }
            write!(&mut line, ")")?;
            line
        };

        lines.push(Line::Expression(node_name, line));

        Ok((None, lines))
    }
}
