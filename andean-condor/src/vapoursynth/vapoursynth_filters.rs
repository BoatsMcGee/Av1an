use std::{
    collections::BTreeMap,
    fmt::{Display, Write},
    str::FromStr,
};

use anyhow::{Result, bail};
use itertools::Itertools;
use serde::{Deserialize, Serialize};
use vapoursynth::{core::CoreRef, node::Node};

use crate::{
    ffmpeg::FFPixelFormat,
    vapoursynth::{
        VapourSynthError,
        plugins::{
            mvutensils::{
                analyse_many::AnalyseMany,
                degrain::Degrain,
                mvu_super::MVUSuper,
                recalculate::Recalculate,
            },
            rescale::{Doubler, RescaleBuilder, VSJETKernel},
            resize::{
                Scaler,
                bicubic::Bicubic,
                bilinear::Bilinear,
                bob::Bob,
                lanczos::Lanczos,
                point::Point,
                spline16::Spline16,
                spline36::Spline36,
                spline64::Spline64,
            },
            standard::{crop::Crop, trim::Trim},
            vszip::{bilateral::Bilateral, wnnm::WNNM},
            zoomvtools::{
                analyse::Analyse as ZoomAnalyse,
                degrain::Degrain as ZoomDegrainPlugin,
                recalculate::Recalculate as ZoomRecalculate,
                zmv_super::ZMVSuper,
            },
        },
        script_builder::{
            NodeVariableName,
            VapourSynthPluginScript,
            script::{Imports, Line},
        },
    },
};

/// The [`Degrain`](VapourSynthFilter::Degrain) variant is a composite filter
/// that intentionally carries many configuration fields, so the enum is larger
/// than the other variants.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum VapourSynthFilter {
    Crop {
        top:    Option<usize>,
        bottom: Option<usize>,
        left:   Option<usize>,
        right:  Option<usize>,
    },
    Resize {
        scaler: Option<Scaler>,
        width:  Option<usize>,
        height: Option<usize>,
        format: Option<FFPixelFormat>,
    },
    Trim {
        start: Option<usize>,
        end:   Option<usize>,
    },
    Rescale {
        kernel:  VSJETKernel,
        width:   usize,
        height:  usize,
        doubler: Doubler,
    },
    WNNM {
        sigma:                Option<Vec<f64>>,
        block_size:           Option<usize>,
        block_step:           Option<usize>,
        group_size:           Option<usize>,
        bm_range:             Option<usize>,
        radius:               Option<usize>,
        ps_num:               Option<usize>,
        ps_range:             Option<usize>,
        residual:             Option<bool>,
        adaptive_aggregation: Option<bool>,
    },
    Bilateral {
        sigma_s:   Option<Vec<f64>>,
        sigma_r:   Option<Vec<f64>>,
        planes:    Option<Vec<bool>>,
        algorithm: Option<Vec<u32>>,
        pbficnum:  Option<Vec<u32>>,
    },
    /// MVUtensils Degrain - motion-compensated temporal denoising workflow.
    /// This is a composite filter that internally chains Super -> AnalyseMany
    /// -> (optional Recalculate) -> Degrain.
    Degrain {
        blksize:        Option<Vec<usize>>,
        overlap:        Option<Vec<usize>>,
        pad:            Option<Vec<usize>>,
        pel:            Option<usize>,
        sharp:          Option<usize>,
        rfilter:        Option<usize>,
        radius:         Option<usize>,
        search:         Option<usize>,
        searchparam:    Option<usize>,
        mvlambda:       Option<usize>,
        chroma:         Option<bool>,
        lsad:           Option<usize>,
        plevel:         Option<usize>,
        globalmv:       Option<bool>,
        pnew:           Option<usize>,
        pzero:          Option<usize>,
        pglobal:        Option<usize>,
        badsad:         Option<usize>,
        badrange:       Option<usize>,
        meander:        Option<bool>,
        trymany:        Option<usize>,
        satd:           Option<bool>,
        recalculate:    Option<bool>,
        recalc_thsad:   Option<usize>,
        recalc_smooth:  Option<bool>,
        recalc_blksize: Option<Vec<usize>>,
        recalc_overlap: Option<Vec<usize>>,
        thsad:          Option<Vec<usize>>,
        thsad2:         Option<Vec<usize>>,
        planes:         Option<Vec<usize>>,
        limit:          Option<Vec<f64>>,
        thscd1:         Option<usize>,
        thscd2:         Option<f64>,
        prefix:         Option<String>,
    },
    /// ZooMVTools Degrain - motion-compensated temporal denoising workflow.
    /// This is a composite filter that internally chains ZMVSuper -> two
    /// Analyse calls (isb=0 forward, isb=1 backward) -> (optional
    /// Recalculate on both) -> Degrain1 (implemented as Degrain).
    ZoomDegrain {
        hpad:               Option<usize>,
        vpad:               Option<usize>,
        pel:                Option<usize>,
        levels:             Option<usize>,
        chroma:             Option<bool>,
        sharp:              Option<usize>,
        rfilter:            Option<usize>,
        blksize:            Option<usize>,
        blksizev:           Option<usize>,
        search:             Option<usize>,
        searchparam:        Option<usize>,
        pelsearch:          Option<usize>,
        lambda:             Option<usize>,
        lsad:               Option<usize>,
        plevel:             Option<usize>,
        global:             Option<bool>,
        pnew:               Option<usize>,
        pzero:              Option<usize>,
        pglobal:            Option<usize>,
        overlap:            Option<usize>,
        overlapv:           Option<usize>,
        divide:             Option<usize>,
        badsad:             Option<usize>,
        badrange:           Option<usize>,
        truemotion:         Option<bool>,
        meander:            Option<bool>,
        trymany:            Option<bool>,
        fields:             Option<bool>,
        tff:                Option<bool>,
        search_coarse:      Option<usize>,
        dct:                Option<usize>,
        recalculate:        Option<bool>,
        recalc_thsad:       Option<usize>,
        recalc_smooth:      Option<bool>,
        recalc_blksize:     Option<usize>,
        recalc_blksizev:    Option<usize>,
        recalc_search:      Option<usize>,
        recalc_searchparam: Option<usize>,
        recalc_lambda:      Option<usize>,
        recalc_overlap:     Option<usize>,
        recalc_overlapv:    Option<usize>,
        recalc_divide:      Option<usize>,
        thsad:              Option<usize>,
        thsadc:             Option<usize>,
        plane:              Option<i32>,
        limit:              Option<usize>,
        limitc:             Option<usize>,
        thscd1:             Option<usize>,
        thscd2:             Option<usize>,
    },
}

impl FromStr for VapourSynthFilter {
    type Err = anyhow::Error;

