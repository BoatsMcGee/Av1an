use std::{
    collections::{BTreeMap, HashMap},
    fmt::Write as _,
    io::{Cursor, Write},
    path::PathBuf,
    sync::{Arc, Condvar, Mutex},
};

use anyhow::{Context, Result, bail, ensure};
use av_decoders::{Decoder, Ffms2Decoder, VapoursynthDecoder};
pub use av_decoders::{DecoderError, ModifyNode};
use thiserror::Error as ThisError;
use vapoursynth::{core::CoreRef, map::OwnedMap, node::Node};

use crate::{
    core::input::clip_info::ClipInfo,
    ffmpeg::get_clip_info,
    models::input::{
        ImportMethod,
        Input as InputModel,
        VapourSynthImportMethod,
        VapourSynthScriptSource,
    },
    vapoursynth::{
        get_api,
        get_clip_info as get_vs_clip_info,
        get_core,
        plugins::dgdecodenv::DGSource,
    },
};

pub mod clip_info;
pub mod color_range;
pub mod pixel_format;

pub enum Input {
    Video {
        path:          PathBuf,
        import_method: ImportMethod,
        decoder:       Decoder,
        clip_info:     Option<ClipInfo>,
    },
    VapourSynth {
        path:          PathBuf,
        import_method: VapourSynthImportMethod,
        cache_path:    Option<PathBuf>,
        decoder:       Decoder,
        clip_info:     Option<ClipInfo>,
    },
    VapourSynthScript {
        source:    VapourSynthScriptSource,
        variables: HashMap<String, String>,
        index:     u8,
        decoder:   Decoder,
        clip_info: Option<ClipInfo>,
    },
}

impl Input {
    #[inline]
    pub fn as_data(&self) -> InputModel {
        match self {
            Input::Video {
                path,
                import_method,
                ..
            } => InputModel::Video {
                path:          path.clone(),
                import_method: import_method.clone(),
            },
            Input::VapourSynth {
                path,
                import_method,
                cache_path,
                ..
            } => InputModel::VapourSynth {
                path:          path.clone(),
                import_method: import_method.clone(),
                cache_path:    cache_path.clone(),
            },
            Input::VapourSynthScript {
                source,
                variables,
                index,
                ..
            } => InputModel::VapourSynthScript {
                source:    source.clone(),
                variables: variables.clone(),
                index:     *index,
            },
        }
    }

    #[inline]
    pub fn validate(data: &InputModel) -> Result<()> {
        match data {
            InputModel::Video {
                path, ..
            } => {
                ensure!(path.exists(), InputError::VideoFileNotFound(path.clone()));
                if let Some(ext) = path.extension() {
                    ensure!(
                        ext != "vpy" && ext != "py",
                        InputError::NotAVideoFile(path.clone())
                    );
                }
                Ok(())
            },
            InputModel::VapourSynth {
                path, ..
            } => {
                // TODO: Check if VapourSynth + plugin is installed and static cache it
                ensure!(path.exists(), InputError::VideoFileNotFound(path.clone()));
                if let Some(ext) = path.extension() {
                    ensure!(
                        ext != "vpy" && ext != "py",
                        InputError::NotAVideoFile(path.clone())
                    );
                }

                Ok(())
            },
            InputModel::VapourSynthScript {
                source, ..
            } => {
                // TODO: Check if VapourSynth is installed and static cache it
                match source {
                    VapourSynthScriptSource::Path(path) => {
                        ensure!(
                            path.exists(),
                            InputError::VapourSynthScriptNotFound(path.clone())
                        );
                        if let Some(ext) = path.extension() {
                            ensure!(
                                ext == "vpy" || ext == "py",
                                InputError::NotAVapourSynthScript(path.clone())
                            );
                        }
                    },
                    VapourSynthScriptSource::Text(_) => (),
                }

                Ok(())
            },
        }
    }

