pub mod bilateral;
pub mod box_blur;
pub mod replace_frames;
pub mod ssimulacra2;
pub mod wnnm;
pub mod xpsnr;

pub(in crate::vapoursynth::plugins::vszip) const NAME: &str = "VapourSynth Zig Image Process";
pub(in crate::vapoursynth::plugins::vszip) const ID: &str = "com.julek.vszip";
pub(in crate::vapoursynth::plugins::vszip) const DOCS: &str =
    "https://github.com/dnjulek/vapoursynth-zip";