    #[inline]
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let parts: Vec<&str> = s.split(':').collect();
        let variant_name = parts[0];
        let variant_args = parts[1]
            .split(';')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(|arg| {
                let mut parts = arg.splitn(2, '=');
                let name = parts
                    .next()
                    .expect("Failed to parse filter argument")
                    .to_string()
                    .to_lowercase();
                let value = parts.next().map(|v| v.trim().to_string()).unwrap_or_default();
                (name, value)
            })
            .collect::<BTreeMap<_, _>>();
        match variant_name {
            "crop" => Ok(VapourSynthFilter::Crop {
                top:    variant_args
                    .get("top")
                    .map(|v| v.parse::<usize>().expect("Failed to parse filter argument value")),
                bottom: variant_args
                    .get("bottom")
                    .map(|v| v.parse::<usize>().expect("Failed to parse filter argument value")),
                left:   variant_args
                    .get("left")
                    .map(|v| v.parse::<usize>().expect("Failed to parse filter argument value")),
                right:  variant_args
                    .get("right")
                    .map(|v| v.parse::<usize>().expect("Failed to parse filter argument value")),
            }),
            "resize" => Ok(VapourSynthFilter::Resize {
                scaler: variant_args
                    .get("scaler")
                    .map(|v| Scaler::from_str(v).expect("Failed to parse filter argument value")),
                width:  variant_args
                    .get("width")
                    .map(|v| v.parse::<usize>().expect("Failed to parse filter argument value")),
                height: variant_args
                    .get("height")
                    .map(|v| v.parse::<usize>().expect("Failed to parse filter argument value")),
                format: variant_args.get("format").map(|v| {
                    FFPixelFormat::from_str(v).expect("Failed to parse filter argument value")
                }),
            }),
            "trim" => Ok(VapourSynthFilter::Trim {
                start: variant_args
                    .get("start")
                    .map(|v| v.parse::<usize>().expect("Failed to parse filter argument value")),
                end:   variant_args
                    .get("end")
                    .map(|v| v.parse::<usize>().expect("Failed to parse filter argument value")),
            }),
            "rescale" => Ok(VapourSynthFilter::Rescale {
                kernel:  variant_args
                    .get("kernel")
                    .map(|v| VSJETKernel::from_str(v).expect("Failed to parse kernel"))
                    .expect("Failed to parse kernel"),
                width:   variant_args
                    .get("width")
                    .map(|v| v.parse::<usize>().expect("Failed to parse filter argument value"))
                    .expect("Failed to parse width"),
                height:  variant_args
                    .get("height")
                    .map(|v| v.parse::<usize>().expect("Failed to parse filter argument value"))
                    .expect("Failed to parse height"),
                doubler: variant_args
                    .get("doubler")
                    .map(|v| Doubler::from_str(v).expect("Failed to parse filter argument value"))
                    .expect("Failed to parse doubler"),
            }),
            "wnnm" => Ok(VapourSynthFilter::WNNM {
                sigma:                variant_args.get("sigma").map(|v| {
                    v.split(',')
                        .map(|s| s.parse::<f64>().expect("Failed to parse filter argument value"))
                        .collect()
                }),
                block_size:           variant_args
                    .get("block_size")
                    .map(|v| v.parse::<usize>().expect("Failed to parse filter argument value")),
                block_step:           variant_args
                    .get("block_step")
                    .map(|v| v.parse::<usize>().expect("Failed to parse filter argument value")),
                group_size:           variant_args
                    .get("group_size")
                    .map(|v| v.parse::<usize>().expect("Failed to parse filter argument value")),
                bm_range:             variant_args
                    .get("bm_range")
                    .map(|v| v.parse::<usize>().expect("Failed to parse filter argument value")),
                radius:               variant_args
                    .get("radius")
                    .map(|v| v.parse::<usize>().expect("Failed to parse filter argument value")),
                ps_num:               variant_args
                    .get("ps_num")
                    .map(|v| v.parse::<usize>().expect("Failed to parse filter argument value")),
                ps_range:             variant_args
                    .get("ps_range")
                    .map(|v| v.parse::<usize>().expect("Failed to parse filter argument value")),
                residual:             variant_args
                    .get("residual")
                    .map(|v| matches!(v.as_str(), "true" | "1")),
                adaptive_aggregation: variant_args
                    .get("adaptive_aggregation")
                    .map(|v| matches!(v.as_str(), "true" | "1")),
            }),
            "bilateral" => Ok(VapourSynthFilter::Bilateral {
                sigma_s:   variant_args.get("sigma_s").map(|v| {
                    v.split(',')
                        .map(|s| s.parse::<f64>().expect("Failed to parse filter argument value"))
                        .collect()
                }),
                sigma_r:   variant_args.get("sigma_r").map(|v| {
                    v.split(',')
                        .map(|s| s.parse::<f64>().expect("Failed to parse filter argument value"))
                        .collect()
                }),
                planes:    variant_args
                    .get("planes")
                    .map(|v| v.split(',').map(|s| matches!(s, "1" | "true")).collect()),
                algorithm: variant_args.get("algorithm").map(|v| {
                    v.split(',')
                        .map(|s| s.parse::<u32>().expect("Failed to parse filter argument value"))
                        .collect()
                }),
                pbficnum:  variant_args.get("pbficnum").map(|v| {
                    v.split(',')
                        .map(|s| s.parse::<u32>().expect("Failed to parse filter argument value"))
                        .collect()
                }),
            }),
            "degrain" => Ok(VapourSynthFilter::Degrain {
                blksize:        variant_args.get("blksize").map(|v| {
                    v.split(',')
                        .map(|s| s.parse::<usize>().expect("Failed to parse filter argument value"))
                        .collect()
                }),
                overlap:        variant_args.get("overlap").map(|v| {
                    v.split(',')
                        .map(|s| s.parse::<usize>().expect("Failed to parse filter argument value"))
                        .collect()
                }),
                pad:            variant_args.get("pad").map(|v| {
                    v.split(',')
                        .map(|s| s.parse::<usize>().expect("Failed to parse filter argument value"))
                        .collect()
                }),
                pel:            variant_args
                    .get("pel")
                    .map(|v| v.parse::<usize>().expect("Failed to parse filter argument value")),
                sharp:          variant_args
                    .get("sharp")
                    .map(|v| v.parse::<usize>().expect("Failed to parse filter argument value")),
                rfilter:        variant_args
                    .get("rfilter")
                    .map(|v| v.parse::<usize>().expect("Failed to parse filter argument value")),
                radius:         variant_args
                    .get("radius")
                    .map(|v| v.parse::<usize>().expect("Failed to parse filter argument value")),
                search:         variant_args
                    .get("search")
                    .map(|v| v.parse::<usize>().expect("Failed to parse filter argument value")),
                searchparam:    variant_args
                    .get("searchparam")
                    .map(|v| v.parse::<usize>().expect("Failed to parse filter argument value")),
                mvlambda:       variant_args
                    .get("mvlambda")
                    .map(|v| v.parse::<usize>().expect("Failed to parse filter argument value")),
                chroma:         variant_args
                    .get("chroma")
                    .map(|v| matches!(v.as_str(), "true" | "1")),
                lsad:           variant_args
                    .get("lsad")
                    .map(|v| v.parse::<usize>().expect("Failed to parse filter argument value")),
                plevel:         variant_args
                    .get("plevel")
                    .map(|v| v.parse::<usize>().expect("Failed to parse filter argument value")),
                globalmv:       variant_args
                    .get("globalmv")
                    .map(|v| matches!(v.as_str(), "true" | "1")),
                pnew:           variant_args
                    .get("pnew")
                    .map(|v| v.parse::<usize>().expect("Failed to parse filter argument value")),
                pzero:          variant_args
                    .get("pzero")
                    .map(|v| v.parse::<usize>().expect("Failed to parse filter argument value")),
                pglobal:        variant_args
                    .get("pglobal")
                    .map(|v| v.parse::<usize>().expect("Failed to parse filter argument value")),
                badsad:         variant_args
                    .get("badsad")
                    .map(|v| v.parse::<usize>().expect("Failed to parse filter argument value")),
                badrange:       variant_args
                    .get("badrange")
                    .map(|v| v.parse::<usize>().expect("Failed to parse filter argument value")),
                meander:        variant_args
                    .get("meander")
                    .map(|v| matches!(v.as_str(), "true" | "1")),
                trymany:        variant_args
                    .get("trymany")
                    .map(|v| v.parse::<usize>().expect("Failed to parse filter argument value")),
                satd:           variant_args
                    .get("satd")
                    .map(|v| matches!(v.as_str(), "true" | "1")),
                recalculate:    variant_args
                    .get("recalculate")
                    .map(|v| matches!(v.as_str(), "true" | "1")),
                recalc_thsad:   variant_args
                    .get("recalc_thsad")
                    .map(|v| v.parse::<usize>().expect("Failed to parse filter argument value")),
                recalc_smooth:  variant_args
                    .get("recalc_smooth")
                    .map(|v| matches!(v.as_str(), "true" | "1")),
                recalc_blksize: variant_args.get("recalc_blksize").map(|v| {
                    v.split(',')
                        .map(|s| s.parse::<usize>().expect("Failed to parse filter argument value"))
                        .collect()
                }),
                recalc_overlap: variant_args.get("recalc_overlap").map(|v| {
                    v.split(',')
                        .map(|s| s.parse::<usize>().expect("Failed to parse filter argument value"))
                        .collect()
                }),
                thsad:          variant_args.get("thsad").map(|v| {
                    v.split(',')
                        .map(|s| s.parse::<usize>().expect("Failed to parse filter argument value"))
                        .collect()
                }),
                thsad2:         variant_args.get("thsad2").map(|v| {
                    v.split(',')
                        .map(|s| s.parse::<usize>().expect("Failed to parse filter argument value"))
                        .collect()
                }),
                planes:         variant_args.get("planes").map(|v| {
                    v.split(',')
                        .map(|s| s.parse::<usize>().expect("Failed to parse filter argument value"))
                        .collect()
                }),
                limit:          variant_args.get("limit").map(|v| {
                    v.split(',')
                        .map(|s| s.parse::<f64>().expect("Failed to parse filter argument value"))
                        .collect()
                }),
                thscd1:         variant_args
                    .get("thscd1")
                    .map(|v| v.parse::<usize>().expect("Failed to parse filter argument value")),
                thscd2:         variant_args
                    .get("thscd2")
                    .map(|v| v.parse::<f64>().expect("Failed to parse filter argument value")),
                prefix:         variant_args.get("prefix").cloned(),
            }),
            "zoom_degrain" => Ok(VapourSynthFilter::ZoomDegrain {
                hpad:               variant_args
                    .get("hpad")
                    .map(|v| v.parse::<usize>().expect("Failed to parse filter argument value")),
                vpad:               variant_args
                    .get("vpad")
                    .map(|v| v.parse::<usize>().expect("Failed to parse filter argument value")),
                pel:                variant_args
                    .get("pel")
                    .map(|v| v.parse::<usize>().expect("Failed to parse filter argument value")),
                levels:             variant_args
                    .get("levels")
                    .map(|v| v.parse::<usize>().expect("Failed to parse filter argument value")),
                chroma:             variant_args
                    .get("chroma")
                    .map(|v| matches!(v.as_str(), "true" | "1")),
                sharp:              variant_args
                    .get("sharp")
                    .map(|v| v.parse::<usize>().expect("Failed to parse filter argument value")),
                rfilter:            variant_args
                    .get("rfilter")
                    .map(|v| v.parse::<usize>().expect("Failed to parse filter argument value")),
                blksize:            variant_args
                    .get("blksize")
                    .map(|v| v.parse::<usize>().expect("Failed to parse filter argument value")),
                blksizev:           variant_args
                    .get("blksizev")
                    .map(|v| v.parse::<usize>().expect("Failed to parse filter argument value")),
                search:             variant_args
                    .get("search")
                    .map(|v| v.parse::<usize>().expect("Failed to parse filter argument value")),
                searchparam:        variant_args
                    .get("searchparam")
                    .map(|v| v.parse::<usize>().expect("Failed to parse filter argument value")),
                pelsearch:          variant_args
                    .get("pelsearch")
                    .map(|v| v.parse::<usize>().expect("Failed to parse filter argument value")),
                lambda:             variant_args
                    .get("lambda")
                    .map(|v| v.parse::<usize>().expect("Failed to parse filter argument value")),
                lsad:               variant_args
                    .get("lsad")
                    .map(|v| v.parse::<usize>().expect("Failed to parse filter argument value")),
                plevel:             variant_args
                    .get("plevel")
                    .map(|v| v.parse::<usize>().expect("Failed to parse filter argument value")),
                global:             variant_args
                    .get("global")
                    .map(|v| matches!(v.as_str(), "true" | "1")),
                pnew:               variant_args
                    .get("pnew")
                    .map(|v| v.parse::<usize>().expect("Failed to parse filter argument value")),
                pzero:              variant_args
                    .get("pzero")
                    .map(|v| v.parse::<usize>().expect("Failed to parse filter argument value")),
                pglobal:            variant_args
                    .get("pglobal")
                    .map(|v| v.parse::<usize>().expect("Failed to parse filter argument value")),
                overlap:            variant_args
                    .get("overlap")
                    .map(|v| v.parse::<usize>().expect("Failed to parse filter argument value")),
                overlapv:           variant_args
                    .get("overlapv")
                    .map(|v| v.parse::<usize>().expect("Failed to parse filter argument value")),
                divide:             variant_args
                    .get("divide")
                    .map(|v| v.parse::<usize>().expect("Failed to parse filter argument value")),
                badsad:             variant_args
                    .get("badsad")
                    .map(|v| v.parse::<usize>().expect("Failed to parse filter argument value")),
                badrange:           variant_args
                    .get("badrange")
                    .map(|v| v.parse::<usize>().expect("Failed to parse filter argument value")),
                truemotion:         variant_args
                    .get("truemotion")
                    .map(|v| matches!(v.as_str(), "true" | "1")),
                meander:            variant_args
                    .get("meander")
                    .map(|v| matches!(v.as_str(), "true" | "1")),
                trymany:            variant_args
                    .get("trymany")
                    .map(|v| matches!(v.as_str(), "true" | "1")),
                fields:             variant_args
                    .get("fields")
                    .map(|v| matches!(v.as_str(), "true" | "1")),
                tff:                variant_args
                    .get("tff")
                    .map(|v| matches!(v.as_str(), "true" | "1")),
                search_coarse:      variant_args
                    .get("search_coarse")
                    .map(|v| v.parse::<usize>().expect("Failed to parse filter argument value")),
                dct:                variant_args
                    .get("dct")
                    .map(|v| v.parse::<usize>().expect("Failed to parse filter argument value")),
                recalculate:        variant_args
                    .get("recalculate")
                    .map(|v| matches!(v.as_str(), "true" | "1")),
                recalc_thsad:       variant_args
                    .get("recalc_thsad")
                    .map(|v| v.parse::<usize>().expect("Failed to parse filter argument value")),
                recalc_smooth:      variant_args
                    .get("recalc_smooth")
                    .map(|v| matches!(v.as_str(), "true" | "1")),
                recalc_blksize:     variant_args
                    .get("recalc_blksize")
                    .map(|v| v.parse::<usize>().expect("Failed to parse filter argument value")),
                recalc_blksizev:    variant_args
                    .get("recalc_blksizev")
                    .map(|v| v.parse::<usize>().expect("Failed to parse filter argument value")),
                recalc_search:      variant_args
                    .get("recalc_search")
                    .map(|v| v.parse::<usize>().expect("Failed to parse filter argument value")),
                recalc_searchparam: variant_args
                    .get("recalc_searchparam")
                    .map(|v| v.parse::<usize>().expect("Failed to parse filter argument value")),
                recalc_lambda:      variant_args
                    .get("recalc_lambda")
                    .map(|v| v.parse::<usize>().expect("Failed to parse filter argument value")),
                recalc_overlap:     variant_args
                    .get("recalc_overlap")
                    .map(|v| v.parse::<usize>().expect("Failed to parse filter argument value")),
                recalc_overlapv:    variant_args
                    .get("recalc_overlapv")
                    .map(|v| v.parse::<usize>().expect("Failed to parse filter argument value")),
                recalc_divide:      variant_args
                    .get("recalc_divide")
                    .map(|v| v.parse::<usize>().expect("Failed to parse filter argument value")),
                thsad:              variant_args
                    .get("thsad")
                    .map(|v| v.parse::<usize>().expect("Failed to parse filter argument value")),
                thsadc:             variant_args
                    .get("thsadc")
                    .map(|v| v.parse::<usize>().expect("Failed to parse filter argument value")),
                plane:              variant_args
                    .get("plane")
                    .map(|v| v.parse::<i32>().expect("Failed to parse filter argument value")),
                limit:              variant_args
                    .get("limit")
                    .map(|v| v.parse::<usize>().expect("Failed to parse filter argument value")),
                limitc:             variant_args
                    .get("limitc")
                    .map(|v| v.parse::<usize>().expect("Failed to parse filter argument value")),
                thscd1:             variant_args
                    .get("thscd1")
                    .map(|v| v.parse::<usize>().expect("Failed to parse filter argument value")),
                thscd2:             variant_args
                    .get("thscd2")
                    .map(|v| v.parse::<usize>().expect("Failed to parse filter argument value")),
            }),
            _ => Err(anyhow::anyhow!("Invalid variant name: {}", variant_name)),
        }
    }
}

