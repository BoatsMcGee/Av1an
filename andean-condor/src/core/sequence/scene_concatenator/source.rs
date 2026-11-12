//! Admission for original mux sources; encoded chunks remain owned by their existing boundary.
use super::*;
use serde::Deserialize;
use serde_json::Value;
use std::{collections::BTreeMap, io::Read, process::ExitStatus};

pub(super) struct PreparedSource {
    pub path: Option<PathBuf>,
    // Keep the private namespace alive until the final mux has stopped reading it.
    _scratch: Option<tempfile::TempDir>,
}

fn alternative(reason: impl std::fmt::Display) -> anyhow::Error {
    anyhow::anyhow!("{reason}; use FFmpeg concat method for this source/configuration")
}

/// Both pipes are bounded and drained concurrently. Even on overflow/read failure,
/// kill/reap and join both readers before returning (including forced cancellation).
fn capture(
    command: &mut Command,
    cancellation: &Cancellation,
    allow_warning: bool,
) -> Result<(ExitStatus, Vec<u8>)> {
    fn read(mut pipe: impl Read, limit: usize) -> Result<Vec<u8>> {
        let mut bytes = Vec::new();
        let mut buffer = [0; 8192];
        loop {
            let count = pipe.read(&mut buffer)?;
            if count == 0 {
                return Ok(bytes);
            }
            anyhow::ensure!(
                bytes.len() + count <= limit,
                "source probe/remux capture exceeded {limit} bytes"
            );
            bytes.extend_from_slice(&buffer[..count]);
        }
    }
    command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = OwnedProcess::spawn(command, cancellation)
        .with_context(|| format!("Running source preflight {command:?}"))?;
    let stdout = child.stdout.take().context("source process missing stdout")?;
    let stderr = child.stderr.take().context("source process missing stderr")?;
    let (status, stdout, stderr) = thread::scope(|scope| -> Result<_> {
        let (tx, rx) = sync::mpsc::channel();
        for (index, pipe, limit) in [
            (0, Box::new(stdout) as Box<dyn Read + Send>, 4 * 1024 * 1024),
            (1, Box::new(stderr) as Box<dyn Read + Send>, 64 * 1024),
        ] {
            let tx = tx.clone();
            scope.spawn(move || {
                let _ = tx.send((index, read(pipe, limit)));
            });
        }
        drop(tx);
        let mut outputs = [None, None];
        let mut killed = Ok(());
        let mut monitored = Ok(());
        while outputs.iter().any(Option::is_none) {
            match rx.recv_timeout(std::time::Duration::from_millis(10)) {
                Ok((index, result)) => {
                    if result.is_err() {
                        killed = child.kill();
                    }
                    outputs[index] = Some(result);
                },
                Err(sync::mpsc::RecvTimeoutError::Timeout) => {},
                Err(sync::mpsc::RecvTimeoutError::Disconnected) => break,
            }
            // A leader can exit while descendants retain the pipes. Reap through
            // the owner now, so its group cleanup can unblock both reader joins.
            if monitored.is_ok()
                && let Err(error) = child.try_wait()
            {
                monitored = Err(error);
                killed = child.kill();
            }
        }
        let status = child.wait();
        cancellation.check_forced()?;
        killed?;
        monitored?;
        Ok((
            status?,
            outputs[0].take().context("source stdout reader panicked")??,
            outputs[1].take().context("source stderr reader panicked")??,
        ))
    })?;
    // Status 1 is interpreted only by identify, never by ffprobe/FFmpeg.
    if !(status.success() || (allow_warning && status.code() == Some(1))) {
        return Err(alternative(format!(
            "Source preflight {command:?} failed ({status}): {}",
            String::from_utf8_lossy(&stderr)
        )));
    }
    Ok((status, stdout))
}

#[derive(Deserialize)]
struct Identify {
    container: Container,
    #[serde(default)]
    errors: Vec<String>,
    #[serde(default)]
    tracks: Vec<IdentifiedTrack>,
    #[serde(default)]
    attachments: Vec<Value>,
}
#[derive(Deserialize)]
struct IdentifiedTrack {
    #[serde(rename = "type")]
    kind: String,
}
#[derive(Deserialize)]
struct Container {
    recognized: bool,
    supported: bool,
}

impl Identify {
    fn supported(&self) -> bool {
        self.container.recognized && self.container.supported
    }
}

