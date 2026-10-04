//! Native quality metrics, and the decode/probe driver they share.
//!
//! Comparing an encode against its source means reading two clips, pairing the
//! frames the pass selected, and handing each pair to a metric. That part is
//! identical for every metric, so it lives in [`probe`] behind one driver,
//! [`probe::probe_selected_frames`], which opens both sides, validates the clip
//! pair, walks the selection, and calls back with the planes of one pair at a
//! time.
//!
//! A metric module therefore only supplies a scorer and a `submit` that turns a
//! pair into whatever that metric consumes. Keeping the driver free of any
//! metric crate's types is what lets the next metric reuse it unchanged: the
//! driver speaks in the [`PlaneSet`] declared in [`probe`], and an adaptor
//! inside the metric translates that into its own descriptor.
//!
//! VMAF and the vship metrics -- SSIMULACRA2, Butteraugli and CVVDP -- are
//! such metrics. Their decoding is handled entirely by the driver, so the two
//! paths -- an FFMS2-encoded output against a VapourSynth reference, and a pair
//! of encoded files -- read identically.
//!
//! vship additionally differs in availability: libvship is opened at runtime
//! and may be absent, so [`vship::native_supported`] decides per metric whether
//! the native path can run at all. Where it cannot, the caller keeps its
//! VapourSynth plugin branch unchanged.

pub mod probe;
pub mod vmaf;
pub mod vship;

pub use probe::{OutputIndexing, PlaneSet, PlaneSource, output_index};
pub use vmaf::score_probed_frames as score_vmaf_frames;
pub use vship::score_probed_frames as score_vship_frames;