impl Display for VapourSynthFilter {
    #[inline]
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            VapourSynthFilter::Crop {
                top,
                bottom,
                left,
                right,
            } => format!(
                "crop:{}{}{}{}",
                top.map(|v| format!("top={};", v)).unwrap_or_default(),
                bottom.map(|v| format!("bottom={};", v)).unwrap_or_default(),
                left.map(|v| format!("left={};", v)).unwrap_or_default(),
                right.map(|v| format!("right={};", v)).unwrap_or_default()
            ),
            VapourSynthFilter::Resize {
                scaler,
                width,
                height,
                format,
            } => format!(
                "resize:{}{}{}{}",
                scaler.map(|v| format!("scaler={};", v)).unwrap_or_default(),
                width.map(|v| format!("width={};", v)).unwrap_or_default(),
                height.map(|v| format!("height={};", v)).unwrap_or_default(),
                format.map(|v| format!("format={};", v.to_pix_fmt_string())).unwrap_or_default(),
            ),
            VapourSynthFilter::Trim {
                start,
                end,
            } => format!(
                "trim:{}{}",
                start.map(|v| format!("start={};", v)).unwrap_or_default(),
                end.map(|v| format!("end={};", v)).unwrap_or_default()
            ),
            VapourSynthFilter::Rescale {
                kernel,
                width,
                height,
                doubler,
            } => format!(
                "rescale:kernel={};width={};height={};doubler={};",
                kernel, width, height, doubler
            ),
            VapourSynthFilter::WNNM {
                sigma,
                block_size,
                block_step,
                group_size,
                bm_range,
                radius,
                ps_num,
                ps_range,
                residual,
                adaptive_aggregation,
            } => format!(
                "wnnm:{}{}{}{}{}{}{}{}{}{}",
                sigma
                    .as_ref()
                    .map(|v| format!("sigma={};", v.iter().join(",")))
                    .unwrap_or_default(),
                block_size.map(|v| format!("block_size={};", v)).unwrap_or_default(),
                block_step.map(|v| format!("block_step={};", v)).unwrap_or_default(),
                group_size.map(|v| format!("group_size={};", v)).unwrap_or_default(),
                bm_range.map(|v| format!("bm_range={};", v)).unwrap_or_default(),
                radius.map(|v| format!("radius={};", v)).unwrap_or_default(),
                ps_num.map(|v| format!("ps_num={};", v)).unwrap_or_default(),
                ps_range.map(|v| format!("ps_range={};", v)).unwrap_or_default(),
                residual.map(|v| format!("residual={};", v)).unwrap_or_default(),
                adaptive_aggregation
                    .map(|v| format!("adaptive_aggregation={};", v))
                    .unwrap_or_default()
            ),
            VapourSynthFilter::Bilateral {
                sigma_s,
                sigma_r,
                planes,
                algorithm,
                pbficnum,
            } => format!(
                "bilateral:{}{}{}{}{}",
                sigma_s
                    .as_ref()
                    .map(|v| format!("sigma_s={};", v.iter().join(",")))
                    .unwrap_or_default(),
                sigma_r
                    .as_ref()
                    .map(|v| format!("sigma_r={};", v.iter().join(",")))
                    .unwrap_or_default(),
                planes
                    .as_ref()
                    .map(|v| format!("planes={};", v.iter().join(",")))
                    .unwrap_or_default(),
                algorithm
                    .as_ref()
                    .map(|v| format!("algorithm={};", v.iter().join(",")))
                    .unwrap_or_default(),
                pbficnum
                    .as_ref()
                    .map(|v| format!("pbficnum={};", v.iter().join(",")))
                    .unwrap_or_default()
            ),
            VapourSynthFilter::Degrain {
                blksize,
                overlap,
                pel,
                radius,
                search,
                mvlambda,
                chroma,
                recalculate,
                thsad,
                thsad2,
                planes,
                limit,
                thscd1,
                thscd2,
                ..
            } => {
                let mut result = String::from("degrain:");
                if let Some(v) = blksize {
                    let _ = write!(result, "blksize={};", v.iter().join(","));
                }
                if let Some(v) = overlap {
                    let _ = write!(result, "overlap={};", v.iter().join(","));
                }
                if let Some(v) = pel {
                    let _ = write!(result, "pel={};", v);
                }
                if let Some(v) = radius {
                    let _ = write!(result, "radius={};", v);
                }
                if let Some(v) = search {
                    let _ = write!(result, "search={};", v);
                }
                if let Some(v) = mvlambda {
                    let _ = write!(result, "mvlambda={};", v);
                }
                if let Some(v) = chroma {
                    let _ = write!(result, "chroma={};", v);
                }
                if let Some(v) = recalculate {
                    let _ = write!(result, "recalculate={};", v);
                }
                if let Some(v) = thsad {
                    let _ = write!(result, "thsad={};", v.iter().join(","));
                }
                if let Some(v) = thsad2 {
                    let _ = write!(result, "thsad2={};", v.iter().join(","));
                }
                if let Some(v) = planes {
                    let _ = write!(result, "planes={};", v.iter().join(","));
                }
                if let Some(v) = limit {
                    let _ = write!(result, "limit={};", v.iter().join(","));
                }
                if let Some(v) = thscd1 {
                    let _ = write!(result, "thscd1={};", v);
                }
                if let Some(v) = thscd2 {
                    let _ = write!(result, "thscd2={};", v);
                }
                result
            },
            VapourSynthFilter::ZoomDegrain {
                hpad,
                vpad,
                pel,
                levels,
                chroma,
                sharp,
                rfilter,
                blksize,
                blksizev,
                search,
                searchparam,
                pelsearch,
                lambda,
                lsad,
                plevel,
                global,
                pnew,
                pzero,
                pglobal,
                overlap,
                overlapv,
                divide,
                badsad,
                badrange,
                truemotion,
                meander,
                trymany,
                fields,
                tff,
                search_coarse,
                dct,
                recalculate,
                recalc_thsad,
                recalc_smooth,
                recalc_blksize,
                recalc_blksizev,
                recalc_search,
                recalc_searchparam,
                recalc_lambda,
                recalc_overlap,
                recalc_overlapv,
                recalc_divide,
                thsad,
                thsadc,
                plane,
                limit,
                limitc,
                thscd1,
                thscd2,
            } => {
                let mut result = String::from("zoom_degrain:");
                if let Some(v) = hpad {
                    let _ = write!(result, "hpad={};", v);
                }
                if let Some(v) = vpad {
                    let _ = write!(result, "vpad={};", v);
                }
                if let Some(v) = pel {
                    let _ = write!(result, "pel={};", v);
                }
                if let Some(v) = levels {
                    let _ = write!(result, "levels={};", v);
                }
                if let Some(v) = chroma {
                    let _ = write!(result, "chroma={};", v);
                }
                if let Some(v) = sharp {
                    let _ = write!(result, "sharp={};", v);
                }
                if let Some(v) = rfilter {
                    let _ = write!(result, "rfilter={};", v);
                }
                if let Some(v) = blksize {
                    let _ = write!(result, "blksize={};", v);
                }
                if let Some(v) = blksizev {
                    let _ = write!(result, "blksizev={};", v);
                }
                if let Some(v) = search {
                    let _ = write!(result, "search={};", v);
                }
                if let Some(v) = searchparam {
                    let _ = write!(result, "searchparam={};", v);
                }
                if let Some(v) = pelsearch {
                    let _ = write!(result, "pelsearch={};", v);
                }
                if let Some(v) = lambda {
                    let _ = write!(result, "lambda={};", v);
                }
                if let Some(v) = lsad {
                    let _ = write!(result, "lsad={};", v);
                }
                if let Some(v) = plevel {
                    let _ = write!(result, "plevel={};", v);
                }
                if let Some(v) = global {
                    let _ = write!(result, "global={};", v);
                }
                if let Some(v) = pnew {
                    let _ = write!(result, "pnew={};", v);
                }
                if let Some(v) = pzero {
                    let _ = write!(result, "pzero={};", v);
                }
                if let Some(v) = pglobal {
                    let _ = write!(result, "pglobal={};", v);
                }
                if let Some(v) = overlap {
                    let _ = write!(result, "overlap={};", v);
                }
                if let Some(v) = overlapv {
                    let _ = write!(result, "overlapv={};", v);
                }
                if let Some(v) = divide {
                    let _ = write!(result, "divide={};", v);
                }
                if let Some(v) = badsad {
                    let _ = write!(result, "badsad={};", v);
                }
                if let Some(v) = badrange {
                    let _ = write!(result, "badrange={};", v);
                }
                if let Some(v) = truemotion {
                    let _ = write!(result, "truemotion={};", v);
                }
                if let Some(v) = meander {
                    let _ = write!(result, "meander={};", v);
                }
                if let Some(v) = trymany {
                    let _ = write!(result, "trymany={};", v);
                }
                if let Some(v) = fields {
                    let _ = write!(result, "fields={};", v);
                }
                if let Some(v) = tff {
                    let _ = write!(result, "tff={};", v);
                }
                if let Some(v) = search_coarse {
                    let _ = write!(result, "search_coarse={};", v);
                }
                if let Some(v) = dct {
                    let _ = write!(result, "dct={};", v);
                }
                if let Some(v) = recalculate {
                    let _ = write!(result, "recalculate={};", v);
                }
                if let Some(v) = recalc_thsad {
                    let _ = write!(result, "recalc_thsad={};", v);
                }
                if let Some(v) = recalc_smooth {
                    let _ = write!(result, "recalc_smooth={};", v);
                }
                if let Some(v) = recalc_blksize {
                    let _ = write!(result, "recalc_blksize={};", v);
                }
                if let Some(v) = recalc_blksizev {
                    let _ = write!(result, "recalc_blksizev={};", v);
                }
                if let Some(v) = recalc_search {
                    let _ = write!(result, "recalc_search={};", v);
                }
                if let Some(v) = recalc_searchparam {
                    let _ = write!(result, "recalc_searchparam={};", v);
                }
                if let Some(v) = recalc_lambda {
                    let _ = write!(result, "recalc_lambda={};", v);
                }
                if let Some(v) = recalc_overlap {
                    let _ = write!(result, "recalc_overlap={};", v);
                }
                if let Some(v) = recalc_overlapv {
                    let _ = write!(result, "recalc_overlapv={};", v);
                }
                if let Some(v) = recalc_divide {
                    let _ = write!(result, "recalc_divide={};", v);
                }
                if let Some(v) = thsad {
                    let _ = write!(result, "thsad={};", v);
                }
                if let Some(v) = thsadc {
                    let _ = write!(result, "thsadc={};", v);
                }
                if let Some(v) = plane {
                    let _ = write!(result, "plane={};", v);
                }
                if let Some(v) = limit {
                    let _ = write!(result, "limit={};", v);
                }
                if let Some(v) = limitc {
                    let _ = write!(result, "limitc={};", v);
                }
                if let Some(v) = thscd1 {
                    let _ = write!(result, "thscd1={};", v);
                }
                if let Some(v) = thscd2 {
                    let _ = write!(result, "thscd2={};", v);
                }
                result
            },
        };
        write!(f, "{s}")
    }
}