fn identify(path: &Path, cancellation: &Cancellation) -> Result<Identify> {
    let (_, bytes) = capture(
        Command::new("mkvmerge")
            .args(["--identify", "--identification-format", "json"])
            .arg(super::mkvmerge::fix_path(path)),
        cancellation,
        true,
    )?;
    let identified: Identify = serde_json::from_slice(&bytes)
        .with_context(|| format!("Malformed mkvmerge identification for {}", path.display()))?;
    anyhow::ensure!(
        identified.errors.is_empty(),
        "mkvmerge source identification errors for {}: {:?}",
        path.display(),
        identified.errors
    );
    Ok(identified)
}

#[derive(Deserialize)]
struct Probe {
    streams: Vec<Stream>,
    format: Format,
    chapters: Vec<Value>,
}
#[derive(Deserialize)]
struct Format {
    #[serde(default)]
    tags: BTreeMap<String, String>,
}
#[derive(Deserialize)]
struct Stream {
    index: u32,
    codec_type: String,
    codec_name: Option<String>,
    #[serde(default)]
    tags: BTreeMap<String, String>,
    #[serde(default)]
    disposition: BTreeMap<String, u32>,
    #[serde(flatten)]
    properties: BTreeMap<String, Value>,
}

fn probe(path: &Path, cancellation: &Cancellation) -> Result<Probe> {
    let (status, bytes) = capture(
        Command::new("ffprobe")
            .args([
                "-v",
                "error",
                "-show_streams",
                "-show_format",
                "-show_chapters",
                "-show_data_hash",
                "sha256",
                "-of",
                "json",
            ])
            .arg(path),
        cancellation,
        false,
    )?;
    anyhow::ensure!(
        status.success(),
        "ffprobe failed for {}; use FFmpeg concat method",
        path.display()
    );
    serde_json::from_slice(&bytes)
        .with_context(|| format!("Malformed ffprobe source report for {}", path.display()))
}

fn tags_preserved(before: &BTreeMap<String, String>, after: &BTreeMap<String, String>) -> bool {
    let after: BTreeMap<_, _> =
        after.iter().map(|(key, value)| (key.to_lowercase(), value)).collect();
    before.iter().all(|(key, value)| after.get(&key.to_lowercase()) == Some(&value))
}

fn chapters_preserved(before: &[Value], after: &[Value]) -> bool {
    before.len() == after.len()
        && before.iter().zip(after).all(|(a, b)| {
            ["start_time", "end_time"].iter().all(|key| {
                let time = |value: &Value| value[*key].as_str().and_then(|s| s.parse::<f64>().ok());
                matches!((time(a), time(b)), (Some(a), Some(b)) if (a - b).abs() < 0.000_001)
            }) && a
                .get("tags")
                .into_iter()
                .flat_map(|tags| tags.as_object().into_iter().flatten())
                .all(|(key, value)| {
                    b["tags"].as_object().is_some_and(|tags| {
                        tags.iter().any(|(k, v)| k.eq_ignore_ascii_case(key) && v == value)
                    })
                })
        })
}

