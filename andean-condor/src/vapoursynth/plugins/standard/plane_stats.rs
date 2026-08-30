use std::fmt::Write;

use anyhow::Result;
use vapoursynth::{core::CoreRef, map::ValueType, node::Node};

use crate::vapoursynth::{
    VapourSynthError,
    plugins::{
        MetricPluginFunction,
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

pub struct PlaneStats {
    /// Only used for script generation
    pub clip_b_name: Option<NodeVariableName>,
    pub plane:       Option<u8>,
    /// Defaults to `PlaneStats`
    pub prop:        Option<String>,
}

impl Plugin for PlaneStats {
    const PLUGIN_NAME: &'static str = NAME;
    const PLUGIN_ID: &'static str = ID;
    const PLUGIN_DOCS: Option<&'static str> = Some(DOCS);
}

impl PluginFunction for PlaneStats {
    const FUNCTION_NAME: &'static str = "PlaneStats";
    const FUNCTION_DOCS: Option<&'static str> =
        Some("https://www.vapoursynth.com/doc/functions/video/planestats.html");
    const REQUIRED_ARGUMENTS: &'static [(&'static str, &'static ValueType)] =
        &[("clipa", &ValueType::VideoNode)];
    const OPTIONAL_ARGUMENTS: &'static [(&'static str, &'static ValueType)] = &[
        ("clipb", &ValueType::VideoNode),
        ("plane", &ValueType::Int),
        ("prop", &ValueType::Data),
    ];
}

impl PlaneStats {
    /// This function calculates the min, max and average normalized value of
    /// all the pixels in the specified plane and stores the values in the
    /// frame properties named propMin, propMax and propAverage.
    ///
    /// Returns a new `Node` with the following properties appended to each
    /// `Frame`:
    /// * PlaneStatsAverage: ValueType::Float
    /// * PlaneStatsMax: ValueType::Int
    /// * PlaneStatsMin: ValueType::Int
    ///
    /// If `prop` is provided, the properties will be named `Average`, `Min` and
    /// `Max` and prefixed with the value of `prop`.
    #[inline]
    pub fn call<'core>(
        self,
        core: CoreRef<'core>,
        node: &Node<'core>,
        clip_b: Option<&Node<'core>>,
    ) -> Result<Node<'core>, VapourSynthError> {
        let mut arguments = Self::arguments()?;
        arguments
            .set_node("clipa", node)
            .map_err(|e| VapourSynthError::PluginArgumentsError {
                plugin:   Self::PLUGIN_NAME.to_owned(),
                argument: "clipa".to_owned(),
                message:  e.to_string(),
            })?;
        if let Some(clip_b) = clip_b {
            arguments.set_node("clipb", clip_b).map_err(|e| {
                VapourSynthError::PluginArgumentsError {
                    plugin:   Self::PLUGIN_NAME.to_owned(),
                    argument: "clipb".to_owned(),
                    message:  e.to_string(),
                }
            })?;
        }
        Self::argument_set_ints(&mut arguments, vec![("plane", self.plane)])?;
        Self::arguments_set(&mut arguments, vec![(
            "prop",
            self.prop.map(|s| s.into_bytes()).as_deref(),
        )])?;
        let node = Self::invoke_and_get_node(core, arguments, Some("clip"))?;

        Ok(node)
    }
}

impl MetricPluginFunction for PlaneStats {
    const PROPERTY_NAMES: &'static [&'static str] = &["PlaneStatsDiff", "PlaneStatsAverage"];
}

impl VapourSynthPluginScript for PlaneStats {
    #[inline]
    fn generate_script(&self, node_name: NodeVariableName) -> Result<(Option<Imports>, Vec<Line>)> {
        let mut lines = vec![];

        let line = {
            let mut line = String::new();
            write!(&mut line, "core.std.PlaneStats(clip = {}", node_name)?;
            if let Some(clip_b_name) = &self.clip_b_name {
                write!(&mut line, ", clipb = {}", clip_b_name)?;
            }
            if let Some(plane) = self.plane {
                write!(&mut line, ", plane = {}", plane)?;
            }
            if let Some(prop) = &self.prop {
                write!(&mut line, ", prop = {}", prop)?;
            }
            write!(&mut line, ")")?;
            line
        };

        lines.push(Line::Expression(node_name, line));

        Ok((None, lines))
    }
}