    #[inline]
    pub fn from_video(data: &InputModel) -> Result<Self> {
        match data {
            InputModel::Video {
                path,
                import_method,
            } => {
                Input::validate(data)?;
                match import_method {
                    // ImportMethod::FFmpeg {} => {
                    //     unimplemented!();
                    // },
                    ImportMethod::FFMS2 {
                        index,
                    } => {
                        let ffms2_decoder = Ffms2Decoder::new(path, *index)?;
                        let decoder = Decoder::from_decoder_impl(av_decoders::DecoderImpl::Ffms2(
                            ffms2_decoder,
                        ))?;

                        Ok(Input::Video {
                            path: path.clone(),
                            import_method: import_method.clone(),
                            decoder,
                            clip_info: None,
                        })
                    },
                }
            },
            _ => panic!("expected `Input::Video`"),
        }
    }

    #[inline]
    pub fn from_vapoursynth(data: &InputModel, modify_node: Option<ModifyNode>) -> Result<Self> {
        Input::validate(data)?;
        match data {
            InputModel::VapourSynth {
                path,
                import_method,
                cache_path,
            } => {
                // Create the source once, in a fixed script. A `ModifyNode` is run again for
                // every decoded frame, so creating the source inside one reopened the file
                // and reloaded its index for each frame. Paths and options are passed as
                // script variables, so they are never interpreted as code.
                if let VapourSynthImportMethod::DGDecNV {
                    dgindexnv_executable,
                } = import_method
                {
                    DGSource::index_video(
                        path,
                        cache_path.as_deref(),
                        dgindexnv_executable.as_deref(),
                    )
                    .map_err(|_| DecoderError::UnsupportedDecoder)?;
                }
                let call = SourceCall::new(import_method);

                let mut script = String::from("from vapoursynth import core\nkwargs = {}\n");
                let mut variables = HashMap::from([(
                    "condor_source".to_owned(),
                    std::path::absolute(path)?.display().to_string(),
                )]);
                if let Some(cache_path) = cache_path {
                    writeln!(script, "kwargs[\"{}\"] = condor_cache", call.cache_argument)?;
                    variables.insert(
                        "condor_cache".to_owned(),
                        std::path::absolute(cache_path)?.display().to_string(),
                    );
                }
                if let (Some(track_argument), Some(track)) = (call.track_argument, call.track) {
                    writeln!(script, "kwargs[\"{track_argument}\"] = int(condor_track)")?;
                    variables.insert("condor_track".to_owned(), track.to_string());
                }
                writeln!(
                    script,
                    "core.{}.{}(source=condor_source, **kwargs).set_output(0)",
                    call.namespace, call.function
                )?;

                let mut vs_decoder = VapoursynthDecoder::from_script(&script, variables, Some(0))?;
                if let Some(node_modifier) = modify_node {
                    vs_decoder.register_node_modifier(node_modifier)?;
                }
                let decoder =
                    Decoder::from_decoder_impl(av_decoders::DecoderImpl::Vapoursynth(vs_decoder))?;

                Ok(Input::VapourSynth {
                    path: path.clone(),
                    import_method: import_method.clone(),
                    cache_path: cache_path.clone(),
                    // modify_node,
                    decoder,
                    clip_info: None,
                })
            },
            InputModel::VapourSynthScript {
                source,
                variables,
                index,
            } => {
                let mut vs_decoder = match source {
                    VapourSynthScriptSource::Path(path) => {
                        VapoursynthDecoder::from_file(path, variables.clone(), Some(*index))?
                    },
                    VapourSynthScriptSource::Text(script) => {
                        VapoursynthDecoder::from_script(script, variables.clone(), Some(*index))?
                    },
                };
                if let Some(node_modifier) = modify_node {
                    vs_decoder.register_node_modifier(node_modifier)?;
                }

                let decoder =
                    Decoder::from_decoder_impl(av_decoders::DecoderImpl::Vapoursynth(vs_decoder))?;

                Ok(Input::VapourSynthScript {
                    source: source.clone(),
                    variables: variables.clone(),
                    index: *index,
                    // modify_node,
                    decoder,
                    clip_info: None,
                })
            },
            _ => panic!("expected `Input::VapourSynth` or `Input::VapourSynthScript`"),
        }
    }

