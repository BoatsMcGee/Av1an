//! VapourSynth-backed inputs: source invocation, filter composition, script
//! generation, and frame streaming.

use std::{
    collections::{BTreeMap, HashMap},
    fmt::Write as _,
    sync::{Arc, Condvar, Mutex, atomic::AtomicBool},
};

use anyhow::{Context, Result, bail};
use av_decoders::{Decoder, DecoderError, ModifyNode, VapoursynthDecoder};
use vapoursynth::{
    core::CoreRef,
    format::{ColorFamily, SampleType},
    frame::FrameRef,
    map::OwnedMap,
    node::Node,
};

use crate::{
    core::input::{Input, InputError, OpenProgress, clip_info::ClipInfo},
    models::input::{
        ImportMethod, Input as InputModel, VapourSynthImportMethod, VapourSynthScriptSource,
    },
    vapoursynth::{
        get_api, get_clip_info, get_core,
        plugins::{
            bestsource::VideoSource, dgdecodenv::DGSource, ffms2::Source, lsmash::LWLibavSource,
        },
        script_builder::{
            VapourSynthPluginScript,
            script::{Imports, Line, VapourSynthScript},
        },
        vapoursynth_filters::VapourSynthFilter,
    },
};

/// The node name a generated script's source and filters are applied to.
const SCRIPT_NODE_NAME: &str = "clip";

/// Precedes every frame's planes in a y4m stream.
const FRAME_HEADER: &[u8] = b"FRAME\n";

/// Validates that a `VapourSynth` input path names an existing video file.
pub fn validate(path: &std::path::Path) -> Result<()> {
    anyhow::ensure!(
        path.exists(),
        InputError::VideoFileNotFound(path.to_owned())
    );
    if let Some(ext) = path.extension() {
        anyhow::ensure!(
            ext != "vpy" && ext != "py",
            InputError::NotAVideoFile(path.to_owned())
        );
    }
    Ok(())
}

/// Validates that a `VapourSynthScript` input's source is usable.
pub fn validate_script(source: &VapourSynthScriptSource) -> Result<()> {
    let VapourSynthScriptSource::Path(path) = source else {
        return Ok(());
    };
    anyhow::ensure!(
        path.exists(),
        InputError::VapourSynthScriptNotFound(path.clone())
    );
    if let Some(ext) = path.extension() {
        anyhow::ensure!(
            ext == "vpy" || ext == "py",
            InputError::NotAVapourSynthScript(path.clone())
        );
    }
    Ok(())
}

/// The input's own filter list, which is empty for a native input.
#[inline]
fn filters_of(data: &InputModel) -> Vec<VapourSynthFilter> {
    match data {
        InputModel::VapourSynth { filters, .. } | InputModel::VapourSynthScript { filters, .. } => {
            filters.clone()
        },
        InputModel::Video { .. } => Vec::new(),
    }
}

/// How a VapourSynth import method calls its source plugin.
pub(super) struct SourceCall {
    namespace: &'static str,
    function: &'static str,
    cache_argument: Option<&'static str>,
    track_argument: Option<&'static str>,
    track: Option<u8>,
    /// How many concurrent readers one instance serves without seeking back
    /// and forth between their positions.
    pub(super) readers_per_instance: usize,
}

impl SourceCall {
    /// Describes how `import_method` invokes its plugin.
    fn new(import_method: &VapourSynthImportMethod) -> Self {
        match import_method {
            VapourSynthImportMethod::LSMASHWorks { index } => Self {
                namespace: "lsmas",
                function: "LWLibavSource",
                cache_argument: Some("cachefile"),
                track_argument: Some("stream_index"),
                track: *index,
                readers_per_instance: 1,
            },
            VapourSynthImportMethod::FFMS2 { index } => Self {
                namespace: "ffms2",
                function: "Source",
                cache_argument: Some("cachefile"),
                track_argument: Some("track"),
                track: *index,
                readers_per_instance: 1,
            },
            VapourSynthImportMethod::BestSource { index } => Self {
                namespace: "bs",
                function: "VideoSource",
                cache_argument: Some("cachepath"),
                track_argument: Some("track"),
                track: *index,
                // BestSource keeps up to 4 decoders (`maxdecoders`)
                readers_per_instance: 4,
            },
            VapourSynthImportMethod::DGDecNV { .. } => Self {
                namespace: "dgdecodenv",
                function: "DGSource",
                // DGSource opens the generated .dgi itself, not the media plus a cache kwarg.
                cache_argument: None,
                track_argument: None,
                track: None,
                readers_per_instance: 1,
            },
        }
    }

