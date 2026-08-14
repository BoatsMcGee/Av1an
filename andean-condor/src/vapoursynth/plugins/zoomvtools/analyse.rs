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

/// ZooMVTools Analyse plugin - estimates motion vectors for one temporal
/// direction/distance.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Analyse {
    /// Only used for script generation
    pub super_clip_name: Option<NodeVariableName>,
    /// Block size (horizontal). Default 8.
    pub blksize:         Option<u32>,
    /// Block size (vertical). Defaults to blksize.
    pub blksizev:        Option<u32>,
    /// Number of hierarchical levels to use. 0 = all. Default 0.
    pub levels:          Option<u32>,
    /// Search algorithm: 0=log/diamond, 1=exhaustive, 2=hex, 3=UMH, 4=horiz,
    /// 5=vert. Default 0.
    pub search:          Option<u32>,
    /// Search radius/step for chosen search. Default 2.
    pub searchparam:     Option<u32>,
    /// Refinement search radius at finest level.
    pub pelsearch:       Option<u32>,
    /// If true, estimate backward vectors (isb=1). Default False.
    pub isb:             Option<u32>,
    /// Weight of motion-coherence penalty. Default 250.
    pub lambda:          Option<u32>,
    /// Include chroma planes in SAD/SATD metric. Default True.
    pub chroma:          Option<bool>,
    /// Temporal distance and direction (signed). Positive=backward,
    /// negative=forward. Default 1.
    pub delta:           Option<i32>,
    /// Use pre-defined settings for typical TV footage. Default True.
    pub truemotion:      Option<bool>,
    /// SAD "knee" that throttles mvlambda per block. Default 400.
    pub lsad:            Option<u32>,
    /// How mvlambda scales across pyramid levels: 0=const, 1=linear,
    /// 2=quadratic. Default 1.
    pub plevel:          Option<u32>,
    /// Estimate global (pan) motion vector. Default True.
    pub global:          Option<bool>,
    /// Extra penalty for accepting a freshly searched vector. Default 25.
    pub pnew:            Option<u32>,
    /// Extra penalty for accepting the zero vector. Defaults to pnew.
    pub pzero:           Option<u32>,
    /// Penalty for the global-motion predictor. Default 0.
    pub pglobal:         Option<u32>,
    /// Block overlap in pixels. Default 4.
    pub overlap:         Option<u32>,
    /// Block overlap (vertical). Defaults to overlap.
    pub overlapv:        Option<u32>,
    /// Divide extra searches among levels. Default 0.
    pub divide:          Option<u32>,
    /// SAD above which a block gets a wider search. Default 10000.
    pub badsad:          Option<u32>,
    /// Radius of the wider search. Default 24.
    pub badrange:        Option<u32>,
    /// CPU optimizations. Default 4.
    pub opt:             Option<u32>,
    /// Scan rows alternately for better predictor reuse. Default True.
    pub meander:         Option<bool>,
    /// Try multiple motion-vector candidates per block. Default False.
    pub trymany:         Option<bool>,
    /// Treat the clip as field-based. Default False.
    pub fields:          Option<bool>,
    /// Top field first. Default False.
    pub tff:             Option<bool>,
    /// Search algorithm for the coarse levels. Default 0.
    pub search_coarse:   Option<u32>,
    /// DCT transform to use: 0=no transform (SAD), 1=hadamard, 2=rows,
    /// 3=columns, 4=full. Default 0.
    pub dct:             Option<u32>,
}

impl Plugin for Analyse {
    const PLUGIN_NAME: &'static str = NAME;
    const PLUGIN_ID: &'static str = ID;
    const PLUGIN_DOCS: Option<&'static str> = Some(DOCS);
}

impl PluginFunction for Analyse {
    const FUNCTION_NAME: &'static str = "Analyse";
    const FUNCTION_DOCS: Option<&'static str> =
        Some("https://gitlab.com/shssoichiro/vapoursynth-zoomvtools/-/blob/main/USAGE.md?ref_type=heads#analyse");
    const REQUIRED_ARGUMENTS: &'static [(&'static str, &'static ValueType)] =
        &[("super_clip", &ValueType::VideoNode)];
    const OPTIONAL_ARGUMENTS: &'static [(&'static str, &'static ValueType)] = &[
        ("blksize", &ValueType::Int),
        ("blksizev", &ValueType::Int),
        ("levels", &ValueType::Int),
        ("search", &ValueType::Int),
        ("searchparam", &ValueType::Int),
        ("pelsearch", &ValueType::Int),
        ("isb", &ValueType::Int),
        ("lambda", &ValueType::Int),
        ("chroma", &ValueType::Int),
        ("delta", &ValueType::Int),
        ("truemotion", &ValueType::Int),
        ("lsad", &ValueType::Int),
        ("plevel", &ValueType::Int),
        ("global", &ValueType::Int),
        ("pnew", &ValueType::Int),
        ("pzero", &ValueType::Int),
        ("pglobal", &ValueType::Int),
        ("overlap", &ValueType::Int),
        ("overlapv", &ValueType::Int),
        ("divide", &ValueType::Int),
        ("badsad", &ValueType::Int),
        ("badrange", &ValueType::Int),
        ("opt", &ValueType::Int),
        ("meander", &ValueType::Int),
        ("trymany", &ValueType::Int),
        ("fields", &ValueType::Int),
        ("tff", &ValueType::Int),
        ("search_coarse", &ValueType::Int),
        ("dct", &ValueType::Int),
    ];
}