    #[inline]
    pub fn from_data(data: &InputModel) -> Result<Self> {
        match data {
            InputModel::Video {
                ..
            } => Input::from_video(data),
            InputModel::VapourSynth {
                ..
            }
            | InputModel::VapourSynthScript {
                ..
            } => Input::from_vapoursynth(data, None),
        }
    }

    #[inline]
    pub fn decoder(&mut self) -> &mut Decoder {
        match self {
            Input::Video {
                decoder, ..
            } => decoder,
            Input::VapourSynth {
                decoder, ..
            } => decoder,
            Input::VapourSynthScript {
                decoder, ..
            } => decoder,
        }
    }

    #[inline]
    pub fn clip_info(&mut self) -> Result<ClipInfo> {
        let clip_info = match self {
            Input::Video {
                path,
                clip_info,
                ..
            } => {
                // Ideally, these values are retrieved via VideoDetails instead
                if clip_info.is_none() {
                    let info = get_clip_info(path.as_path())?;
                    *clip_info = Some(info);
                }

                clip_info.as_ref()
            },
            Input::VapourSynth {
                decoder,
                clip_info,
                ..
            }
            | Input::VapourSynthScript {
                decoder,
                clip_info,
                ..
            } => {
                if clip_info.is_none() {
                    let info = {
                        let vapoursynth_decoder =
                            decoder.get_vapoursynth_impl().expect("Decoder is VapourSynth");
                        let node = vapoursynth_decoder.get_output(
                            vapoursynth_decoder.get_output_index(),
                            vapoursynth_decoder.get_node_modifier(),
                        )?;
                        get_vs_clip_info(&node)?
                    };
                    *clip_info = Some(info);
                }

                clip_info.as_ref()
            },
        };

        Ok(*clip_info.expect("ClipInfo is Some"))
    }

    #[inline]
    pub fn y4m_header(&mut self, frames: Option<usize>) -> Result<String> {
        let clip_info = self.clip_info()?;
        let decoder = match self {
            Input::Video {
                decoder, ..
            } => decoder,
            Input::VapourSynth {
                decoder, ..
            } => decoder,
            Input::VapourSynthScript {
                decoder, ..
            } => decoder,
        };
        let details = decoder.get_video_details();

        let chroma_str = match details.chroma_sampling {
            av_decoders::v_frame::chroma::ChromaSubsampling::Monochrome => "mono",
            av_decoders::v_frame::chroma::ChromaSubsampling::Yuv420 => "420",
            av_decoders::v_frame::chroma::ChromaSubsampling::Yuv422 => "422",
            av_decoders::v_frame::chroma::ChromaSubsampling::Yuv444 => "444",
        };
        let chroma_header = format!(
            "{}{}{}",
            chroma_str,
            match details.chroma_sampling {
                av_decoders::v_frame::chroma::ChromaSubsampling::Monochrome => "",
                _ if details.bit_depth > 8 => "p",
                _ => "",
            },
            match details.bit_depth {
                _ if details.bit_depth > 8 => format!("{}", details.bit_depth),
                _ => String::new(),
            },
        );

        let header = format!(
            "YUV4MPEG2 C{} W{} H{} F{}:{} Ip A0:0{}\n",
            chroma_header,
            clip_info.resolution.0,
            clip_info.resolution.1,
            clip_info.frame_rate.numer(),
            clip_info.frame_rate.denom(),
            frames.map_or_else(String::new, |frames| format!(" XLENGTH {}", frames))
        );

        Ok(header)
    }