    /// Keep paths and options as script variables rather than interpreting them as code.
    fn script(
        &self,
        source: &std::path::Path,
        cache_path: Option<&std::path::Path>,
    ) -> Result<(String, HashMap<String, String>)> {
        let mut script = String::from("from vapoursynth import core\nkwargs = {}\n");
        let mut variables =
            HashMap::from([("condor_source".to_owned(), source.display().to_string())]);
        if let (Some(argument), Some(cache_path)) = (self.cache_argument, cache_path) {
            writeln!(script, "kwargs[\"{argument}\"] = condor_cache")?;
            variables.insert(
                "condor_cache".to_owned(),
                std::path::absolute(cache_path)?.display().to_string(),
            );
        }
        if let (Some(track_argument), Some(track)) = (self.track_argument, self.track) {
            writeln!(script, "kwargs[\"{track_argument}\"] = int(condor_track)")?;
            variables.insert("condor_track".to_owned(), track.to_string());
        }
        writeln!(
            script,
            "core.{}.{}(source=condor_source, **kwargs).set_output(0)",
            self.namespace, self.function
        )?;
        Ok((script, variables))
    }

    /// Creates a new instance of the source with the VapourSynth API.
    #[inline]
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
        if let (Some(argument), Some(cache)) = (self.cache_argument, cache) {
            arguments.set_data(argument, cache.as_bytes())?;
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

/// DGSource consumes its index as `source`; the other plugins consume the media.
fn source_path(
    path: &std::path::Path,
    import_method: &VapourSynthImportMethod,
    cache_path: Option<&std::path::Path>,
) -> Result<std::path::PathBuf> {
    match import_method {
        VapourSynthImportMethod::DGDecNV {
            dgindexnv_executable,
        } => DGSource::index_video(
            path,
            cache_path,
            dgindexnv_executable.as_deref(),
            None,
            None,
        ),
        _ => Ok(std::path::absolute(path)?),
    }
}

#[cfg(all(test, target_os = "linux"))]
#[path = "vapoursynth_i13_tests.rs"]
pub(crate) mod i13_tests;

/// Translate the input's explicit source options, leaving absent options to the plugin.
fn source_script(
    path: &std::path::Path,
    import_method: &VapourSynthImportMethod,
    cache_path: Option<&std::path::Path>,
) -> Result<(Option<Imports>, Vec<Line>)> {
    let source = source_path(path, import_method, cache_path)?;
    let cache = cache_path.map(std::path::absolute).transpose()?;
    match import_method {
        VapourSynthImportMethod::FFMS2 { index } => Source {
            track: index.map(i32::from),
            cachefile: cache,
            ..Source::new(&source)
        }
        .generate_script(SCRIPT_NODE_NAME.to_owned()),
        VapourSynthImportMethod::BestSource { index } => VideoSource {
            track: index.map(i32::from),
            cachepath: cache,
            ..VideoSource::new(&source)
        }
        .generate_script(SCRIPT_NODE_NAME.to_owned()),
        VapourSynthImportMethod::LSMASHWorks { index } => LWLibavSource {
            stream_index: index.map(i32::from),
            cachefile: cache,
            ..LWLibavSource::new(&source)
        }
        .generate_script(SCRIPT_NODE_NAME.to_owned()),
        VapourSynthImportMethod::DGDecNV { .. } => {
            DGSource::new(&source).generate_script(SCRIPT_NODE_NAME.to_owned())
        },
    }
}

/// Chains a caller's [`ModifyNode`] and an input's own filters into one, the
/// caller's first so the input's filters act on its output.
fn compose_modifier(
    modify_node: Option<ModifyNode>,
    filters: Vec<VapourSynthFilter>,
) -> ModifyNode {
    Box::new(move |core, node| {
        let mut node = match &modify_node {
            Some(modify_node) => modify_node(core, node)?,
            None => node.ok_or_else(|| DecoderError::VapoursynthInternalError {
                cause: "VapourSynth output node does not exist".to_owned(),
            })?,
        };
        for filter in &filters {
            node = filter.invoke_plugin_function(core, &node).map_err(|e| {
                DecoderError::VapoursynthInternalError {
                    cause: e.to_string(),
                }
            })?;
        }
        Ok(node)
    })
}

/// Whether any of `data`'s filters needs a plugin `invoke_plugin_function`
/// cannot reach, and must therefore be baked into a generated script.
#[inline]
fn has_script_only_filter(data: &InputModel) -> bool {
    match data {
        InputModel::VapourSynth { filters, .. } | InputModel::VapourSynthScript { filters, .. } => {
            filters.iter().any(VapourSynthFilter::is_script_only)
        },
        InputModel::Video { .. } => false,
    }
}

/// Pre-indexes `data`'s source ahead of opening it, reporting progress.
///
/// Only DGDecNV indexes out of process: it spawns `dgindexnv`, whose stdout
/// carries something countable. Every other import method indexes lazily
/// inside its own source plugin, where no callback of ours can reach it, so
/// nothing is reported here and the caller stays indeterminate.
pub fn index_source(
    data: &InputModel,
    mut progress: impl FnMut(OpenProgress),
    cancelled: Option<&AtomicBool>,
) -> Result<()> {
    let InputModel::VapourSynth {
        path,
        import_method,
        cache_path,
        ..
    } = data
    else {
        return Ok(());
    };
    let VapourSynthImportMethod::DGDecNV {
        dgindexnv_executable,
    } = import_method
    else {
        return Ok(());
    };
    // Before spawning: a missing source must fail with the error
    // `from_vapoursynth` would give, not after a pointless `dgindexnv` run.
    Input::validate(data)?;

    // `index_video` reports the start itself, so an existing `.dgi` never
    // claims to be indexing.
    DGSource::index_video(
        path,
        cache_path.as_deref(),
        dgindexnv_executable.as_deref(),
        Some(&mut |current, total| {
            progress(OpenProgress::Indexing { current, total });
        }),
        cancelled,
    )?;
    Ok(())
}

pub fn index_source_cancel(
    data: &InputModel,
    mut progress: impl FnMut(OpenProgress),
    cancellation: &crate::utils::cancellation::Cancellation,
) -> Result<()> {
    if let InputModel::VapourSynth {
        path,
        import_method:
            VapourSynthImportMethod::DGDecNV {
                dgindexnv_executable,
            },
        cache_path,
        ..
    } = data
    {
        Input::validate(data)?;
        DGSource::index_video_cancel(
            path,
            cache_path.as_deref(),
            dgindexnv_executable.as_deref(),
            Some(&mut |current, total| progress(OpenProgress::Indexing { current, total })),
            cancellation,
        )?;
    }
    Ok(())
}

/// Opens a VapourSynth input, chaining its filters onto the source node.
pub fn from_vapoursynth(data: &InputModel, modify_node: Option<ModifyNode>) -> Result<Input> {
    Input::validate(data)?;

    // `VapoursynthDecoder::new()` panics without the library, and the core API
    // is reached through VSScript, so this is the same test that
    // constructor does.
    get_api().map_err(|_| {
        anyhow::anyhow!(
            "VapourSynth is not available; it is required for VapourSynth inputs and the \
             VapourSynth metric path"
        )
    })?;

    if has_script_only_filter(data) {
        // Baking filters into a script rebuilds from the source node, so a
        // caller's modifier cannot be applied to it.
        if modify_node.is_some() {
            bail!(
                "a caller-supplied ModifyNode cannot be combined with a script-only filter, \
                 because the script is rebuilt from the source"
            );
        }
        return from_vapoursynth_scripted(data);
    }

    // The input's own filters run after any caller-supplied modifier.
    let filters = filters_of(data);
    let modifier = match (modify_node, filters.is_empty()) {
        (None, true) => None,
        (modify_node, _) => Some(compose_modifier(modify_node, filters.clone())),
    };

    match data {
        InputModel::VapourSynth {
            path,
            import_method,
            cache_path,
            ..
        } => {
            let source = source_path(path, import_method, cache_path.as_deref())?;
            let call = SourceCall::new(import_method);

            // Keep the source in the script: each new reader or metadata query
            // applies `ModifyNode`, which must not reopen/reindex the source.
            let (script, variables) = call.script(&source, cache_path.as_deref())?;

            Ok(Input::VapourSynth {
                path: path.clone(),
                import_method: import_method.clone(),
                cache_path: cache_path.clone(),
                filters,
                decoder: source_decoder(
                    VapoursynthDecoder::from_script(&script, variables, Some(0))?,
                    modifier,
                )?,
                clip_info: None,
            })
        },
        InputModel::VapourSynthScript {
            source,
            variables,
            index,
            stream_concurrently,
            ..
        } => {
            let vs_decoder = match source {
                VapourSynthScriptSource::Path(path) => {
                    VapoursynthDecoder::from_file(path, variables.clone(), Some(*index))?
                },
                VapourSynthScriptSource::Text(script) => {
                    VapoursynthDecoder::from_script(script, variables.clone(), Some(*index))?
                },
            };
            Ok(Input::VapourSynthScript {
                source: source.clone(),
                variables: variables.clone(),
                index: *index,
                filters,
                stream_concurrently: *stream_concurrently,
                decoder: source_decoder(vs_decoder, modifier)?,
                clip_info: None,
            })
        },
        _ => panic!("expected `Input::VapourSynth` or `Input::VapourSynthScript`"),
    }
}

/// Registers `modifier` on `vs_decoder` and wraps it for [`Input`] to hold.
fn source_decoder(
    mut vs_decoder: VapoursynthDecoder,
    modifier: Option<ModifyNode>,
) -> Result<Decoder> {
    if let Some(modifier) = modifier {
        vs_decoder.register_node_modifier(modifier)?;
    }
    Ok(Decoder::from_decoder_impl(
        av_decoders::DecoderImpl::Vapoursynth(vs_decoder),
    )?)
}

/// Adds a source node's lines, then one node's worth of script per filter.
fn add_source_and_filters<'a>(
    script: &mut VapourSynthScript,
    source_imports: Option<Imports>,
    source_lines: Vec<Line>,
    filters: impl Iterator<Item = &'a VapourSynthFilter>,
) -> Result<()> {
    if let Some(source_imports) = source_imports {
        script.add_imports(source_imports);
    }
    script.add_lines(source_lines);

    for filter in filters {
        let (import_lines, filter_lines) = filter.generate_script(SCRIPT_NODE_NAME.to_owned())?;
        if let Some(import_lines) = import_lines {
            script.add_imports(import_lines);
        }
        script.add_lines(filter_lines);
    }
    Ok(())
}

/// Builds a script that imports the source and bakes `data`'s filters into it,
/// for filters needing script-only plugins (such as `Rescale`).
pub fn from_vapoursynth_scripted(data: &InputModel) -> Result<Input> {
    const SCRIPT_OUTPUT_INDEX: u8 = 0;

    match data {
        InputModel::VapourSynth {
            path,
            import_method,
            cache_path,
            filters,
            ..
        } => {
            let mut script = VapourSynthScript::default();
            let (source_imports, source_lines) =
                source_script(path, import_method, cache_path.as_deref())?;
            add_source_and_filters(&mut script, source_imports, source_lines, filters.iter())?;
            script.outputs.insert(SCRIPT_OUTPUT_INDEX, SCRIPT_NODE_NAME.to_owned());

            from_vapoursynth(
                &InputModel::VapourSynthScript {
                    source: VapourSynthScriptSource::Text(script.to_string()),
                    variables: HashMap::new(),
                    index: SCRIPT_OUTPUT_INDEX,
                    // Already baked into the script.
                    filters: Vec::new(),
                    // The generated script decodes with the same source plugin
                    // as the input it wraps, so it keeps whatever that input
                    // was allowed to do.
                    stream_concurrently: streams_concurrently(import_method),
                },
                None,
            )
        },
        // A user script's output node has no name this code can reference, so a
        // script-only filter cannot be appended to it.
        InputModel::VapourSynthScript { filters, .. } => {
            if let Some(filter) = filters.iter().find(|f| f.is_script_only()) {
                bail!(
                    "{filter} is script-only and cannot be applied to a VapourSynth script input; \
                     put it in the script instead"
                );
            }
            from_vapoursynth(data, None)
        },
        InputModel::Video { .. } => bail!("scripted VapourSynth input expected"),
    }
}

/// Reopens a natively-decoded input through VapourSynth, translating its
/// native filters so both paths convert the clip the same way. Already-
/// VapourSynth inputs return `None`.
pub fn as_vapoursynth_script(input: &mut Input) -> Result<Option<Input>> {
    const SCRIPT_OUTPUT_INDEX: u8 = 0;

    let Input::Video {
        path,
        import_method,
        filters,
        ..
    } = input
    else {
        return Ok(None);
    };

    let ImportMethod::FFMS2 { index, cache_path } = import_method;
    let (source_imports, source_lines) = source_script(
        path,
        &VapourSynthImportMethod::FFMS2 { index: *index },
        cache_path.as_deref(),
    )?;
    let translated = filters
        .iter()
        // Each deferred filter sees the preceding node's format, not the initial source.
        .flat_map(crate::core::input::Ffms2Filter::to_vapoursynth)
        .collect::<Vec<_>>();

    let mut script = VapourSynthScript::default();
    add_source_and_filters(&mut script, source_imports, source_lines, translated.iter())?;
    script.outputs.insert(SCRIPT_OUTPUT_INDEX, SCRIPT_NODE_NAME.to_owned());

    let script_input = InputModel::VapourSynthScript {
        source: VapourSynthScriptSource::Text(script.to_string()),
        variables: HashMap::new(),
        index: SCRIPT_OUTPUT_INDEX,
        filters: Vec::new(),
        // This wraps a native input, whose FFMS2 source is known to handle
        // several readers at once.
        stream_concurrently: true,
    };

    Ok(Some(from_vapoursynth(&script_input, None)?))
}

/// Describes a VapourSynth input's clip.
pub fn clip_info(decoder: &mut Decoder) -> Result<ClipInfo> {
    get_clip_info(&output_node(decoder)?)
}

/// The input's VapourSynth output node, with its filters applied.
pub fn output_node<'core>(decoder: &'core mut Decoder) -> Result<Node<'core>> {
    let vs = decoder.get_vapoursynth_impl().expect("Decoder is VapourSynth");
    Ok(vs.get_output(vs.get_output_index(), vs.get_node_modifier())?)
}