impl Analyse {
    #[inline]
    pub fn invoke<'core>(
        self,
        core: CoreRef<'core>,
        super_clip: &Node<'core>,
    ) -> Result<Node<'core>, VapourSynthError> {
        let mut arguments = Self::arguments()?;
        arguments.set_node("super_clip", super_clip).map_err(|e| {
            VapourSynthError::PluginArgumentsError {
                plugin:   Self::PLUGIN_NAME.to_owned(),
                argument: "super_clip".to_owned(),
                message:  e.to_string(),
            }
        })?;
        Self::argument_set_ints(&mut arguments, vec![
            ("blksize", self.blksize),
            ("blksizev", self.blksizev),
            ("levels", self.levels),
            ("search", self.search),
            ("searchparam", self.searchparam),
            ("pelsearch", self.pelsearch),
            ("isb", self.isb),
            ("lambda", self.lambda),
            ("chroma", self.chroma.map(|b| if b { 1 } else { 0 })),
            ("truemotion", self.truemotion.map(|b| if b { 1 } else { 0 })),
            ("lsad", self.lsad),
            ("plevel", self.plevel),
            ("global", self.global.map(|b| if b { 1 } else { 0 })),
            ("pnew", self.pnew),
            ("pzero", self.pzero),
            ("pglobal", self.pglobal),
            ("overlap", self.overlap),
            ("overlapv", self.overlapv),
            ("divide", self.divide),
            ("badsad", self.badsad),
            ("badrange", self.badrange),
            ("opt", self.opt),
            ("meander", self.meander.map(|b| if b { 1 } else { 0 })),
            ("trymany", self.trymany.map(|b| if b { 1 } else { 0 })),
            ("fields", self.fields.map(|b| if b { 1 } else { 0 })),
            ("tff", self.tff.map(|b| if b { 1 } else { 0 })),
            ("search_coarse", self.search_coarse),
            ("dct", self.dct),
        ])?;
        // delta is signed (negative = forward, positive = backward)
        Self::argument_set_int(&mut arguments, "delta", self.delta.map(|d| d as i64))?;
        let node = Self::invoke_and_get_node(core, arguments, Some("clip"))?;
        Ok(node)
    }
}

impl VapourSynthPluginScript for Analyse {
    #[inline]
    fn generate_script(&self, node_name: NodeVariableName) -> Result<(Option<Imports>, Vec<Line>)> {
        let mut lines = vec![];

        let line = {
            let mut line = String::new();
            match &self.super_clip_name {
                Some(super_clip_name) => {
                    write!(
                        &mut line,
                        "core.zoomv.Analyse(super_clip = {}",
                        super_clip_name
                    )?;
                },
                None => {
                    write!(&mut line, "core.zoomv.Analyse(super_clip = {}", node_name)?;
                },
            }
            if let Some(blksize) = self.blksize {
                write!(&mut line, ", blksize = {}", blksize as i64)?;
            }
            if let Some(blksizev) = self.blksizev {
                write!(&mut line, ", blksizev = {}", blksizev as i64)?;
            }
            if let Some(levels) = self.levels {
                write!(&mut line, ", levels = {}", levels as i64)?;
            }
            if let Some(search) = self.search {
                write!(&mut line, ", search = {}", search as i64)?;
            }
            if let Some(searchparam) = self.searchparam {
                write!(&mut line, ", searchparam = {}", searchparam as i64)?;
            }
            if let Some(pelsearch) = self.pelsearch {
                write!(&mut line, ", pelsearch = {}", pelsearch as i64)?;
            }
            if let Some(isb) = self.isb {
                write!(&mut line, ", isb = {}", isb as i64)?;
            }
            if let Some(lambda) = self.lambda {
                write!(&mut line, ", lambda = {}", lambda as i64)?;
            }
            if let Some(chroma) = self.chroma {
                write!(&mut line, ", chroma = {}", chroma as i64)?;
            }
            if let Some(delta) = self.delta {
                write!(&mut line, ", delta = {}", delta)?;
            }
            if let Some(truemotion) = self.truemotion {
                write!(&mut line, ", truemotion = {}", truemotion as i64)?;
            }
            if let Some(lsad) = self.lsad {
                write!(&mut line, ", lsad = {}", lsad as i64)?;
            }
            if let Some(plevel) = self.plevel {
                write!(&mut line, ", plevel = {}", plevel as i64)?;
            }
            if let Some(global) = self.global {
                write!(&mut line, ", global = {}", global as i64)?;
            }
            if let Some(pnew) = self.pnew {
                write!(&mut line, ", pnew = {}", pnew as i64)?;
            }
            if let Some(pzero) = self.pzero {
                write!(&mut line, ", pzero = {}", pzero as i64)?;
            }
            if let Some(pglobal) = self.pglobal {
                write!(&mut line, ", pglobal = {}", pglobal as i64)?;
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
            if let Some(badsad) = self.badsad {
                write!(&mut line, ", badsad = {}", badsad as i64)?;
            }
            if let Some(badrange) = self.badrange {
                write!(&mut line, ", badrange = {}", badrange as i64)?;
            }
            if let Some(opt) = self.opt {
                write!(&mut line, ", opt = {}", opt as i64)?;
            }
            if let Some(meander) = self.meander {
                write!(&mut line, ", meander = {}", meander as i64)?;
            }
            if let Some(trymany) = self.trymany {
                write!(&mut line, ", trymany = {}", trymany as i64)?;
            }
            if let Some(fields) = self.fields {
                write!(&mut line, ", fields = {}", fields as i64)?;
            }
            if let Some(tff) = self.tff {
                write!(&mut line, ", tff = {}", tff as i64)?;
            }
            if let Some(search_coarse) = self.search_coarse {
                write!(&mut line, ", search_coarse = {}", search_coarse as i64)?;
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
