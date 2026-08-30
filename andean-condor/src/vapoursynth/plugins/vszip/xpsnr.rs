use std::fmt::Write;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use vapoursynth::{core::CoreRef, map::ValueType, node::Node};

use crate::vapoursynth::{
    VapourSynthError,
    plugins::{
        MetricPluginFunction,
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
pub struct XPSNR {
    /// Only used for script generation
    pub reference_node_name: String,
    /// Only used for script generation
    pub distorted_node_name: String,
    pub temporal:            Option<bool>,
    pub verbose:             Option<bool>,
}

impl Plugin for XPSNR {
    const PLUGIN_NAME: &'static str = NAME;
    const PLUGIN_ID: &'static str = ID;
    const PLUGIN_DOCS: Option<&'static str> = Some(DOCS);
}

impl PluginFunction for XPSNR {
    const FUNCTION_NAME: &'static str = "XPSNR";
    const FUNCTION_DOCS: Option<&'static str> =
        Some("https://github.com/dnjulek/vapoursynth-zip/wiki/XPSNR");
    const REQUIRED_ARGUMENTS: &'static [(&'static str, &'static ValueType)] =
        &[("reference", &ValueType::VideoNode), ("distorted", &ValueType::VideoNode)];
    const OPTIONAL_ARGUMENTS: &'static [(&'static str, &'static ValueType)] =
        &[("temporal", &ValueType::Int), ("verbose", &ValueType::Int)];
}

impl XPSNR {
    #[inline]
    pub fn invoke<'core>(
        self,
        core: CoreRef<'core>,
        reference: &Node<'core>,
        distorted: &Node<'core>,
    ) -> Result<Node<'core>, VapourSynthError> {
        let mut arguments = Self::arguments()?;
        arguments.set_node("reference", reference).map_err(|e| {
            VapourSynthError::PluginArgumentsError {
                plugin:   Self::PLUGIN_NAME.to_owned(),
                argument: "reference".to_owned(),
                message:  e.to_string(),
            }
        })?;
        arguments.set_node("distorted", distorted).map_err(|e| {
            VapourSynthError::PluginArgumentsError {
                plugin:   Self::PLUGIN_NAME.to_owned(),
                argument: "distorted".to_owned(),
                message:  e.to_string(),
            }
        })?;
        Self::argument_set_ints(&mut arguments, vec![
            ("temporal", self.temporal.map(|b| if b { 1 } else { 0 })),
            ("verbose", self.verbose.map(|b| if b { 1 } else { 0 })),
        ])?;
        let node = Self::invoke_and_get_node(core, arguments, Some("clip"))?;

        Ok(node)
    }

    /// Combine the per-plane XPSNR scores into a single weighted score, giving
    /// luma four times the weight of each chroma plane.
    #[inline]
    pub fn weight_xpsnr(y: f64, u: f64, v: f64) -> f64 {
        -10.0
            * f64::log10(
                4.0f64.mul_add(
                    f64::powf(10.0, -y / 10.0),
                    f64::powf(10.0, -u / 10.0) + f64::powf(10.0, -v / 10.0),
                ) / 6.0,
            )
    }
}

impl MetricPluginFunction for XPSNR {
    /// The plugin writes one score per plane and no combined score, so all
    /// three properties are required. Use
    /// [`MetricPluginFunction::get_multiple_scores`] to read them and
    /// [`XPSNR::weight_xpsnr`] to reduce them to a single score.
    const PROPERTY_NAMES: &'static [&'static str] = &["XPSNR_Y", "XPSNR_U", "XPSNR_V"];
}

impl VapourSynthPluginScript for XPSNR {
    #[inline]
    fn generate_script(&self, node_name: NodeVariableName) -> Result<(Option<Imports>, Vec<Line>)> {
        let mut lines = vec![];

        let line = {
            let mut line = String::new();
            write!(
                &mut line,
                "core.vszip.XPSNR(reference = {}, distorted = {}",
                self.reference_node_name, self.distorted_node_name
            )?;
            if let Some(temporal) = self.temporal {
                write!(&mut line, ", temporal = {}", temporal as i64)?;
            }
            if let Some(verbose) = self.verbose {
                write!(&mut line, ", verbose = {}", verbose as i64)?;
            }
            write!(&mut line, ")")?;
            line
        };

        lines.push(Line::Expression(node_name, line));

        Ok((None, lines))
    }
}