    #[inline]
    pub fn y4m_frame(&mut self, index: usize) -> Result<Cursor<Vec<u8>>> {
        static FRAME_HEADER: &str = "FRAME\n";
        static CONTEXT: &str = "get y4m frame";

        let mut stream = Cursor::new(Vec::new());
        stream.write_all(FRAME_HEADER.as_bytes())?; // Frame header

        match self {
            Input::Video {
                decoder, ..
            } => {
                let details = decoder.get_video_details();
                match details.bit_depth {
                    8 => {
                        let frame = decoder.get_video_frame::<u8>(index).context(CONTEXT)?;

                        let mut planes = Vec::new();
                        planes.extend(frame.y_plane.byte_data());
                        if let Some(plane) = frame.u_plane {
                            planes.extend(plane.byte_data());
                        }
                        if let Some(plane) = frame.v_plane {
                            planes.extend(plane.byte_data());
                        }
                        stream.write_all(&planes)?;
                    },
                    _ => {
                        let frame = decoder.get_video_frame::<u16>(index).context(CONTEXT)?;

                        let mut planes = Vec::new();
                        planes.extend(frame.y_plane.byte_data());
                        if let Some(plane) = frame.u_plane {
                            planes.extend(plane.byte_data());
                        }
                        if let Some(plane) = frame.v_plane {
                            planes.extend(plane.byte_data());
                        }
                        stream.write_all(&planes)?;
                    },
                }
            },
            Input::VapourSynth {
                decoder, ..
            }
            | Input::VapourSynthScript {
                decoder, ..
            } => {
                let vapoursynth_decoder =
                    decoder.get_vapoursynth_impl().expect("Decoder is VapourSynth");
                let node = vapoursynth_decoder.get_output(
                    vapoursynth_decoder.get_output_index(),
                    vapoursynth_decoder.get_node_modifier(),
                )?;
                let frame = node.get_frame(index)?;
                let framedata = {
                    let mut data = Vec::new();
                    let planes_indices =
                        if frame.format().color_family() == vapoursynth::format::ColorFamily::RGB {
                            [1, 2, 0]
                        } else {
                            [0, 1, 2]
                        };
                    for plane_index in planes_indices {
                        if let Ok(plane_data) = frame.data(plane_index) {
                            data.extend_from_slice(plane_data);
                        } else {
                            for row in 0..frame.height(plane_index) {
                                data.extend_from_slice(frame.data_row(plane_index, row));
                            }
                        }
                    }
                    data
                };

                stream.write_all(&framedata)?;
            },
        }

        Ok(stream)
    }

    /// Whether [`Input::frame_sources`] can deliver frames to several readers
    /// at once without them slowing each other down.
    ///
    /// True for native inputs (one decoder per reader) and for BestSource,
    /// FFMS2, and L-SMASH imports (instances are shared only as far as the
    /// plugin serves several positions). False for DGDecNV, which has not been
    /// measured, and for VapourSynth scripts, whose source plugin is unknown
    /// and is shared by every reader.
    #[inline]
    pub fn streams_concurrently(&self) -> bool {
        match self {
            Input::Video {
                ..
            } => true,
            Input::VapourSynth {
                import_method, ..
            } => !matches!(import_method, VapourSynthImportMethod::DGDecNV { .. }),
            Input::VapourSynthScript {
                ..
            } => false,
        }
    }