/// Recognized inputs keep their native IDs/options. Unsupported primary inputs can
/// copy all ancillary streams, but have no mkvmerge ID namespace to safely remap.
pub(super) fn prepare(
    input: Option<&Path>,
    config: Option<&MkvmergeConfig>,
    scratch: &Path,
    cancellation: &Cancellation,
) -> Result<PreparedSource> {
    for extra in config.into_iter().flat_map(|config| &config.extra_inputs) {
        if !identify(&extra.path, cancellation)?.supported() {
            return Err(alternative(format!(
                "Unsupported extra input {} may include video or explicit IDs; cannot safely adapt its full content",
                extra.path.display()
            )));
        }
    }
    let mut prepared = PreparedSource {
        path: input.map(Path::to_path_buf),
        _scratch: None,
    };
    let Some(input) = input else {
        return Ok(prepared);
    };
    if identify(input, cancellation)?.supported() {
        return Ok(prepared);
    }
    if config.and_then(|config| config.tracks.as_ref()).is_some_and(|tracks| {
        tracks.values().any(|track| {
            !matches!(
                track,
                crate::models::sequence::scene_concatenator::mkvmerge::MkvmergeTrack::Video(_)
            )
        })
    }) {
        return Err(alternative(
            "Unsupported source has no provable mkvmerge track/attachment ID mapping",
        ));
    }
    let mut original = probe(input, cancellation)?;
    // The container writer changes on remux; track encoder tags and all other
    // source tags remain preservation requirements, not ignorable diagnostics.
    original.format.tags.retain(|key, _| !key.eq_ignore_ascii_case("encoder"));
    anyhow::ensure!(
        !original.streams.is_empty(),
        "Source probe contains no streams; use FFmpeg concat method"
    );
    let mut indices = std::collections::HashSet::new();
    for stream in &original.streams {
        if !indices.insert(stream.index)
            || !matches!(
                stream.codec_type.as_str(),
                "video" | "audio" | "subtitle" | "attachment"
            )
            || stream.disposition.get("attached_pic") == Some(&1)
        {
            return Err(alternative(
                "Unsupported source stream identity/type or cover art cannot be preserved",
            ));
        }
    }
    let ancillary: Vec<_> =
        original.streams.iter().filter(|stream| stream.codec_type != "video").collect();
    let copy_metadata = config
        .and_then(|config| config.metadata.as_ref())
        .is_none_or(|metadata| metadata.copy);
    let copy_chapters = config
        .and_then(|config| config.chapters.as_ref())
        .is_none_or(|chapters| chapters.copy);
    if ancillary.is_empty() {
        if (copy_metadata && !original.format.tags.is_empty())
            || (copy_chapters && !original.chapters.is_empty())
        {
            return Err(alternative(
                "Unsupported metadata/chapter-only source cannot be carried without an ancillary track",
            ));
        }
        prepared.path = None;
        return Ok(prepared);
    }
    std::fs::create_dir_all(scratch)?;
    let directory = tempfile::Builder::new().prefix(".condor-source-").tempdir_in(scratch)?;
    let path = directory.path().join("ancillary.mkv");
    let mut command = Command::new("ffmpeg");
    command.args(["-v", "error", "-nostdin", "-n", "-i"]).arg(input);
    for stream in &ancillary {
        command.arg("-map").arg(format!("0:{}", stream.index));
    }
    command
        .args([
            "-c",
            "copy",
            "-map_metadata:g",
            if copy_metadata { "0" } else { "-1" },
            "-map_chapters",
            if copy_chapters { "0" } else { "-1" },
            "-f",
            "matroska",
        ])
        .arg(&path);
    let (status, _) = capture(&mut command, cancellation, false)?;
    if !status.success() || !std::fs::metadata(&path).is_ok_and(|metadata| metadata.len() > 0) {
        return Err(alternative(
            "Ancillary lossless Matroska remux failed or produced an empty/unrecognized source",
        ));
    }
    let identified = identify(&path, cancellation)?;
    if !identified.supported()
        || identified.attachments.len()
            != ancillary.iter().filter(|stream| stream.codec_type == "attachment").count()
        || identified.tracks.len()
            != ancillary.iter().filter(|stream| stream.codec_type != "attachment").count()
        || ["audio", "subtitle"].iter().any(|kind| {
            identified
                .tracks
                .iter()
                .filter(|track| {
                    track.kind
                        == if *kind == "subtitle" {
                            "subtitles"
                        } else {
                            kind
                        }
                })
                .count()
                != ancillary.iter().filter(|stream| stream.codec_type == *kind).count()
        })
    {
        return Err(alternative(
            "mkvmerge cannot identify every remuxed ancillary track/attachment",
        ));
    }
    let adapted = probe(&path, cancellation)?;
    let preserved = ancillary.len() == adapted.streams.len()
        && ancillary.iter().zip(&adapted.streams).all(|(a, b)| {
            // Generic attachments have no codec_name; their complete bytes are extradata.
            (a.codec_name.is_some()
                || (a.codec_type == "attachment" && a.properties.contains_key("extradata_hash")))
                && a.codec_name == b.codec_name
                && a.codec_type == b.codec_type
                && tags_preserved(&a.tags, &b.tags)
                && a.disposition == b.disposition
                && [
                    "extradata_hash",
                    "sample_rate",
                    "channels",
                    "channel_layout",
                    "bits_per_sample",
                    "width",
                    "height",
                ]
                .iter()
                .all(|key| a.properties.get(*key) == b.properties.get(*key))
        })
        && (!copy_metadata || tags_preserved(&original.format.tags, &adapted.format.tags))
        && (!copy_chapters || chapters_preserved(&original.chapters, &adapted.chapters));
    if !preserved {
        return Err(alternative(
            "Ancillary remux did not verifiably preserve codecs, track metadata, attachments or chapters",
        ));
    }
    prepared.path = Some(path);
    prepared._scratch = Some(directory);
    Ok(prepared)
}