/// Whether several readers can share this input without slowing each other
/// down. False for DGDecNV, which is unmeasured.
#[inline]
#[must_use]
pub fn streams_concurrently(import_method: &VapourSynthImportMethod) -> bool {
    !matches!(import_method, VapourSynthImportMethod::DGDecNV { .. })
}

/// Creates `count` source nodes, each applying the input's filters. Sources
/// share a plugin instance until it has served `readers_per_instance` readers,
/// since an instance decoding from several positions seeks back and forth.
pub fn frame_sources<'core>(
    count: usize,
    path: &std::path::Path,
    cache_path: Option<&std::path::Path>,
    import_method: &VapourSynthImportMethod,
    decoder: &'core mut Decoder,
) -> Result<Vec<Node<'core>>> {
    let call = SourceCall::new(import_method);
    let vs = decoder.get_vapoursynth_impl().expect("Decoder is VapourSynth");
    let core = get_core(&vs.env)?;
    let source = source_path(path, import_method, cache_path)?.display().to_string();
    let cache = cache_path
        .map(std::path::absolute)
        .transpose()?
        .map(|cache| cache.display().to_string());

    let mut sources = Vec::with_capacity(count);
    let mut instance = None;
    for index in 0..count {
        let node = if index < call.readers_per_instance {
            // The input's own source instance
            vs.get_output(vs.get_output_index(), vs.get_node_modifier())?
        } else {
            if index % call.readers_per_instance == 0 {
                instance = Some(call.invoke(core, &source, cache.as_deref())?);
            }
            let node = instance.clone().expect("source instance exists");
            match vs.get_node_modifier() {
                Some(modify_node) => modify_node(core, Some(node))?,
                None => node,
            }
        };
        sources.push(node);
    }
    Ok(sources)
}