    /// Creates `count` independent [`FrameSource`]s, so frames can be streamed
    /// on several threads at once.
    ///
    /// - Native inputs: each source opens its own decoder when it is opened.
    /// - `Input::VapourSynth`: each source plugin instance serves as many
    ///   readers as it can without seeking back and forth between them. That is
    ///   4 for BestSource, which keeps up to 4 decoders, and 1 for FFMS2,
    ///   L-SMASH, and DGDecNV, which decode from a single position. The input's
    ///   `ModifyNode` is applied to every source.
    /// - `Input::VapourSynthScript`: every source shares the script's output.
    #[inline]
    pub fn frame_sources(&mut self, count: usize) -> Result<Vec<FrameSource<'_>>> {
        let data = self.as_data();
        match self {
            Input::Video {
                ..
            } => Ok(std::iter::repeat_with(|| FrameSource {
                kind: FrameSourceKind::Native(data.clone()),
            })
            .take(count)
            .collect()),
            Input::VapourSynth {
                path,
                import_method,
                cache_path,
                decoder,
                ..
            } => {
                let call = SourceCall::new(import_method);
                let vapoursynth_decoder =
                    &*decoder.get_vapoursynth_impl().expect("Decoder is VapourSynth");
                let core = get_core(&vapoursynth_decoder.env)?;
                let source = std::path::absolute(&*path)?.display().to_string();
                let cache = cache_path
                    .as_deref()
                    .map(std::path::absolute)
                    .transpose()?
                    .map(|cache| cache.display().to_string());
                let mut sources = Vec::with_capacity(count);
                let mut instance = None;
                for index in 0..count {
                    let node = if index < call.readers_per_instance {
                        // The input's own source instance
                        vapoursynth_decoder.get_output(
                            vapoursynth_decoder.get_output_index(),
                            vapoursynth_decoder.get_node_modifier(),
                        )?
                    } else {
                        if index % call.readers_per_instance == 0 {
                            instance = Some(call.invoke(core, &source, cache.as_deref())?);
                        }
                        let node = instance.clone().expect("source instance exists");
                        match vapoursynth_decoder.get_node_modifier() {
                            Some(modify_node) => modify_node(core, Some(node))?,
                            None => node,
                        }
                    };
                    sources.push(FrameSource {
                        kind: FrameSourceKind::VapourSynth(node),
                    });
                }
                Ok(sources)
            },
            Input::VapourSynthScript {
                decoder, ..
            } => {
                let vapoursynth_decoder =
                    &*decoder.get_vapoursynth_impl().expect("Decoder is VapourSynth");
                std::iter::repeat_with(|| {
                    Ok(FrameSource {
                        kind: FrameSourceKind::VapourSynth(vapoursynth_decoder.get_output(
                            vapoursynth_decoder.get_output_index(),
                            vapoursynth_decoder.get_node_modifier(),
                        )?),
                    })
                })
                .take(count)
                .collect()
            },
        }
    }

    #[inline]
    pub fn y4m_frames(
        &mut self,
        frame_sender: crossbeam_channel::Sender<Cursor<Vec<u8>>>,
        frame_indices: &[usize],
    ) -> Result<()> {
        static FRAME_HEADER: &str = "FRAME\n";
        static CONTEXT: &str = "get y4m frame";
        let bit_depth = self.clip_info()?.format_info.as_bit_depth()?;

        match self {
            Input::Video {
                decoder, ..
            } => {
                for index in frame_indices {
                    let framedata = match bit_depth {
                        8 => {
                            let frame = decoder.get_video_frame::<u8>(*index).context(CONTEXT)?;

                            let mut planes = Vec::new();
                            planes.extend(frame.y_plane.byte_data());
                            if let Some(plane) = frame.u_plane {
                                planes.extend(plane.byte_data());
                            }
                            if let Some(plane) = frame.v_plane {
                                planes.extend(plane.byte_data());
                            }
                            planes
                        },
                        _ => {
                            let frame = decoder.get_video_frame::<u16>(*index).context(CONTEXT)?;

                            let mut planes = Vec::new();
                            planes.extend(frame.y_plane.byte_data());
                            if let Some(plane) = frame.u_plane {
                                planes.extend(plane.byte_data());
                            }
                            if let Some(plane) = frame.v_plane {
                                planes.extend(plane.byte_data());
                            }
                            planes
                        },
                    };
                    let mut cursor = Cursor::new(Vec::new());
                    cursor.write_all(FRAME_HEADER.as_bytes())?;
                    cursor.write_all(&framedata)?;
                    frame_sender.send(cursor)?;
                }
                drop(frame_sender);
            },
            Input::VapourSynth {
                decoder, ..
            }
            | Input::VapourSynthScript {
                decoder, ..
            } => {
                let vapoursynth_decoder =
                    decoder.get_vapoursynth_impl().expect("Decoder is VapourSynth");
                let node = vapoursynth_decoder.get_output(
                    vapoursynth_decoder.get_output_index(),
                    vapoursynth_decoder.get_node_modifier(),
                )?;
                let window = std::thread::available_parallelism().map_or(24, |n| n.get());
                send_vapoursynth_frames(&node, &frame_sender, frame_indices, window)?;
                drop(frame_sender);
            },
        }

        Ok(())
    }
}

/// A handle for streaming an [`Input`]'s frames on another thread, created by
/// [`Input::frame_sources`].
///
/// Sources are independent, so several can decode concurrently on different
/// threads. Move a source to the thread that will read frames and call
/// [`FrameSource::open`] there.
pub struct FrameSource<'a> {
    kind: FrameSourceKind<'a>,
}

