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
        vszip::{DOCS, ID, NAME},
    },
    script_builder::{
        NodeVariableName,
        VapourSynthPluginScript,
        script::{Imports, Line},
    },
};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BoxBlur {
    pub planes:  Option<Vec<u32>>,
    pub hradius: Option<u32>,
    pub hpasses: Option<u32>,
    pub vradius: Option<u32>,
    pub vpasses: Option<u32>,
}

impl Plugin for BoxBlur {
    const PLUGIN_NAME: &'static str = NAME;
    const PLUGIN_ID: &'static str = ID;
    const PLUGIN_DOCS: Option<&'static str> = Some(DOCS);
}

impl PluginFunction for BoxBlur {
    const FUNCTION_NAME: &'static str = "BoxBlur";
    const FUNCTION_DOCS: Option<&'static str> =
        Some("https://github.com/dnjulek/vapoursynth-zip/wiki/BoxBlur");
    const REQUIRED_ARGUMENTS: &'static [(&'static str, &'static ValueType)] =
        &[("clip", &ValueType::VideoNode), ("planes", &ValueType::Int)];
    const OPTIONAL_ARGUMENTS: &'static [(&'static str, &'static ValueType)] = &[
        ("hradius", &ValueType::Int),
        ("hpasses", &ValueType::Int),
        ("vradius", &ValueType::Int),
        ("vpasses", &ValueType::Int),
    ];
}

impl BoxBlur {
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
            ("hradius", self.hradius),
            ("hpasses", self.hpasses),
            ("vradius", self.vradius),
            ("vpasses", self.vpasses),
        ])?;
        Self::argument_set_int_arrays(&mut arguments, vec![("planes", self.planes)])?;
        let node = Self::invoke_and_get_node(core, arguments, Some("clip"))?;

        Ok(node)
    }
}

impl VapourSynthPluginScript for BoxBlur {
    #[inline]
    fn generate_script(&self, node_name: NodeVariableName) -> Result<(Option<Imports>, Vec<Line>)> {
        let mut lines = vec![];

        let line = {
            let mut line = String::new();
            write!(&mut line, "core.vszip.BoxBlur(clip = {}", node_name)?;
            if let Some(planes) = &self.planes {
                write!(&mut line, ", planes = [{}]", planes.iter().join(", "))?;
            }
            if let Some(hradius) = self.hradius {
                write!(&mut line, ", hradius = {}", hradius)?;
            }
            if let Some(hpasses) = self.hpasses {
                write!(&mut line, ", hpasses = {}", hpasses)?;
            }
            if let Some(vradius) = self.vradius {
                write!(&mut line, ", vradius = {}", vradius)?;
            }
            if let Some(vpasses) = self.vpasses {
                write!(&mut line, ", vpasses = {}", vpasses)?;
            }
            write!(&mut line, ")")?;
            line
        };

        lines.push(Line::Expression(node_name, line));

        Ok((None, lines))
    }
}
