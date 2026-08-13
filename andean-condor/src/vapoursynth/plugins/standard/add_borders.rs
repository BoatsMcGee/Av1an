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
        standard::{DOCS, ID, NAME},
    },
    script_builder::{
        NodeVariableName,
        VapourSynthPluginScript,
        script::{Imports, Line},
    },
};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AddBorders {
    pub top:    Option<u32>,
    pub bottom: Option<u32>,
    pub left:   Option<u32>,
    pub right:  Option<u32>,
    pub color:  Option<Vec<f64>>,
}

impl Plugin for AddBorders {
    const PLUGIN_NAME: &'static str = NAME;
    const PLUGIN_ID: &'static str = ID;
    const PLUGIN_DOCS: Option<&'static str> = Some(DOCS);
}

impl PluginFunction for AddBorders {
    const FUNCTION_NAME: &'static str = "AddBorders";
    const FUNCTION_DOCS: Option<&'static str> =
        Some("https://www.vapoursynth.com/doc/functions/video/addborders.html");
    const REQUIRED_ARGUMENTS: &'static [(&'static str, &'static ValueType)] =
        &[("clip", &ValueType::VideoNode)];
    const OPTIONAL_ARGUMENTS: &'static [(&'static str, &'static ValueType)] = &[
        ("top", &ValueType::Int),
        ("bottom", &ValueType::Int),
        ("left", &ValueType::Int),
        ("right", &ValueType::Int),
        ("color", &ValueType::Float),
    ];
}

impl AddBorders {
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
        Self::arguments_set_float_arrays(&mut arguments, vec![("color", self.color)])?;
        let node = Self::invoke_and_get_node(core, arguments, Some("clip"))?;

        Ok(node)
    }
}

impl VapourSynthPluginScript for AddBorders {
    #[inline]
    fn generate_script(&self, node_name: NodeVariableName) -> Result<(Option<Imports>, Vec<Line>)> {
        let mut lines = vec![];

        let line = {
            let mut line = String::new();
            write!(&mut line, "core.std.AddBorders(clip = {}", node_name)?;
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
            if let Some(color) = &self.color {
                write!(&mut line, ", color = [{}]", color.iter().join(", "))?;
            }
            write!(&mut line, ")")?;
            line
        };

        lines.push(Line::Expression(node_name, line));

        Ok((None, lines))
    }
}