enum FrameSourceKind<'a> {
    /// VapourSynth nodes are thread-safe.
    VapourSynth(Node<'a>),
    /// Native decoders are not thread-safe, so each source opens its own
    /// decoder on the thread that reads from it.
    Native(InputModel),
}

impl<'a> FrameSource<'a> {
    /// Opens the source for reading on the current thread.
    ///
    /// # Errors
    ///
    /// Returns an error if a native decoder cannot be opened.
    #[inline]
    pub fn open(self) -> Result<FrameReader<'a>> {
        Ok(FrameReader {
            kind: match self.kind {
                FrameSourceKind::VapourSynth(node) => FrameReaderKind::VapourSynth(node),
                FrameSourceKind::Native(data) => {
                    FrameReaderKind::Native(Box::new(Input::from_data(&data)?))
                },
            },
        })
    }
}

/// Reads frames for a [`FrameSource`] on a single thread.
pub struct FrameReader<'a> {
    kind: FrameReaderKind<'a>,
}

enum FrameReaderKind<'a> {
    VapourSynth(Node<'a>),
    Native(Box<Input>),
}

impl FrameReader<'_> {
    /// Sends `frame_indices` as Y4M frames through `frame_sender`, in order.
    ///
    /// VapourSynth sources keep at most `window` frames requested but not yet
    /// sent. With a bounded `frame_sender`, this blocks while the receiver is
    /// behind, so resident frames stay bounded by `window` and the channel
    /// capacity.
    ///
    /// # Errors
    ///
    /// Returns an error if a frame cannot be decoded or the receiver is gone.
    #[inline]
    pub fn y4m_frames(
        &mut self,
        frame_sender: &crossbeam_channel::Sender<Cursor<Vec<u8>>>,
        frame_indices: &[usize],
        window: usize,
    ) -> Result<()> {
        match &mut self.kind {
            FrameReaderKind::VapourSynth(node) => {
                send_vapoursynth_frames(node, frame_sender, frame_indices, window.max(1))
            },
            FrameReaderKind::Native(input) => input.y4m_frames(frame_sender.clone(), frame_indices),
        }
    }
}

/// How a VapourSynth import method calls its source plugin.
struct SourceCall {
    namespace:            &'static str,
    function:             &'static str,
    cache_argument:       &'static str,
    track_argument:       Option<&'static str>,
    track:                Option<u8>,
    /// How many concurrent readers one instance serves without seeking back
    /// and forth between their positions.
    readers_per_instance: usize,
}

impl SourceCall {
    fn new(import_method: &VapourSynthImportMethod) -> Self {
        match import_method {
            VapourSynthImportMethod::LSMASHWorks {
                index,
            } => Self {
                namespace:            "lsmas",
                function:             "LWLibavSource",
                cache_argument:       "cachefile",
                track_argument:       Some("stream_index"),
                track:                *index,
                readers_per_instance: 1,
            },
            VapourSynthImportMethod::FFMS2 {
                index,
            } => Self {
                namespace:            "ffms2",
                function:             "Source",
                cache_argument:       "cachefile",
                track_argument:       Some("track"),
                track:                *index,
                readers_per_instance: 1,
            },
            VapourSynthImportMethod::BestSource {
                index,
            } => Self {
                namespace:            "bs",
                function:             "VideoSource",
                cache_argument:       "cachepath",
                track_argument:       Some("track"),
                track:                *index,
                // BestSource keeps up to 4 decoders (`maxdecoders`)
                readers_per_instance: 4,
            },
            VapourSynthImportMethod::DGDecNV {
                ..
            } => Self {
                namespace:            "dgdecodenv",
                function:             "DGSource",
                cache_argument:       "indexing_path",
                track_argument:       None,
                track:                None,
                readers_per_instance: 1,
            },
        }
    }

