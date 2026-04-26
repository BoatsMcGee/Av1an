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
pub struct BM3DCUDA {
    // Only used for script generation
    pub clip_name:  NodeVariableName,
    // Only used for script generation
    pub ref_name:   Option<NodeVariableName>,
    /// The strength of denoising for each plane.
    ///
    /// Defaults to `[3.0, 3.0, 3.0]`
    pub sigma:      Option<[f64; 3]>,
    pub radius:     Option<u32>,
    pub block_step: Option<u32>,
    pub bm_range:   Option<u32>,
    pub ps_range:   Option<u32>,

    /// Multi-threaded copy between CPU and GPU at the expense of 4x memory consumption.
    ///
    /// Defaults to `true`
    pub fast:       Option<bool>,
}

impl PluginFunction for BM3DCUDA {
    const PLUGIN_NAME: &'static str = "VapourSynth-BM3DCUDA";
    const PLUGIN_ID: &'static str = "com.wolframrhodium.bm3dcuda";
    const FUNCTION_NAME: &'static str = "BM3Dv2";
    const REQUIRED_ARGUMENTS: &'static [(&'static str, &'static ValueType)] =
        &[("clip", &ValueType::VideoNode)];
    const OPTIONAL_ARGUMENTS: &'static [(&'static str, &'static ValueType)] = &[
        ("ref", &ValueType::VideoNode),
        ("sigma", &ValueType::Float),
        ("radius", &ValueType::Int),
        ("block_step", &ValueType::Int),
        ("bm_range", &ValueType::Int),
        ("ps_range", &ValueType::Int),
    ];
}

impl BM3DCUDA {
    #[inline]
    pub fn invoke<'core>(
        self,
        core: CoreRef<'core>,
        clip: &Node<'core>,
        reference: Option<&Node<'core>>,
    ) -> Result<Node<'core>, VapourSynthError> {
        let mut arguments = Self::arguments()?;
        arguments
            .set_node("clip", clip)
            .map_err(|e| VapourSynthError::PluginArgumentsError {
                plugin:   Self::PLUGIN_NAME.to_owned(),
                argument: "clip".to_owned(),
                message:  e.to_string(),
            })?;
        if let Some(reference) = reference {
            arguments.set_node("ref", reference).map_err(|e| {
                VapourSynthError::PluginArgumentsError {
                    plugin:   Self::PLUGIN_NAME.to_owned(),
                    argument: "ref".to_owned(),
                    message:  e.to_string(),
                }
            })?;
        }
        Self::argument_set_ints(&mut arguments, vec![
            ("radius", self.radius),
            ("block_step", self.block_step),
            ("bm_range", self.bm_range),
            ("ps_range", self.ps_range),
            ("fast", self.fast.map(|b| if b { 1 } else { 0 })),
        ])?;
        Self::arguments_set_float_arrays(&mut arguments, vec![(
            "sigma",
            self.sigma.map(|s| s.to_vec()),
        )])?;
        let node = Self::invoke_and_get_node(core, arguments, Some("clip"))?;

        Ok(node)
    }
}

impl VapourSynthPluginScript for BM3DCUDA {
    #[inline]
    fn generate_script(&self, node_name: NodeVariableName) -> Result<(Option<Imports>, Vec<Line>)> {
        let mut lines = vec![];

        let line = {
            let mut line = String::new();
            write!(&mut line, "core.bm3dcuda.BM3Dv2(clip = {}", node_name)?;
            if let Some(ref_name) = &self.ref_name {
                write!(&mut line, ", ref = {}", ref_name)?;
            }
            if let Some(sigma) = &self.sigma {
                write!(&mut line, ", sigma = [{}]", sigma.iter().join(", "))?;
            }
            if let Some(radius) = self.radius {
                write!(&mut line, ", radius = {}", radius as i64)?;
            }
            if let Some(block_step) = self.block_step {
                write!(&mut line, ", block_step = {}", block_step as i64)?;
            }
            if let Some(bm_range) = self.bm_range {
                write!(&mut line, ", bm_range = {}", bm_range as i64)?;
            }
            if let Some(ps_range) = self.ps_range {
                write!(&mut line, ", ps_range = {}", ps_range as i64)?;
            }
            if let Some(fast) = self.fast {
                write!(&mut line, ", fast = {}", fast as i64)?;
            }
            write!(&mut line, ")")?;
            line
        };

        lines.push(Line::Expression(node_name, line));

        Ok((None, lines))
    }
}