impl VapourSynthFilter {
    #[inline]
    pub fn is_script_only(&self) -> bool {
        matches!(self, VapourSynthFilter::Rescale { .. })
    }

    #[inline]
    pub fn invoke_plugin_function<'core>(
        &self,
        core: CoreRef<'core>,
        node: &Node<'core>,
    ) -> Result<Node<'core>> {
        if self.is_script_only() {
            bail!("Cannot invoke script-only filter");
        }

        self.invoke_plugin_function_impl(core, node)
            .map_err(|e: VapourSynthError| anyhow::anyhow!(e))
    }

    fn invoke_plugin_function_impl<'core>(
        &self,
        core: CoreRef<'core>,
        node: &Node<'core>,
    ) -> Result<Node<'core>, VapourSynthError> {
        match self {
            VapourSynthFilter::Crop {
                top,
                bottom,
                left,
                right,
            } => {
                let plugin = Crop {
                    top:    top.map(|v| v as u32),
                    bottom: bottom.map(|v| v as u32),
                    left:   left.map(|v| v as u32),
                    right:  right.map(|v| v as u32),
                };

                Ok(plugin.invoke(core, node)?)
            },
            VapourSynthFilter::Resize {
                scaler,
                width,
                height,
                format,
            } => {
                let scaler = scaler.unwrap_or(Scaler::Bicubic);
                let width = width.map(|v| v as u32);
                let height = height.map(|v| v as u32);
                let format = if let Some(format) = format {
                    Some(format.to_vapoursynth_format().map_err(|e| {
                        VapourSynthError::PluginFunctionError {
                            plugin:   "Resize".to_owned(),
                            function: "Resize".to_owned(),
                            message:  e.to_string(),
                        }
                    })?)
                } else {
                    None
                };

                let node = match scaler {
                    Scaler::Bicubic => Bicubic {
                        width,
                        height,
                        format,
                        ..Default::default()
                    }
                    .invoke(core, node)?,
                    Scaler::Bilinear => Bilinear {
                        width,
                        height,
                        format,
                        ..Default::default()
                    }
                    .invoke(core, node)?,
                    Scaler::Bob => Bob {
                        width,
                        height,
                        format,
                        ..Default::default()
                    }
                    .invoke(core, node)?,
                    Scaler::Lanczos => Lanczos {
                        width,
                        height,
                        format,
                        ..Default::default()
                    }
                    .invoke(core, node)?,
                    Scaler::Point => Point {
                        width,
                        height,
                        format,
                        ..Default::default()
                    }
                    .invoke(core, node)?,
                    Scaler::Spline16 => Spline16 {
                        width,
                        height,
                        format,
                        ..Default::default()
                    }
                    .invoke(core, node)?,
                    Scaler::Spline36 => Spline36 {
                        width,
                        height,
                        format,
                        ..Default::default()
                    }
                    .invoke(core, node)?,
                    Scaler::Spline64 => Spline64 {
                        width,
                        height,
                        format,
                        ..Default::default()
                    }
                    .invoke(core, node)?,
                };

                Ok(node)
            },
            VapourSynthFilter::Trim {
                start,
                end,
            } => {
                let plugin = Trim {
                    first: start.map(|v| v as u32),
                    last: end.map(|v| v as u32),
                    ..Default::default()
                };

                Ok(plugin.invoke(core, node)?)
            },
            VapourSynthFilter::Rescale {
                ..
            } => unreachable!(),
            VapourSynthFilter::WNNM {
                sigma,
                block_size,
                block_step,
                group_size,
                bm_range,
                radius,
                ps_num,
                ps_range,
                residual,
                adaptive_aggregation,
            } => {
                let plugin = WNNM {
                    sigma:                sigma.clone(),
                    block_size:           block_size.map(|v| v as u32),
                    block_step:           block_step.map(|v| v as u32),
                    group_size:           group_size.map(|v| v as u32),
                    bm_range:             bm_range.map(|v| v as u32),
                    radius:               radius.map(|v| v as u32),
                    ps_num:               ps_num.map(|v| v as u32),
                    ps_range:             ps_range.map(|v| v as u32),
                    residual:             *residual,
                    adaptive_aggregation: *adaptive_aggregation,
                    rclip_name:           None,
                };

                Ok(plugin.invoke(core, node, None)?)
            },
            VapourSynthFilter::Bilateral {
                sigma_s,
                sigma_r,
                planes,
                algorithm,
                pbficnum,
            } => {
                let plugin = Bilateral {
                    sigma_s:   sigma_s.clone(),
                    sigma_r:   sigma_r.clone(),
                    planes:    planes.clone(),
                    algorithm: algorithm.clone(),
                    pbficnum:  pbficnum.clone(),
                    ref_name:  None,
                };

                Ok(plugin.invoke(core, node, None)?)
            },
            VapourSynthFilter::Degrain {
                blksize,
                overlap,
                pad,
                pel,
                sharp,
                rfilter,
                radius,
                search,
                searchparam,
                mvlambda,
                chroma,
                lsad,
                plevel,
                globalmv,
                pnew,
                pzero,
                pglobal,
                badsad,
                badrange,
                meander,
                trymany,
                satd,
                recalculate,
                recalc_thsad,
                recalc_smooth,
                recalc_blksize,
                recalc_overlap,
                thsad,
                thsad2,
                planes,
                limit,
                thscd1,
                thscd2,
                prefix,
            } => {
                // Step 1: Create super clip
                let super_plugin = MVUSuper {
                    blksize:      blksize
                        .clone()
                        .map(|v| v.into_iter().map(|u| u as u32).collect()),
                    overlap:      overlap
                        .clone()
                        .map(|v| v.into_iter().map(|u| u as u32).collect()),
                    pad:          pad.clone().map(|v| v.into_iter().map(|u| u as u32).collect()),
                    pel:          pel.map(|v| v as u32),
                    sharp:        sharp.map(|v| v as u32),
                    rfilter:      rfilter.map(|v| v as u32),
                    onelevel:     Some(false),
                    pelclip_name: None,
                    prefix:       prefix.clone(),
                };
                let super_node = super_plugin.invoke(core, node, None)?;

                // Step 2: AnalyseMany to produce vector clips
                let analyse_many_plugin = AnalyseMany {
                    super_name:  None,
                    blksize:     blksize.clone().map(|v| v.into_iter().map(|u| u as u32).collect()),
                    overlap:     overlap.clone().map(|v| v.into_iter().map(|u| u as u32).collect()),
                    levels:      None,
                    search:      search.map(|v| v as u32),
                    searchparam: searchparam.map(|v| v as u32),
                    pelsearch:   None,
                    mvlambda:    mvlambda.map(|v| v as u32),
                    chroma:      *chroma,
                    delta:       Some(1),
                    lsad:        lsad.map(|v| v as u32),
                    plevel:      plevel.map(|v| v as u32),
                    globalmv:    *globalmv,
                    pnew:        pnew.map(|v| v as u32),
                    pzero:       pzero.map(|v| v as u32),
                    pglobal:     pglobal.map(|v| v as u32),
                    badsad:      badsad.map(|v| v as u32),
                    badrange:    badrange.map(|v| v as u32),
                    meander:     *meander,
                    trymany:     trymany.map(|v| v as u32),
                    fields:      Some(false),
                    tff:         Some(false),
                    satd:        *satd,
                    radius:      radius.map(|v| v as u32),
                    prefix:      prefix.clone(),
                };
                let vectors_node = analyse_many_plugin.invoke(core, &super_node)?;

                // Step 3: Optional Recalculate
                let vectors_node = if recalculate.unwrap_or(false) {
                    let recalc_plugin = Recalculate {
                        super_name:   None,
                        vectors_name: None,
                        thsad:        recalc_thsad.map(|v| v as u32),
                        smooth:       *recalc_smooth,
                        blksize:      recalc_blksize
                            .clone()
                            .map(|v| v.into_iter().map(|u| u as u32).collect()),
                        overlap:      recalc_overlap
                            .clone()
                            .map(|v| v.into_iter().map(|u| u as u32).collect()),
                        search:       None,
                        searchparam:  None,
                        mvlambda:     None,
                        chroma:       *chroma,
                        pnew:         None,
                        meander:      None,
                        fields:       None,
                        tff:          None,
                        satd:         None,
                        prefix:       prefix.clone(),
                    };
                    recalc_plugin.invoke(core, &super_node, &vectors_node)?
                } else {
                    vectors_node
                };

                // Step 4: Degrain
                let degrain_plugin = Degrain {
                    clip_name:    None,
                    super_name:   None,
                    vectors_name: None,
                    thsad:        thsad.clone().map(|v| v.into_iter().map(|u| u as u32).collect()),
                    thsad2:       thsad2.clone().map(|v| v.into_iter().map(|u| u as u32).collect()),
                    planes:       planes.clone().map(|v| v.into_iter().map(|u| u as u32).collect()),
                    limit:        limit.clone(),
                    thscd1:       thscd1.map(|v| v as u32),
                    thscd2:       *thscd2,
                    weights:      None,
                    prefix:       prefix.clone(),
                };
                degrain_plugin.invoke(core, node, &super_node, &vectors_node)
            },
            VapourSynthFilter::ZoomDegrain {
                hpad,
                vpad,
                pel,
                levels,
                chroma,
                sharp,
                rfilter,
                blksize,
                blksizev,
                search,
                searchparam,
                pelsearch,
                lambda,
                lsad,
                plevel,
                global,
                pnew,
                pzero,
                pglobal,
                overlap,
                overlapv,
                divide,
                badsad,
                badrange,
                truemotion,
                meander,
                trymany,
                fields,
                tff,
                search_coarse,
                dct,
                recalculate,
                recalc_thsad,
                recalc_smooth,
                recalc_blksize,
                recalc_blksizev,
                recalc_search,
                recalc_searchparam,
                recalc_lambda,
                recalc_overlap,
                recalc_overlapv,
                recalc_divide,
                thsad,
                thsadc,
                plane,
                limit,
                limitc,
                thscd1,
                thscd2,
            } => {
                // Step 1: Create super clip
                let super_plugin = ZMVSuper {
                    hpad:         hpad.map(|v| v as u32),
                    vpad:         vpad.map(|v| v as u32),
                    pel:          pel.map(|v| v as u32),
                    levels:       levels.map(|v| v as u32),
                    chroma:       *chroma,
                    sharp:        sharp.map(|v| v as u32),
                    rfilter:      rfilter.map(|v| v as u32),
                    pelclip_name: None,
                    opt:          Some(4),
                };
                let super_node = super_plugin.invoke(core, node, None)?;

                // Step 2: Analyse forward (isb=0) and backward (isb=1)
                let analyse_forward = ZoomAnalyse {
                    super_clip_name: None,
                    blksize:         blksize.map(|v| v as u32),
                    blksizev:        blksizev.map(|v| v as u32),
                    levels:          levels.map(|v| v as u32),
                    search:          search.map(|v| v as u32),
                    searchparam:     searchparam.map(|v| v as u32),
                    pelsearch:       pelsearch.map(|v| v as u32),
                    isb:             Some(0),
                    lambda:          lambda.map(|v| v as u32),
                    chroma:          *chroma,
                    delta:           Some(-1),
                    truemotion:      *truemotion,
                    lsad:            lsad.map(|v| v as u32),
                    plevel:          plevel.map(|v| v as u32),
                    global:          *global,
                    pnew:            pnew.map(|v| v as u32),
                    pzero:           pzero.map(|v| v as u32),
                    pglobal:         pglobal.map(|v| v as u32),
                    overlap:         overlap.map(|v| v as u32),
                    overlapv:        overlapv.map(|v| v as u32),
                    divide:          divide.map(|v| v as u32),
                    badsad:          badsad.map(|v| v as u32),
                    badrange:        badrange.map(|v| v as u32),
                    opt:             Some(4),
                    meander:         *meander,
                    trymany:         *trymany,
                    fields:          *fields,
                    tff:             *tff,
                    search_coarse:   search_coarse.map(|v| v as u32),
                    dct:             dct.map(|v| v as u32),
                };
                let mvfw_node = analyse_forward.clone().invoke(core, &super_node)?;

                let mut analyse_backward = analyse_forward;
                analyse_backward.isb = Some(1);
                analyse_backward.delta = Some(1);
                let mvbw_node = analyse_backward.invoke(core, &super_node)?;

                // Step 3: Optional Recalculate on both vector clips
                let (mvbw_node, mvfw_node) = if recalculate.unwrap_or(false) {
                    let recalc_plugin = ZoomRecalculate {
                        super_clip_name: None,
                        vectors_name:    None,
                        thsad:           recalc_thsad.map(|v| v as u32),
                        smooth:          *recalc_smooth,
                        blksize:         recalc_blksize.map(|v| v as u32),
                        blksizev:        recalc_blksizev.map(|v| v as u32),
                        search:          recalc_search.map(|v| v as u32),
                        searchparam:     recalc_searchparam.map(|v| v as u32),
                        lambda:          recalc_lambda.map(|v| v as u32),
                        chroma:          *chroma,
                        truemotion:      Some(true),
                        pnew:            Some(25),
                        overlap:         recalc_overlap.map(|v| v as u32),
                        overlapv:        recalc_overlapv.map(|v| v as u32),
                        divide:          recalc_divide.map(|v| v as u32),
                        opt:             Some(4),
                        meander:         Some(true),
                        fields:          *fields,
                        tff:             *tff,
                        dct:             Some(0),
                    };
                    let mvbw_node = recalc_plugin.clone().invoke(core, &super_node, &mvbw_node)?;
                    let mvfw_node = recalc_plugin.invoke(core, &super_node, &mvfw_node)?;
                    (mvbw_node, mvfw_node)
                } else {
                    (mvbw_node, mvfw_node)
                };

                // Step 4: Degrain1
                let degrain_plugin = ZoomDegrainPlugin {
                    clip_name:       None,
                    super_clip_name: None,
                    mvbw_name:       None,
                    mvfw_name:       None,
                    thsad:           thsad.map(|v| v as u32),
                    thsadc:          thsadc.map(|v| v as u32),
                    plane:           *plane,
                    limit:           limit.map(|v| v as u32),
                    limitc:          limitc.map(|v| v as u32),
                    thscd1:          thscd1.map(|v| v as u32),
                    thscd2:          thscd2.map(|v| v as u32),
                    opt:             Some(4),
                };
                degrain_plugin.invoke(core, node, &super_node, &mvbw_node, &mvfw_node)
            },
        }
    }

    #[inline]
    pub fn generate_script(
        &self,
        node_name: NodeVariableName,
    ) -> Result<(Option<Imports>, Vec<Line>)> {
        let (import_lines, filter_lines) = match self {
            VapourSynthFilter::Crop {
                top,
                bottom,
                left,
                right,
            } => {
                let plugin = Crop {
                    top:    top.map(|v| v as u32),
                    bottom: bottom.map(|v| v as u32),
                    left:   left.map(|v| v as u32),
                    right:  right.map(|v| v as u32),
                };

                plugin.generate_script(node_name)?
            },
            VapourSynthFilter::Resize {
                scaler,
                width,
                height,
                format,
            } => {
                let scaler = scaler.unwrap_or(Scaler::Bicubic);
                let width = width.map(|v| v as u32);
                let height = height.map(|v| v as u32);
                let format = if let Some(format) = format {
                    Some(format.to_vapoursynth_format()?)
                } else {
                    None
                };

                match scaler {
                    Scaler::Bicubic => Bicubic {
                        width,
                        height,
                        format,
                        ..Default::default()
                    }
                    .generate_script(node_name)?,
                    Scaler::Bilinear => Bilinear {
                        width,
                        height,
                        format,
                        ..Default::default()
                    }
                    .generate_script(node_name)?,
                    Scaler::Bob => Bob {
                        width,
                        height,
                        format,
                        ..Default::default()
                    }
                    .generate_script(node_name)?,
                    Scaler::Lanczos => Lanczos {
                        width,
                        height,
                        format,
                        ..Default::default()
                    }
                    .generate_script(node_name)?,
                    Scaler::Point => Point {
                        width,
                        height,
                        format,
                        ..Default::default()
                    }
                    .generate_script(node_name)?,
                    Scaler::Spline16 => Spline16 {
                        width,
                        height,
                        format,
                        ..Default::default()
                    }
                    .generate_script(node_name)?,
                    Scaler::Spline36 => Spline36 {
                        width,
                        height,
                        format,
                        ..Default::default()
                    }
                    .generate_script(node_name)?,
                    Scaler::Spline64 => Spline64 {
                        width,
                        height,
                        format,
                        ..Default::default()
                    }
                    .generate_script(node_name)?,
                }
            },
            VapourSynthFilter::Trim {
                start,
                end,
            } => {
                let plugin = Trim {
                    first: start.map(|v| v as u32),
                    last: end.map(|v| v as u32),
                    ..Default::default()
                };

                plugin.generate_script(node_name)?
            },
            VapourSynthFilter::Rescale {
                kernel,
                width,
                height,
                doubler,
            } => RescaleBuilder {
                descale_kernel: kernel.clone(),
                width: *width as f64,
                height: *height as f64,
                doubler: *doubler,
                ..Default::default()
            }
            .generate_script(node_name)?,
            VapourSynthFilter::WNNM {
                sigma,
                block_size,
                block_step,
                group_size,
                bm_range,
                radius,
                ps_num,
                ps_range,
                residual,
                adaptive_aggregation,
            } => {
                let plugin = WNNM {
                    sigma:                sigma.clone(),
                    block_size:           block_size.map(|v| v as u32),
                    block_step:           block_step.map(|v| v as u32),
                    group_size:           group_size.map(|v| v as u32),
                    bm_range:             bm_range.map(|v| v as u32),
                    radius:               radius.map(|v| v as u32),
                    ps_num:               ps_num.map(|v| v as u32),
                    ps_range:             ps_range.map(|v| v as u32),
                    residual:             *residual,
                    adaptive_aggregation: *adaptive_aggregation,
                    rclip_name:           None,
                };

                plugin.generate_script(node_name)?
            },
            VapourSynthFilter::Bilateral {
                sigma_s,
                sigma_r,
                planes,
                algorithm,
                pbficnum,
            } => {
                let plugin = Bilateral {
                    sigma_s:   sigma_s.clone(),
                    sigma_r:   sigma_r.clone(),
                    planes:    planes.clone(),
                    algorithm: algorithm.clone(),
                    pbficnum:  pbficnum.clone(),
                    ref_name:  None,
                };

                plugin.generate_script(node_name)?
            },
            VapourSynthFilter::Degrain {
                blksize,
                overlap,
                pad,
                pel,
                sharp,
                rfilter,
                radius,
                search,
                searchparam,
                mvlambda,
                chroma,
                lsad,
                plevel,
                globalmv,
                pnew,
                pzero,
                pglobal,
                badsad,
                badrange,
                meander,
                trymany,
                satd,
                recalculate,
                recalc_thsad,
                recalc_smooth,
                recalc_blksize,
                recalc_overlap,
                thsad,
                thsad2,
                planes,
                limit,
                thscd1,
                thscd2,
                prefix,
            } => {
                let mut lines = vec![];

                let super_name = "mvu_super".to_string();
                let vectors_name = "mvu_vectors".to_string();

                // Step 1: Super
                let super_plugin = MVUSuper {
                    blksize:      blksize
                        .clone()
                        .map(|v| v.into_iter().map(|u| u as u32).collect()),
                    overlap:      overlap
                        .clone()
                        .map(|v| v.into_iter().map(|u| u as u32).collect()),
                    pad:          pad.clone().map(|v| v.into_iter().map(|u| u as u32).collect()),
                    pel:          pel.map(|v| v as u32),
                    sharp:        sharp.map(|v| v as u32),
                    rfilter:      rfilter.map(|v| v as u32),
                    onelevel:     Some(false),
                    pelclip_name: None,
                    prefix:       prefix.clone(),
                };
                let (_, super_lines) = super_plugin.generate_script(super_name.clone())?;
                lines.extend(super_lines);

                // Step 2: AnalyseMany
                let analyse_many_plugin = AnalyseMany {
                    super_name:  Some(super_name.clone()),
                    blksize:     blksize.clone().map(|v| v.into_iter().map(|u| u as u32).collect()),
                    overlap:     overlap.clone().map(|v| v.into_iter().map(|u| u as u32).collect()),
                    levels:      None,
                    search:      search.map(|v| v as u32),
                    searchparam: searchparam.map(|v| v as u32),
                    pelsearch:   None,
                    mvlambda:    mvlambda.map(|v| v as u32),
                    chroma:      *chroma,
                    delta:       Some(1),
                    lsad:        lsad.map(|v| v as u32),
                    plevel:      plevel.map(|v| v as u32),
                    globalmv:    *globalmv,
                    pnew:        pnew.map(|v| v as u32),
                    pzero:       pzero.map(|v| v as u32),
                    pglobal:     pglobal.map(|v| v as u32),
                    badsad:      badsad.map(|v| v as u32),
                    badrange:    badrange.map(|v| v as u32),
                    meander:     *meander,
                    trymany:     trymany.map(|v| v as u32),
                    fields:      Some(false),
                    tff:         Some(false),
                    satd:        *satd,
                    radius:      radius.map(|v| v as u32),
                    prefix:      prefix.clone(),
                };
                let (_, analyse_lines) =
                    analyse_many_plugin.generate_script(vectors_name.clone())?;
                lines.extend(analyse_lines);

                // Step 3: Optional Recalculate
                let final_vectors_name = if recalculate.unwrap_or(false) {
                    let recalc_name = "mvu_vectors_recalc".to_string();
                    let recalc_plugin = Recalculate {
                        super_name:   Some(super_name.clone()),
                        vectors_name: Some(vectors_name),
                        thsad:        recalc_thsad.map(|v| v as u32),
                        smooth:       *recalc_smooth,
                        blksize:      recalc_blksize
                            .clone()
                            .map(|v| v.into_iter().map(|u| u as u32).collect()),
                        overlap:      recalc_overlap
                            .clone()
                            .map(|v| v.into_iter().map(|u| u as u32).collect()),
                        search:       None,
                        searchparam:  None,
                        mvlambda:     None,
                        chroma:       *chroma,
                        pnew:         None,
                        meander:      None,
                        fields:       None,
                        tff:          None,
                        satd:         None,
                        prefix:       prefix.clone(),
                    };
                    let (_, recalc_lines) = recalc_plugin.generate_script(recalc_name.clone())?;
                    lines.extend(recalc_lines);
                    recalc_name
                } else {
                    vectors_name
                };

                // Step 4: Degrain
                let degrain_plugin = Degrain {
                    clip_name:    Some(node_name.clone()),
                    super_name:   Some(super_name),
                    vectors_name: Some(final_vectors_name),
                    thsad:        thsad.clone().map(|v| v.into_iter().map(|u| u as u32).collect()),
                    thsad2:       thsad2.clone().map(|v| v.into_iter().map(|u| u as u32).collect()),
                    planes:       planes.clone().map(|v| v.into_iter().map(|u| u as u32).collect()),
                    limit:        limit.clone(),
                    thscd1:       thscd1.map(|v| v as u32),
                    thscd2:       *thscd2,
                    weights:      None,
                    prefix:       prefix.clone(),
                };
                let (_, degrain_lines) = degrain_plugin.generate_script(node_name)?;
                lines.extend(degrain_lines);

                (None, lines)
            },
            VapourSynthFilter::ZoomDegrain {
                hpad,
                vpad,
                pel,
                levels,
                chroma,
                sharp,
                rfilter,
                blksize,
                blksizev,
                search,
                searchparam,
                pelsearch,
                lambda,
                lsad,
                plevel,
                global,
                pnew,
                pzero,
                pglobal,
                overlap,
                overlapv,
                divide,
                badsad,
                badrange,
                truemotion,
                meander,
                trymany,
                fields,
                tff,
                search_coarse,
                dct,
                recalculate,
                recalc_thsad,
                recalc_smooth,
                recalc_blksize,
                recalc_blksizev,
                recalc_search,
                recalc_searchparam,
                recalc_lambda,
                recalc_overlap,
                recalc_overlapv,
                recalc_divide,
                thsad,
                thsadc,
                plane,
                limit,
                limitc,
                thscd1,
                thscd2,
            } => {
                let mut lines = vec![];

                let super_name = "zmv_super".to_string();
                let mvfw_name = "zmv_mvfw".to_string();
                let mvbw_name = "zmv_mvbw".to_string();

                // Step 1: Super
                let super_plugin = ZMVSuper {
                    hpad:         hpad.map(|v| v as u32),
                    vpad:         vpad.map(|v| v as u32),
                    pel:          pel.map(|v| v as u32),
                    levels:       levels.map(|v| v as u32),
                    chroma:       *chroma,
                    sharp:        sharp.map(|v| v as u32),
                    rfilter:      rfilter.map(|v| v as u32),
                    pelclip_name: None,
                    opt:          Some(4),
                };
                let (_, super_lines) = super_plugin.generate_script(super_name.clone())?;
                lines.extend(super_lines);

                // Step 2: Analyse forward (isb=0) and backward (isb=1)
                let analyse_plugin = ZoomAnalyse {
                    super_clip_name: Some(super_name.clone()),
                    blksize:         blksize.map(|v| v as u32),
                    blksizev:        blksizev.map(|v| v as u32),
                    levels:          levels.map(|v| v as u32),
                    search:          search.map(|v| v as u32),
                    searchparam:     searchparam.map(|v| v as u32),
                    pelsearch:       pelsearch.map(|v| v as u32),
                    isb:             Some(0),
                    lambda:          lambda.map(|v| v as u32),
                    chroma:          *chroma,
                    delta:           Some(-1),
                    truemotion:      *truemotion,
                    lsad:            lsad.map(|v| v as u32),
                    plevel:          plevel.map(|v| v as u32),
                    global:          *global,
                    pnew:            pnew.map(|v| v as u32),
                    pzero:           pzero.map(|v| v as u32),
                    pglobal:         pglobal.map(|v| v as u32),
                    overlap:         overlap.map(|v| v as u32),
                    overlapv:        overlapv.map(|v| v as u32),
                    divide:          divide.map(|v| v as u32),
                    badsad:          badsad.map(|v| v as u32),
                    badrange:        badrange.map(|v| v as u32),
                    opt:             Some(4),
                    meander:         *meander,
                    trymany:         *trymany,
                    fields:          *fields,
                    tff:             *tff,
                    search_coarse:   search_coarse.map(|v| v as u32),
                    dct:             dct.map(|v| v as u32),
                };
                let (_, analyse_fw_lines) = analyse_plugin.generate_script(mvfw_name.clone())?;
                lines.extend(analyse_fw_lines);

                let mut analyse_backward = analyse_plugin;
                analyse_backward.isb = Some(1);
                analyse_backward.delta = Some(1);
                let (_, analyse_bw_lines) = analyse_backward.generate_script(mvbw_name.clone())?;
                lines.extend(analyse_bw_lines);

                // Step 3: Optional Recalculate on both vector clips
                let (final_mvbw_name, final_mvfw_name) = if recalculate.unwrap_or(false) {
                    let recalc_bw_name = "zmv_mvbw_recalc".to_string();
                    let recalc_fw_name = "zmv_mvfw_recalc".to_string();
                    let recalc_plugin = ZoomRecalculate {
                        super_clip_name: Some(super_name.clone()),
                        vectors_name:    Some(mvbw_name),
                        thsad:           recalc_thsad.map(|v| v as u32),
                        smooth:          *recalc_smooth,
                        blksize:         recalc_blksize.map(|v| v as u32),
                        blksizev:        recalc_blksizev.map(|v| v as u32),
                        search:          recalc_search.map(|v| v as u32),
                        searchparam:     recalc_searchparam.map(|v| v as u32),
                        lambda:          recalc_lambda.map(|v| v as u32),
                        chroma:          *chroma,
                        truemotion:      Some(true),
                        pnew:            Some(25),
                        overlap:         recalc_overlap.map(|v| v as u32),
                        overlapv:        recalc_overlapv.map(|v| v as u32),
                        divide:          recalc_divide.map(|v| v as u32),
                        opt:             Some(4),
                        meander:         Some(true),
                        fields:          *fields,
                        tff:             *tff,
                        dct:             Some(0),
                    };
                    let (_, recalc_bw_lines) =
                        recalc_plugin.generate_script(recalc_bw_name.clone())?;
                    lines.extend(recalc_bw_lines);

                    let mut recalc_forward = recalc_plugin;
                    recalc_forward.vectors_name = Some(mvfw_name);
                    let (_, recalc_fw_lines) =
                        recalc_forward.generate_script(recalc_fw_name.clone())?;
                    lines.extend(recalc_fw_lines);

                    (recalc_bw_name, recalc_fw_name)
                } else {
                    (mvbw_name, mvfw_name)
                };

                // Step 4: Degrain1
                let degrain_plugin = ZoomDegrainPlugin {
                    clip_name:       Some(node_name.clone()),
                    super_clip_name: Some(super_name),
                    mvbw_name:       Some(final_mvbw_name),
                    mvfw_name:       Some(final_mvfw_name),
                    thsad:           thsad.map(|v| v as u32),
                    thsadc:          thsadc.map(|v| v as u32),
                    plane:           *plane,
                    limit:           limit.map(|v| v as u32),
                    limitc:          limitc.map(|v| v as u32),
                    thscd1:          thscd1.map(|v| v as u32),
                    thscd2:          thscd2.map(|v| v as u32),
                    opt:             Some(4),
                };
                let (_, degrain_lines) = degrain_plugin.generate_script(node_name)?;
                lines.extend(degrain_lines);

                (None, lines)
            },
        };

        Ok((import_lines, filter_lines))
    }

    #[inline]
    pub fn can_alter_time(&self) -> bool {
        matches!(self, VapourSynthFilter::Trim { .. })
    }
}
