pub mod add_borders;
pub mod crop;
pub mod plane_stats;
// pub mod flip;
// pub mod invert;
pub mod assume_fps;
pub mod box_blur;
pub mod reverse;
pub mod select_every;
pub mod splice;
pub mod trim;

pub(in crate::vapoursynth::plugins::standard) const NAME: &str = "std";
pub(in crate::vapoursynth::plugins::standard) const ID: &str = "com.vapoursynth.std";
pub(in crate::vapoursynth::plugins::standard) const DOCS: &str =
    "https://www.vapoursynth.com/doc/functions.html#video-functions";
