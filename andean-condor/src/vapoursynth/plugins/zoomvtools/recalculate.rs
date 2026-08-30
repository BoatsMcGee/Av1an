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

/// ZooMVTools Recalculate plugin - re-estimates an existing vector field
/// at a finer block size.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Recalculate {
    /// Only used for script generation
    pub super_clip_name: Option<NodeVariableName>,
    /// Only used for script generation
    pub vectors_name:    Option<NodeVariableName>,
    /// Blocks whose SAD is below this keep their vector. Default 100.
    pub thsad:           Option<u32>,
    /// Interpolate new vector field from neighbours. Default True.
    pub smooth:          Option<bool>,
    /// Finer block size (horizontal). Default 8.
    pub blksize:         Option<u32>,
    /// Finer block size (vertical). Defaults to blksize.
    pub blksizev:        Option<u32>,
    /// Search algorithm: 0=log/diamond, 1=exhaustive, 2=hex, 3=UMH, 4=horiz,
    /// 5=vert. Default 0.
    pub search:          Option<u32>,
    /// Search radius/step for chosen search. Default 2.
    pub searchparam:     Option<u32>,
    /// Weight of motion-coherence penalty. Default 250.
    pub lambda:          Option<u32>,
    /// Include chroma planes in SAD/SATD metric. Default True.
    pub chroma:          Option<bool>,
    /// Use pre-defined settings for typical TV footage. Default True.
    pub truemotion:      Option<bool>,
    /// Extra penalty for accepting a freshly searched vector. Default 25.
    pub pnew:            Option<u32>,
    /// Block overlap in pixels. Default 4.
    pub overlap:         Option<u32>,
    /// Block overlap (vertical). Defaults to overlap.
    pub overlapv:        Option<u32>,
    /// Divide extra searches among levels. Default 0.
    pub divide:          Option<u32>,
    /// CPU optimizations. Default 4.
    pub opt:             Option<u32>,
    /// Scan rows alternately for better predictor reuse. Default True.
    pub meander:         Option<bool>,
    /// Treat the clip as field-based. Default False.
    pub fields:          Option<bool>,
    /// Top field first. Default False.
    pub tff:             Option<bool>,
    /// DCT transform to use: 0=no transform (SAD), 1=hadamard, 2=rows,
    /// 3=columns, 4=full. Default 0.
    pub dct:             Option<u32>,
}

impl Plugin for Recalculate {
    const PLUGIN_NAME: &'static str = NAME;
    const PLUGIN_ID: &'static str = ID;
    const PLUGIN_DOCS: Option<&'static str> = Some(DOCS);
}

impl PluginFunction for Recalculate {
    const FUNCTION_NAME: &'static str = "Recalculate";
    const FUNCTION_DOCS: Option<&'static str> = Some("https://gitlab.com/shssoichiro/vapoursynth-zoomvtools/-/blob/main/USAGE.md?ref_type=heads#recalculate");
    const REQUIRED_ARGUMENTS: &'static [(&'static str, &'static ValueType)] =
        &[("super_clip", &ValueType::VideoNode), ("vectors", &ValueType::VideoNode)];
    const OPTIONAL_ARGUMENTS: &'static [(&'static str, &'static ValueType)] = &[
        ("thsad", &ValueType::Int),
        ("smooth", &ValueType::Int),
        ("blksize", &ValueType::Int),
        ("blksizev", &ValueType::Int),
        ("search", &ValueType::Int),
        ("searchparam", &ValueType::Int),
        ("lambda", &ValueType::Int),
        ("chroma", &ValueType::Int),
        ("truemotion", &ValueType::Int),
        ("pnew", &ValueType::Int),
        ("overlap", &ValueType::Int),
        ("overlapv", &ValueType::Int),
        ("divide", &ValueType::Int),
        ("opt", &ValueType::Int),
        ("meander", &ValueType::Int),
        ("fields", &ValueType::Int),
        ("tff", &ValueType::Int),
        ("dct", &ValueType::Int),
    ];
}