/// Serialises existing integer Gray or YUV planes, excluding stride padding.
fn write_y4m_frame(frame: &FrameRef<'_>) -> Result<Vec<u8>> {
    let format = frame.format();
    anyhow::ensure!(
        matches!(format.color_family(), ColorFamily::Gray | ColorFamily::YUV)
            && format.sample_type() == SampleType::Integer,
        DecoderError::UnsupportedFormat {
            fmt: format!(
                "{} {} for Y4M; convert to integer Gray or YUV before encoding",
                format.color_family(),
                format.sample_type()
            ),
        }
    );
    let mut data = Vec::new();

    for plane_index in 0..format.plane_count() {
        if let Ok(plane_data) = frame.data(plane_index) {
            data.extend_from_slice(plane_data);
        } else {
            for row in 0..frame.height(plane_index) {
                data.extend_from_slice(frame.data_row(plane_index, row));
            }
        }
    }
    Ok(data)
}

/// Writes one frame at `index` as a complete y4m frame.
pub fn y4m_frame(node: &Node<'_>, index: usize, stream: &mut Vec<u8>) -> Result<()> {
    let frame = node.get_frame(index).with_context(|| format!("get y4m frame {index}"))?;
    let data = write_y4m_frame(&frame).with_context(|| format!("serialize y4m frame {index}"))?;
    stream.extend_from_slice(FRAME_HEADER);
    stream.extend_from_slice(&data);
    Ok(())
}

