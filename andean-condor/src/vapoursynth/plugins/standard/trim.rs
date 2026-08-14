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
pub struct Trim {
    pub first:  Option<u32>,
    pub last:   Option<u32>,
    pub length: Option<u32>,
}

impl Plugin for Trim {
    const PLUGIN_NAME: &'static str = NAME;
    const PLUGIN_ID: &'static str = ID;
    const PLUGIN_DOCS: Option<&'static str> = Some(DOCS);
}

impl PluginFunction for Trim {
    const FUNCTION_NAME: &'static str = "Trim";
    const FUNCTION_DOCS: Option<&'static str> =
        Some("https://www.vapoursynth.com/doc/functions/video/trim.html");
    const REQUIRED_ARGUMENTS: &'static [(&'static str, &'static ValueType)] =
        &[("clip", &ValueType::VideoNode)];
    const OPTIONAL_ARGUMENTS: &'static [(&'static str, &'static ValueType)] = &[
        ("first", &ValueType::Int),
        ("last", &ValueType::Int),
        ("length", &ValueType::Int),
    ];
}

impl Trim {
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
            ("first", self.first),
            ("last", self.last),
            ("length", self.length),
        ])?;
        let node = Self::invoke_and_get_node(core, arguments, Some("clip"))?;

        Ok(node)
    }
}

impl VapourSynthPluginScript for Trim {
    #[inline]
    fn generate_script(&self, node_name: NodeVariableName) -> Result<(Option<Imports>, Vec<Line>)> {
        let mut lines = vec![];

        let line = {
            let mut line = String::new();
            write!(&mut line, "core.std.Trim(clip = {}", node_name)?;
            if let Some(first) = self.first {
                write!(&mut line, ", first = {}", first)?;
            }
            if let Some(last) = self.last {
                write!(&mut line, ", last = {}", last)?;
            }
            if let Some(length) = self.length {
                write!(&mut line, ", length = {}", length)?;
            }
            write!(&mut line, ")")?;
            line
        };

        lines.push(Line::Expression(node_name, line));

        Ok((None, lines))
    }
}