    /// Creates a new instance of the source with the VapourSynth API.
    fn invoke<'core>(
        &self,
        core: CoreRef<'core>,
        source: &str,
        cache: Option<&str>,
    ) -> Result<Node<'core>> {
        let Some(plugin) = core.get_plugin_by_namespace(self.namespace)? else {
            bail!("VapourSynth plugin \"{}\" is not installed", self.namespace);
        };
        let mut arguments = OwnedMap::new(get_api()?);
        arguments.set_data("source", source.as_bytes())?;
        if let Some(cache) = cache {
            arguments.set_data(self.cache_argument, cache.as_bytes())?;
        }
        if let (Some(track_argument), Some(track)) = (self.track_argument, self.track) {
            arguments.set_int(track_argument, i64::from(track))?;
        }
        let result = plugin.invoke(self.function, &arguments)?;
        if let Some(error) = result.error() {
            bail!("{}.{} failed: {error}", self.namespace, self.function);
        }
        Ok(result.get_video_node("clip")?)
    }
}

/// Streams `frame_indices` from `node` as Y4M frames, in order, while keeping
/// at most `window` frames requested but not yet sent.
fn send_vapoursynth_frames(
    node: &Node<'_>,
    frame_sender: &crossbeam_channel::Sender<Cursor<Vec<u8>>>,
    frame_indices: &[usize],
    window: usize,
) -> Result<()> {
    static FRAME_HEADER: &str = "FRAME\n";
    static CONTEXT: &str = "get y4m frame";
    let state = Arc::new((Mutex::new((BTreeMap::new(), 0_usize)), Condvar::new()));
    let mut next_request = 0;
    let mut next_send = 0;

    let result = (|| -> Result<()> {
        while next_send < frame_indices.len() {
            while next_request < frame_indices.len() && next_request - next_send < window {
                let state = Arc::clone(&state);
                let position = next_request;
                node.get_frame_async(frame_indices[position], move |frame, _index, _node| {
                    let (lock, condvar) = &*state;
                    let mut pending = lock.lock().expect("mutex should acquire lock");
                    pending.0.insert(position, frame.map_err(|e| e.to_string()));
                    pending.1 += 1;
                    condvar.notify_all();
                });
                next_request += 1;
            }

            let frame = {
                let (lock, condvar) = &*state;
                let pending = lock.lock().expect("mutex should acquire lock");
                let mut pending = condvar
                    .wait_while(pending, |p| !p.0.contains_key(&next_send))
                    .expect("Condvar should be notified");
                pending.0.remove(&next_send).expect("Map should have frame")
            };
            let frame = frame.map_err(|error| {
                anyhow::anyhow!("{CONTEXT} {}: {error}", frame_indices[next_send])
            })?;

            let framedata = {
                let mut data = Vec::new();
                data.extend_from_slice(FRAME_HEADER.as_bytes());
                let planes_indices =
                    if frame.format().color_family() == vapoursynth::format::ColorFamily::RGB {
                        [1, 2, 0]
                    } else {
                        [0, 1, 2]
                    };

                for plane_index in planes_indices {
                    if let Ok(plane_data) = frame.data(plane_index) {
                        data.extend_from_slice(plane_data);
                    } else {
                        for row in 0..frame.height(plane_index) {
                            data.extend_from_slice(frame.data_row(plane_index, row));
                        }
                    }
                }
                data
            };
            drop(frame);

            frame_sender.send(Cursor::new(framedata))?;
            next_send += 1;
        }
        Ok(())
    })();

    // Wait for every outstanding request before the node can be dropped.
    let (lock, condvar) = &*state;
    drop(
        condvar
            .wait_while(lock.lock().expect("mutex should acquire lock"), |p| {
                p.1 < next_request
            })
            .expect("Condvar should be notified"),
    );
    result
}

#[derive(Debug, ThisError)]
pub enum InputError {
    #[error("No video file found at {0}")]
    VideoFileNotFound(PathBuf),
    #[error("No VapourSynth script file found at {0}")]
    VapourSynthScriptNotFound(PathBuf),
    #[error("File {0} is not a video file")]
    NotAVideoFile(PathBuf),
    #[error("File {0} is not a VapourSynth script")]
    NotAVapourSynthScript(PathBuf),
}
