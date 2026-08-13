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

/// MVUtensils Degrain plugin - motion-compensated temporal denoiser.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Degrain {
    /// Only used for script generation
    pub clip_name:    Option<NodeVariableName>,
    /// Only used for script generation
    pub super_name:   Option<NodeVariableName>,
    /// Only used for script generation
    pub vectors_name: Option<NodeVariableName>,
    /// SAD [luma, chroma] at which a reference block's weight reaches zero.
    pub thsad:        Option<Vec<u32>>,
    /// SAD [luma, chroma] for the furthest references.
    pub thsad2:       Option<Vec<u32>>,
    /// Which planes to process. Default [0, 1, 2].
    pub planes:       Option<Vec<u32>>,
    /// Maximum absolute change per pixel [luma, chroma]. Default [inf, inf].
    pub limit:        Option<Vec<f64>>,
    /// Scene-change SAD threshold. Default 400.
    pub thscd1:       Option<u32>,
    /// Percentage of blocks that must be "changed" for scene change. Default
    /// 51.
    pub thscd2:       Option<f64>,
    /// Optional per-frame bias weights.
    pub weights:      Option<Vec<u32>>,
    /// Prefix for frame properties.
    pub prefix:       Option<String>,
}

impl Plugin for Degrain {
    const PLUGIN_NAME: &'static str = NAME;
    const PLUGIN_ID: &'static str = ID;
    const PLUGIN_DOCS: Option<&'static str> = Some(DOCS);
}

impl PluginFunction for Degrain {
    const FUNCTION_NAME: &'static str = "Degrain";
    const FUNCTION_DOCS: Option<&'static str> =
        Some("https://github.com/myrsloik/mvutensils#degrain");
    const REQUIRED_ARGUMENTS: &'static [(&'static str, &'static ValueType)] = &[
        ("clip", &ValueType::VideoNode),
        ("super", &ValueType::VideoNode),
        ("vectors", &ValueType::VideoNode),
    ];
    const OPTIONAL_ARGUMENTS: &'static [(&'static str, &'static ValueType)] = &[
        ("thsad", &ValueType::Int),
        ("thsad2", &ValueType::Int),
        ("planes", &ValueType::Int),
        ("limit", &ValueType::Float),
        ("thscd1", &ValueType::Int),
        ("thscd2", &ValueType::Float),
        ("weights", &ValueType::Int),
        ("prefix", &ValueType::Data),
    ];
}

impl Degrain {
    #[inline]
    pub fn invoke<'core>(
        self,
        core: CoreRef<'core>,
        clip: &Node<'core>,
        super_node: &Node<'core>,
        vectors: &[Node<'core>],
    ) -> Result<Node<'core>, VapourSynthError> {
        let mut arguments = Self::arguments()?;
        arguments
            .set_node("clip", clip)
            .map_err(|e| VapourSynthError::PluginArgumentsError {
                plugin:   Self::PLUGIN_NAME.to_owned(),
                argument: "clip".to_owned(),
                message:  e.to_string(),
            })?;
        arguments.set_node("super", super_node).map_err(|e| {
            VapourSynthError::PluginArgumentsError {
                plugin:   Self::PLUGIN_NAME.to_owned(),
                argument: "super".to_owned(),
                message:  e.to_string(),
            }
        })?;
        for vector_node in vectors.iter() {
            arguments.append_node("vectors", vector_node).map_err(|e| {
                VapourSynthError::PluginArgumentsError {
                    plugin:   Self::PLUGIN_NAME.to_owned(),
                    argument: "vectors".to_owned(),
                    message:  e.to_string(),
                }
            })?;
        }
        Self::argument_set_int_arrays(&mut arguments, vec![
            ("thsad", self.thsad),
            ("thsad2", self.thsad2),
            ("planes", self.planes),
            ("weights", self.weights),
        ])?;
        Self::arguments_set_float_arrays(&mut arguments, vec![
            ("limit", self.limit),
            // thscd2 is a float percentage (0-100)
            ("thscd2", self.thscd2.map(|f| vec![f])),
        ])?;
        Self::argument_set_ints(&mut arguments, vec![("thscd1", self.thscd1)])?;
        Self::arguments_set(&mut arguments, vec![("prefix", self.prefix)])?;
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
            let super_name = self.super_name.as_ref().unwrap_or(&node_name);
            let vectors_name = self.vectors_name.as_ref().unwrap_or(&node_name);
            write!(
                &mut line,
                "core.mvu.Degrain(clip = {}, super = {}, vectors = {}",
                clip_name, super_name, vectors_name
            )?;
            if let Some(thsad) = &self.thsad {
                write!(&mut line, ", thsad = [{}]", thsad.iter().join(", "))?;
            }
            if let Some(thsad2) = &self.thsad2 {
                write!(&mut line, ", thsad2 = [{}]", thsad2.iter().join(", "))?;
            }
            if let Some(planes) = &self.planes {
                write!(&mut line, ", planes = [{}]", planes.iter().join(", "))?;
            }
            if let Some(limit) = &self.limit {
                write!(&mut line, ", limit = [{}]", limit.iter().join(", "))?;
            }
            if let Some(thscd1) = self.thscd1 {
                write!(&mut line, ", thscd1 = {}", thscd1 as i64)?;
            }
            if let Some(thscd2) = self.thscd2 {
                write!(&mut line, ", thscd2 = {}", thscd2)?;
            }
            if let Some(weights) = &self.weights {
                write!(&mut line, ", weights = [{}]", weights.iter().join(", "))?;
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
