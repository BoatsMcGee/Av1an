use std::fmt::Write;

use anyhow::Result;
use itertools::Itertools;
use serde::{Deserialize, Serialize};
use vapoursynth::{core::CoreRef, map::ValueType, node::Node};

use crate::vapoursynth::{
    plugins::PluginFunction,
    script_builder::{
        script::{Imports, Line},
        NodeVariableName,
        VapourSynthPluginScript,
    },
    VapourSynthError,
};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WNNM {
    /// Denoising strength per plane. A plane with sigma < FLT_EPSILON is copied
    /// through. Missing entries inherit the previous one.
    ///
    /// Defaults to `[3.0, 3.0, 3.0]`
    pub sigma:                Option<Vec<f64>>,
    pub block_size:           Option<u32>,
    pub block_step:           Option<u32>,
    pub group_size:           Option<u32>,
    pub bm_range:             Option<u32>,
    pub radius:               Option<u32>,
    pub ps_num:               Option<u32>,
    pub ps_range:             Option<u32>,
    pub residual:             Option<bool>,
    pub adaptive_aggregation: Option<bool>,
    /// Only used for script generation
    pub rclip_name:           Option<NodeVariableName>,
}

impl PluginFunction for WNNM {
    const PLUGIN_NAME: &'static str = "VapourSynth Zig Image Process";
    const PLUGIN_ID: &'static str = "com.julek.vszip";
    const FUNCTION_NAME: &'static str = "WNNM";
    const REQUIRED_ARGUMENTS: &'static [(&'static str, &'static ValueType)] =
        &[("clip", &ValueType::VideoNode), ("planes", &ValueType::Int)];
    const OPTIONAL_ARGUMENTS: &'static [(&'static str, &'static ValueType)] = &[
        ("sigma", &ValueType::Float),
        ("block_size", &ValueType::Int),
        ("block_step", &ValueType::Int),
        ("group_size", &ValueType::Int),
        ("bm_range", &ValueType::Int),
        ("radius", &ValueType::Int),
        ("ps_num", &ValueType::Int),
        ("ps_range", &ValueType::Int),
        ("residual", &ValueType::Int),
        ("adaptive_aggregation", &ValueType::Int),
        ("rclip", &ValueType::VideoNode),
    ];
}

impl WNNM {
    #[inline]
    pub fn invoke<'core>(
        self,
        core: CoreRef<'core>,
        node: &Node<'core>,
        rclip: Option<&Node<'core>>,
    ) -> Result<Node<'core>, VapourSynthError> {
        let mut arguments = Self::arguments()?;
        arguments
            .set_node("clip", node)
            .map_err(|e| VapourSynthError::PluginArgumentsError {
                plugin:   Self::PLUGIN_NAME.to_owned(),
                argument: "clip".to_owned(),
                message:  e.to_string(),
            })?;
        if let Some(rclip) = rclip {
            arguments.set_node("rclip", rclip).map_err(|e| {
                VapourSynthError::PluginArgumentsError {
                    plugin:   Self::PLUGIN_NAME.to_owned(),
                    argument: "ref".to_owned(),
                    message:  e.to_string(),
                }
            })?;
        }
        Self::arguments_set_float_arrays(&mut arguments, vec![("sigma", self.sigma)])?;
        Self::argument_set_ints(&mut arguments, vec![
            ("block_size", self.block_size),
            ("block_step", self.block_step),
            ("group_size", self.group_size),
            ("bm_range", self.bm_range),
            ("radius", self.radius),
            ("ps_num", self.ps_num),
            ("ps_range", self.ps_range),
            ("residual", self.residual.map(|b| if b { 1 } else { 0 })),
            (
                "adaptive_aggregation",
                self.adaptive_aggregation.map(|b| if b { 1 } else { 0 }),
            ),
        ])?;
        let node = Self::invoke_and_get_node(core, arguments, Some("clip"))?;

        Ok(node)
    }
}

impl VapourSynthPluginScript for WNNM {
    #[inline]
    fn generate_script(&self, node_name: NodeVariableName) -> Result<(Option<Imports>, Vec<Line>)> {
        let mut lines = vec![];

        let line = {
            let mut line = String::new();
            write!(&mut line, "core.vszip.WNNM(clip = {}", node_name)?;
            if let Some(sigma) = &self.sigma {
                write!(&mut line, ", sigma = [{}]", sigma.iter().join(", "))?;
            }
            if let Some(block_size) = self.block_size {
                write!(&mut line, ", block_size = {}", block_size as i64)?;
            }
            if let Some(block_step) = self.block_step {
                write!(&mut line, ", block_step = {}", block_step as i64)?;
            }
            if let Some(group_size) = self.group_size {
                write!(&mut line, ", group_size = {}", group_size as i64)?;
            }
            if let Some(bm_range) = self.bm_range {
                write!(&mut line, ", bm_range = {}", bm_range as i64)?;
            }
            if let Some(radius) = self.radius {
                write!(&mut line, ", radius = {}", radius as i64)?;
            }
            if let Some(ps_num) = self.ps_num {
                write!(&mut line, ", ps_num = {}", ps_num as i64)?;
            }
            if let Some(ps_range) = self.ps_range {
                write!(&mut line, ", ps_range = {}", ps_range as i64)?;
            }
            if let Some(residual) = self.residual {
                write!(&mut line, ", residual = {}", residual as i64)?;
            }
            if let Some(adaptive_aggregation) = self.adaptive_aggregation {
                write!(
                    &mut line,
                    ", adaptive_aggregation = {}",
                    adaptive_aggregation as i64
                )?;
            }
            if let Some(rclip_name) = &self.rclip_name {
                write!(&mut line, ", rclip = {}", rclip_name)?;
            }
            write!(&mut line, ")")?;
            line
        };

        lines.push(Line::Expression(node_name, line));

        Ok((None, lines))
    }
}