impl Recalculate {
    #[inline]
    pub fn invoke<'core>(
        self,
        core: CoreRef<'core>,
        super_clip: &Node<'core>,
        vectors: &Node<'core>,
    ) -> Result<Node<'core>, VapourSynthError> {
        let mut arguments = Self::arguments()?;
        arguments.set_node("super_clip", super_clip).map_err(|e| {
            VapourSynthError::PluginArgumentsError {
                plugin:   Self::PLUGIN_NAME.to_owned(),
                argument: "super_clip".to_owned(),
                message:  e.to_string(),
            }
        })?;
        arguments.set_node("vectors", vectors).map_err(|e| {
            VapourSynthError::PluginArgumentsError {
                plugin:   Self::PLUGIN_NAME.to_owned(),
                argument: "vectors".to_owned(),
                message:  e.to_string(),
            }
        })?;
        Self::argument_set_ints(&mut arguments, vec![
            ("thsad", self.thsad),
            ("smooth", self.smooth.map(|b| if b { 1 } else { 0 })),
            ("blksize", self.blksize),
            ("blksizev", self.blksizev),
            ("search", self.search),
            ("searchparam", self.searchparam),
            ("lambda", self.lambda),
            ("chroma", self.chroma.map(|b| if b { 1 } else { 0 })),
            ("truemotion", self.truemotion.map(|b| if b { 1 } else { 0 })),
            ("pnew", self.pnew),
            ("overlap", self.overlap),
            ("overlapv", self.overlapv),
            ("divide", self.divide),
            ("opt", self.opt),
            ("meander", self.meander.map(|b| if b { 1 } else { 0 })),
            ("fields", self.fields.map(|b| if b { 1 } else { 0 })),
            ("tff", self.tff.map(|b| if b { 1 } else { 0 })),
            ("dct", self.dct),
        ])?;
        let node = Self::invoke_and_get_node(core, arguments, Some("clip"))?;
        Ok(node)
    }
}

impl VapourSynthPluginScript for Recalculate {
    #[inline]
    fn generate_script(&self, node_name: NodeVariableName) -> Result<(Option<Imports>, Vec<Line>)> {
        let mut lines = vec![];

        let line = {
            let mut line = String::new();
            match (&self.super_clip_name, &self.vectors_name) {
                (Some(super_clip_name), Some(vectors_name)) => {
                    write!(
                        &mut line,
                        "core.zoomv.Recalculate(super_clip = {}, vectors = {}",
                        super_clip_name, vectors_name
                    )?;
                },
                (Some(super_clip_name), None) => {
                    write!(
                        &mut line,
                        "core.zoomv.Recalculate(super_clip = {}, vectors = {}",
                        super_clip_name, node_name
                    )?;
                },
                (None, Some(vectors_name)) => {
                    write!(
                        &mut line,
                        "core.zoomv.Recalculate(super_clip = {}, vectors = {}",
                        node_name, vectors_name
                    )?;
                },
                (None, None) => {
                    write!(
                        &mut line,
                        "core.zoomv.Recalculate(super_clip = {}, vectors = {}",
                        node_name, node_name
                    )?;
                },
            }
            if let Some(thsad) = self.thsad {
                write!(&mut line, ", thsad = {}", thsad as i64)?;
            }
            if let Some(smooth) = self.smooth {
                write!(&mut line, ", smooth = {}", smooth as i64)?;
            }
            if let Some(blksize) = self.blksize {
                write!(&mut line, ", blksize = {}", blksize as i64)?;
            }
            if let Some(blksizev) = self.blksizev {
                write!(&mut line, ", blksizev = {}", blksizev as i64)?;
            }
            if let Some(search) = self.search {
                write!(&mut line, ", search = {}", search as i64)?;
            }
            if let Some(searchparam) = self.searchparam {
                write!(&mut line, ", searchparam = {}", searchparam as i64)?;
            }
            if let Some(lambda) = self.lambda {
                write!(&mut line, ", lambda = {}", lambda as i64)?;
            }
            if let Some(chroma) = self.chroma {
                write!(&mut line, ", chroma = {}", chroma as i64)?;
            }
            if let Some(truemotion) = self.truemotion {
                write!(&mut line, ", truemotion = {}", truemotion as i64)?;
            }
            if let Some(pnew) = self.pnew {
                write!(&mut line, ", pnew = {}", pnew as i64)?;
            }
            if let Some(overlap) = self.overlap {
                write!(&mut line, ", overlap = {}", overlap as i64)?;
            }
            if let Some(overlapv) = self.overlapv {
                write!(&mut line, ", overlapv = {}", overlapv as i64)?;
            }
            if let Some(divide) = self.divide {
                write!(&mut line, ", divide = {}", divide as i64)?;
            }
            if let Some(opt) = self.opt {
                write!(&mut line, ", opt = {}", opt as i64)?;
            }
            if let Some(meander) = self.meander {
                write!(&mut line, ", meander = {}", meander as i64)?;
            }
            if let Some(fields) = self.fields {
                write!(&mut line, ", fields = {}", fields as i64)?;
            }
            if let Some(tff) = self.tff {
                write!(&mut line, ", tff = {}", tff as i64)?;
            }
            if let Some(dct) = self.dct {
                write!(&mut line, ", dct = {}", dct as i64)?;
            }
            write!(&mut line, ")")?;
            line
        };

        lines.push(Line::Expression(node_name, line));

        Ok((None, lines))
    }
}
