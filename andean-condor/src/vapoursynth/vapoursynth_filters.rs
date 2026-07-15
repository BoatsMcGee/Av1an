use std::{collections::BTreeMap, fmt::Display, str::FromStr};

use anyhow::{bail, Result};
use itertools::Itertools;
use serde::{Deserialize, Serialize};
use vapoursynth::{core::CoreRef, node::Node};

use crate::{
    ffmpeg::FFPixelFormat,
    vapoursynth::{
        plugins::{
            rescale::{Doubler, RescaleBuilder, VSJETKernel},
            resize::{
                bicubic::Bicubic,
                bilinear::Bilinear,
                bob::Bob,
                lanczos::Lanczos,
                point::Point,
                spline16::Spline16,
                spline36::Spline36,
                spline64::Spline64,
                Scaler,
            },
            standard::{crop::Crop, trim::Trim},
            vszip::{bilateral::Bilateral, wnnm::WNNM},
        },
        script_builder::{
            script::{Imports, Line},
            NodeVariableName,
            VapourSynthPluginScript,
        },
    },
};

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
}

impl FromStr for VapourSynthFilter {
    type Err = anyhow::Error;

    #[inline]
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // Parse the string representation of the enum variant
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
                    Some(format.to_vapoursynth_format()?)
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
        };

        Ok((import_lines, filter_lines))
    }

    #[inline]
    pub fn can_alter_time(&self) -> bool {
        matches!(self, VapourSynthFilter::Trim { .. })
    }
}