/// Streams `frame_indices` from `node` as Y4M frames, in order, while keeping
/// at most `window` frames requested but not yet sent.
pub fn y4m_frames(
    node: &Node<'_>,
    frame_sender: &crossbeam_channel::Sender<std::io::Cursor<Vec<u8>>>,
    frame_indices: &[usize],
    window: usize,
) -> Result<()> {
    y4m_frames_cancel(
        node,
        frame_sender,
        frame_indices,
        window,
        &crate::utils::cancellation::Cancellation::default(),
    )
}

pub fn y4m_frames_cancel(
    node: &Node<'_>,
    frame_sender: &crossbeam_channel::Sender<std::io::Cursor<Vec<u8>>>,
    frame_indices: &[usize],
    window: usize,
    cancellation: &crate::utils::cancellation::Cancellation,
) -> Result<()> {
    let state = Arc::new((Mutex::new((BTreeMap::new(), 0_usize)), Condvar::new()));
    let mut next_request = 0;
    let mut next_send = 0;

    let result = (|| -> Result<()> {
        while next_send < frame_indices.len() {
            cancellation.check_forced()?;
            // Requests run ahead of sends, so frames decode in parallel without
            // all sitting resident at once.
            while next_request < frame_indices.len() && next_request - next_send < window {
                cancellation.check_forced()?;
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
                let mut pending = lock.lock().expect("mutex should acquire lock");
                while !pending.0.contains_key(&next_send) {
                    cancellation.check_forced()?;
                    pending = condvar
                        .wait_timeout(pending, std::time::Duration::from_millis(10))
                        .expect("Condvar should be notified")
                        .0;
                }
                pending.0.remove(&next_send).expect("Map should have frame")
            };
            let frame = frame.map_err(|error| {
                anyhow::anyhow!("get y4m frame {}: {error}", frame_indices[next_send])
            })?;

            let payload = write_y4m_frame(&frame)
                .with_context(|| format!("serialize y4m frame {}", frame_indices[next_send]))?;
            let mut data = Vec::new();
            data.extend_from_slice(FRAME_HEADER);
            data.extend_from_slice(&payload);
            drop(frame);

            cancellation.send(frame_sender, std::io::Cursor::new(data))?;
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

#[cfg(test)]
#[path = "vapoursynth_i05_tests.rs"]
mod i05_tests;

#[cfg(test)]
#[path = "vapoursynth_i06_tests.rs"]
mod i06_tests;

#[cfg(test)]
mod tests {
    use super::*;

    fn i03_input(script: &str) -> Input {
        Input::from_data(&InputModel::VapourSynthScript {
            source: VapourSynthScriptSource::Text(script.to_owned()),
            variables: HashMap::new(),
            index: 0,
            filters: Vec::new(),
            stream_concurrently: true,
        })
        .expect("I03 requires the installed native VapourSynth runtime")
    }

    fn i03_assert_writers(input: &mut Input, payloads: &[Vec<u8>], streaming: bool) {
        let expected: Vec<Vec<u8>> = payloads
            .iter()
            .map(|payload| [b"FRAME\n".as_slice(), payload].concat())
            .collect();
        if streaming {
            let node = output_node(input.decoder()).expect("output node");
            let (sender, receiver) = crossbeam_channel::bounded(payloads.len());
            // One request also keeps the pre-fix panic reproduction free of outstanding callbacks.
            y4m_frames(&node, &sender, &[0, 1], 1).expect("stream frames");
            drop(sender);
            let actual: Vec<_> = receiver.iter().map(std::io::Cursor::into_inner).collect();
            assert_eq!(actual, expected);
        } else {
            let mut actual = b"retained output\n".to_vec();
            let node = output_node(input.decoder()).expect("output node");
            for index in 0..payloads.len() {
                y4m_frame(&node, index, &mut actual).expect("single frame");
            }
            assert_eq!(
                actual,
                [b"retained output\n".as_slice(), &expected.concat()].concat()
            );
        }
    }

    fn i03_gray(bit_depth: u8, streaming: bool) {
        let (colors, payloads) = if bit_depth == 8 {
            ([37, 219], [vec![37; 54], vec![219; 54]])
        } else {
            (
                [4660, 43981],
                [0x1234_u16.to_ne_bytes().repeat(54), 0xabcd_u16.to_ne_bytes().repeat(54)],
            )
        };
        let mut input = i03_input(&format!(
            "import vapoursynth as vs\nc = vs.core\n\
             a = c.std.BlankClip(width=18, height=3, length=1, format=vs.GRAY{bit_depth}, color=[{}])\n\
             b = c.std.BlankClip(a, color=[{}])\n(a + b).set_output()",
            colors[0], colors[1]
        ));
        {
            let node = output_node(input.decoder()).expect("gray output node");
            let frame = node.get_frame(0).expect("native gray frame");
            assert_eq!(frame.format().plane_count(), 1);
            let visible = 18 * usize::from(bit_depth / 8);
            eprintln!(
                "I03 GRAY{bit_depth}: stride={}, visible={visible}",
                frame.stride(0)
            );
            if frame.stride(0) != visible {
                assert!(frame.data(0).is_err(), "padded plane must use visible rows");
            }
        }
        i03_assert_writers(&mut input, &payloads, streaming);
    }

    #[test]
    fn i03_gray8_single_exact_payload() {
        i03_gray(8, false);
    }

    #[test]
    fn i03_gray8_bulk_exact_payload() {
        i03_gray(8, true);
    }

    #[test]
    fn i03_gray16_single_exact_payload() {
        i03_gray(16, false);
    }

    #[test]
    fn i03_gray16_bulk_exact_payload() {
        i03_gray(16, true);
    }

    #[test]
    fn i03_padded_yuv_keeps_visible_planes_in_order() {
        let mut input = i03_input(
            "import vapoursynth as vs\nc = vs.core\n\
             a = c.std.BlankClip(width=18, height=3, length=1, format=vs.YUV422P8, color=[17, 83, 201])\n\
             b = c.std.BlankClip(a, color=[231, 41, 159])\n(a + b).set_output()",
        );
        {
            let node = output_node(input.decoder()).expect("YUV output node");
            let frame = node.get_frame(0).expect("native YUV frame");
            assert_eq!(frame.format().plane_count(), 3);
            for (plane, visible) in [18, 9, 9].into_iter().enumerate() {
                eprintln!(
                    "I03 YUV plane {plane}: stride={}, visible={visible}",
                    frame.stride(plane)
                );
                if frame.stride(plane) != visible {
                    assert!(
                        frame.data(plane).is_err(),
                        "padded plane must use visible rows"
                    );
                }
            }
        }
        let payloads = [
            [vec![17; 54], vec![83; 27], vec![201; 27]].concat(),
            [vec![231; 54], vec![41; 27], vec![159; 27]].concat(),
        ];
        i03_assert_writers(&mut input, &payloads, false);
        i03_assert_writers(&mut input, &payloads, true);
    }

    #[test]
    fn i03_rgb_bulk_rejects_before_publication() {
        let decoder = VapoursynthDecoder::from_script(
            "import vapoursynth as vs\nvs.core.std.BlankClip(width=18, height=3, length=2, format=vs.RGB24, color=[17, 83, 201]).set_output()",
            HashMap::new(),
            None,
        ).expect("native RGB fixture");
        let node = decoder.get_output(0, None).expect("actual RGB node");
        assert_eq!(
            node.info().format.color_family(),
            vapoursynth::format::ColorFamily::RGB
        );
        let frame = node.get_frame(1).expect("actual RGB frame");
        let error = write_y4m_frame(&frame).expect_err("RGB planes cannot be serialized as YUV");
        assert!(
            matches!(
                error.downcast_ref::<DecoderError>(),
                Some(DecoderError::UnsupportedFormat { .. })
            ),
            "{error:#}"
        );
        assert!(
            error.to_string().contains("convert to integer Gray or YUV"),
            "{error:#}"
        );
        let (sender, receiver) = crossbeam_channel::bounded(2);
        let error = y4m_frames(&node, &sender, &[1, 0], 2).expect_err("RGB is not YUV");
        assert!(
            matches!(
                error.downcast_ref::<DecoderError>(),
                Some(DecoderError::UnsupportedFormat { .. })
            ),
            "{error:#}"
        );
        assert!(error.to_string().contains("frame 1"), "{error:#}");
        assert!(
            receiver.try_recv().is_err(),
            "rejected RGB must publish no frame"
        );
    }

    #[test]
    fn i03_rgb_input_rejected_at_open() {
        let data = InputModel::VapourSynthScript {
            source: VapourSynthScriptSource::Text(
                "import vapoursynth as vs\nvs.core.std.BlankClip(format=vs.RGB24).set_output()"
                    .to_owned(),
            ),
            variables: HashMap::new(),
            index: 0,
            filters: Vec::new(),
            stream_concurrently: true,
        };
        let error = Input::from_data(&data).err().expect("RGB input must fail to open");
        assert!(
            matches!(
                error.downcast_ref::<DecoderError>(),
                Some(DecoderError::UnsupportedFormat { .. })
            ),
            "{error:#}"
        );
    }

    #[test]
    fn i03_single_rejected_live_output_preserves_stream() {
        let mut input = i03_input(
            "import vapoursynth as vs\nvs.core.std.BlankClip(width=18, height=3, length=2, format=vs.GRAY8).set_output()",
        );
        // Opening already rejects RGB. A live script replacement exercises the later frame guard
        // without disguising RGB planes as YUV: both the real node and frame report RGB.
        input.decoder().get_vapoursynth_impl().expect("VS decoder").env.eval_script(
            "vs.core.std.BlankClip(width=18, height=3, length=2, format=vs.RGB24, color=[17, 83, 201]).set_output()",
        ).expect("replace live output");
        {
            let node = output_node(input.decoder()).expect("live RGB output");
            assert_eq!(
                node.info().format.color_family(),
                vapoursynth::format::ColorFamily::RGB
            );
        }
        let mut stream = b"retained output\n".to_vec();
        let node = output_node(input.decoder()).expect("live RGB output");
        let error = y4m_frame(&node, 1, &mut stream).expect_err("reject live RGB output");
        assert!(
            matches!(
                error.downcast_ref::<DecoderError>(),
                Some(DecoderError::UnsupportedFormat { .. })
            ),
            "{error:#}"
        );
        assert!(error.to_string().contains("frame 1"), "{error:#}");
        assert_eq!(
            stream, b"retained output\n",
            "no stray FRAME header or planes"
        );
    }

    #[test]
    fn i03_float_bulk_rejects_before_publication() {
        let decoder = VapoursynthDecoder::from_script(
            "import vapoursynth as vs\nvs.core.std.BlankClip(width=18, height=3, length=2, format=vs.YUV444PS, color=[0.25, 0.5, 0.75]).set_output()",
            HashMap::new(),
            None,
        ).expect("native float fixture");
        let node = decoder.get_output(0, None).expect("actual float node");
        let (sender, receiver) = crossbeam_channel::bounded(2);
        let error = y4m_frames(&node, &sender, &[1, 0], 2).expect_err("float is not integer Y4M");
        assert!(
            matches!(
                error.downcast_ref::<DecoderError>(),
                Some(DecoderError::UnsupportedFormat { .. })
            ),
            "{error:#}"
        );
        assert!(
            receiver.try_recv().is_err(),
            "rejected float must publish no frame"
        );
    }

    /// `VapoursynthDecoder::new()` panics rather than erroring when the library
    /// is missing, so `from_vapoursynth` checks availability first:
    /// constructing a script input must come back as an error instead of
    /// aborting the process.
    #[test]
    fn script_input_without_vapoursynth_errors_instead_of_panicking() {
        let available = get_api().is_ok();
        let data = InputModel::VapourSynthScript {
            source: VapourSynthScriptSource::Text(
                "from vapoursynth import core
core.std.BlankClip().set_output()"
                    .to_owned(),
            ),
            variables: HashMap::new(),
            index: 0,
            filters: Vec::new(),
            stream_concurrently: false,
        };

        let result = Input::from_data(&data);
        if !available {
            let Err(error) = result else {
                panic!("a VapourSynth input cannot be constructed without VapourSynth");
            };
            assert!(
                error.to_string().contains("VapourSynth is not available"),
                "{error}"
            );
        }
    }
}
