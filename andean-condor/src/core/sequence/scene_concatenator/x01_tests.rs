use super::*;

fn run(command: &mut Command) -> Vec<u8> {
    let output = command.output().expect("required native fixture tool");
    assert!(
        output.status.success(),
        "{command:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}

fn chunks(root: &Path) -> Vec<PathBuf> {
    ["black", "white"]
        .into_iter()
        .enumerate()
        .map(|(index, color)| {
            let path = root.join(format!("chunk {index} 'quoted'.mkv"));
            run(Command::new("ffmpeg")
                .args([
                    "-v",
                    "error",
                    "-f",
                    "lavfi",
                    "-i",
                    &format!("color={color}:size=32x16:rate=24"),
                    "-frames:v",
                    "2",
                    "-c:v",
                    "ffv1",
                ])
                .arg(&path));
            path
        })
        .collect()
}

fn mux(
    root: &Path,
    source: &Path,
    chunks: &[PathBuf],
    config: Option<&MkvmergeConfig>,
) -> Result<()> {
    let (tx, _rx) = sync::mpsc::channel();
    SceneConcatenator::mkvmerge_cancel(
        root,
        &root.join("out.mkv"),
        chunks,
        Some(source),
        Ratio::new(24, 1),
        config,
        &tx,
        &Cancellation::default(),
    )
}

fn probe(path: &Path) -> serde_json::Value {
    serde_json::from_slice(&run(Command::new("ffprobe")
        .args(["-v", "error", "-count_frames", "-show_streams", "-show_format", "-of", "json"])
        .arg(path)))
    .expect("native JSON")
}

#[test]
fn x01_native_y4m_omits_unsupported_root_preserves_video_order() {
    let dir = tempfile::tempdir().expect("fixture");
    let source = dir.path().join("source 'with spaces'.y4m");
    run(Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "color=red:size=32x16:rate=24",
            "-frames:v",
            "4",
        ])
        .arg(&source));
    let original = std::fs::read(&source).expect("source");
    mux(dir.path(), &source, &chunks(dir.path()), None).expect("Y4M must not reach mkvmerge");
    let output = dir.path().join("out.mkv");
    let data = probe(&output);
    assert_eq!(data["streams"].as_array().expect("streams").len(), 1);
    assert_eq!(data["streams"][0]["nb_read_frames"], "4");
    let pixels = run(Command::new("ffmpeg")
        .args(["-v", "error", "-i"])
        .arg(&output)
        .args(["-f", "rawvideo", "-pix_fmt", "gray", "-"]));
    assert_eq!(pixels.len(), 32 * 16 * 4);
    assert!(pixels[..32 * 16 * 2].iter().all(|&pixel| pixel < 5));
    assert!(pixels[32 * 16 * 2..].iter().all(|&pixel| pixel > 250));
    assert_eq!(std::fs::read(source).expect("unchanged root"), original);
}

#[test]
fn x01_native_wmv_audio_is_losslessly_adapted_and_scratch_is_owned() {
    let dir = tempfile::tempdir().expect("fixture");
    let source = dir.path().join("source 'audio'.wmv");
    run(Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "color=size=32x16:rate=24",
            "-f",
            "lavfi",
            "-i",
            "sine=sample_rate=48000",
            "-t",
            "0.2",
            "-c:v",
            "wmv2",
            "-c:a",
            "wmav2",
            "-metadata",
            "title=X01 preserved title",
            "-metadata:s:a:0",
            "language=eng",
        ])
        .arg(&source));
    let original = std::fs::read(&source).expect("source");
    let scratch = SceneConcatenator::scratch_directory(dir.path());
    std::fs::create_dir_all(&scratch).expect("scratch");
    std::fs::write(scratch.join("source.mkv"), b"caller scratch sentinel").expect("sentinel");
    mux(dir.path(), &source, &chunks(dir.path()), None).expect("native ancillary adaptation");
    let output = dir.path().join("out.mkv");
    let data = probe(&output);
    let streams = data["streams"].as_array().expect("streams");
    let audio = streams
        .iter()
        .find(|stream| stream["codec_type"] == "audio")
        .expect("preserved audio");
    assert_eq!(audio["codec_name"], "wmav2");
    assert_eq!(audio["tags"]["language"], "eng");
    assert_eq!(data["format"]["tags"]["title"], "X01 preserved title");
    let packet_hashes = |path: &Path| {
        let data: serde_json::Value = serde_json::from_slice(&run(Command::new("ffprobe")
            .args([
                "-v",
                "error",
                "-select_streams",
                "a",
                "-show_packets",
                "-show_data_hash",
                "sha256",
                "-of",
                "json",
            ])
            .arg(path)))
        .expect("packet hashes");
        data["packets"]
            .as_array()
            .expect("packets")
            .iter()
            .map(|packet| packet["data_hash"].clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(
        packet_hashes(&source),
        packet_hashes(&output),
        "compressed audio bytes unchanged"
    );
    assert_eq!(std::fs::read(source).expect("source intact"), original);
    assert_eq!(
        std::fs::read(scratch.join("source.mkv")).expect("caller sentinel"),
        b"caller scratch sentinel"
    );
    assert!(
        !std::fs::read_dir(scratch).expect("scratch listing").any(|entry| entry
            .expect("entry")
            .file_name()
            .to_string_lossy()
            .starts_with(".condor-source-"))
    );
    let config = serde_json::from_str(r#"{"tracks":{"1":{"type":"audio","language":"jpn"}}}"#)
        .expect("original ID settings");
    std::fs::write(&output, b"mapping refusal sentinel").expect("old output");
    let error = mux(
        dir.path(),
        &dir.path().join("source 'audio'.wmv"),
        &[],
        Some(&config),
    )
    .expect_err("source index 1 cannot be reused as remux mkvmerge ID 0");
    assert!(format!("{error:#}").contains("no provable mkvmerge"));
    assert_eq!(
        std::fs::read(output).expect("output sentinel"),
        b"mapping refusal sentinel"
    );
}

#[test]
fn x01_native_known_mkv_mp4_keep_ids_selection_and_track_options() {
    for extension in ["mkv", "mp4"] {
        let dir = tempfile::tempdir().expect("fixture");
        let source = dir.path().join(format!("known source.{extension}"));
        run(Command::new("ffmpeg")
            .args([
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "color=size=32x16:rate=24",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:sample_rate=48000",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=880:sample_rate=48000",
                "-map",
                "0:v",
                "-map",
                "1:a",
                "-map",
                "2:a",
                "-t",
                "0.2",
                "-c:v",
                "mpeg4",
                "-c:a",
                "aac",
            ])
            .arg(&source));
        let config = serde_json::from_str(r#"{"tracks":{"1":{"type":"audio","copy":false},"2":{"type":"audio","language":"jpn","name":"selected track"}}}"#).expect("settings");
        let result = mux(dir.path(), &source, &chunks(dir.path()), Some(&config));
        if extension == "mp4" && result.is_err() {
            // X02 owns final exit-1 policy. Native v103 warns about the excluded
            // valid ID 1; the produced track selection must still remain correct.
            let error = result.expect_err("native warning");
            let Some(SceneConcatenatorError::MkvmergeFailed { status, stdout, .. }) =
                error.downcast_ref()
            else {
                panic!("{error:#}")
            };
            assert_eq!(status.code(), Some(1));
            assert!(
                stdout.contains("ID 1 was requested but not found"),
                "{stdout}"
            );
        } else {
            result.expect("recognized source retains IDs");
        }
        let data = probe(&dir.path().join("out.mkv"));
        let audio = data["streams"]
            .as_array()
            .expect("streams")
            .iter()
            .filter(|stream| stream["codec_type"] == "audio")
            .collect::<Vec<_>>();
        assert_eq!(audio.len(), 1);
        assert_eq!(audio[0]["tags"]["language"], "jpn");
        assert_eq!(audio[0]["tags"]["title"], "selected track");
    }
}

#[test]
fn x01_unsupported_ids_and_extra_inputs_refuse_before_output_mutation() {
    let dir = tempfile::tempdir().expect("fixture");
    let source = dir.path().join("root.y4m");
    run(Command::new("ffmpeg")
        .args(["-v", "error", "-f", "lavfi", "-i", "color=size=32x16:rate=24", "-frames:v", "2"])
        .arg(&source));
    let original = std::fs::read(&source).expect("root");
    let chunks = chunks(dir.path());
    for config in [
        serde_json::json!({"tracks":{"1":{"type":"audio","copy":true}}}),
        serde_json::json!({"extra_inputs":[{"path":source}]}),
    ] {
        std::fs::write(dir.path().join("out.mkv"), b"existing output sentinel").expect("sentinel");
        let config = serde_json::from_value(config).expect("config");
        let error = mux(dir.path(), &source, &chunks, Some(&config))
            .expect_err("no provable source ID namespace");
        assert!(
            format!("{error:#}").contains("use FFmpeg concat method"),
            "{error:#}"
        );
        assert_eq!(
            std::fs::read(dir.path().join("out.mkv")).expect("old output"),
            b"existing output sentinel"
        );
        assert_eq!(std::fs::read(&source).expect("root intact"), original);
    }
}

#[cfg(unix)]
#[test]
fn x01_isolated_preflight_errors_cancellation_and_preservation() {
    use std::os::unix::fs::PermissionsExt;
    let python = which::which("python3").expect("required Python fixture tool");
    let ffmpeg = which::which("ffmpeg").expect("required FFmpeg");
    let ffprobe = which::which("ffprobe").expect("required FFprobe");
    let mkvmerge = which::which("mkvmerge").expect("required mkvmerge");
    let fixtures = tempfile::tempdir().expect("native fixtures");
    let scenes = chunks(fixtures.path());
    let source = fixtures.path().join("source.mkv");
    std::fs::write(
        fixtures.path().join("sub.srt"),
        "1\n00:00:00,000 --> 00:00:00,100\nX01 subtitle\n",
    )
    .expect("subtitle");
    std::fs::write(
        fixtures.path().join("attachment.txt"),
        b"X01 attachment identity",
    )
    .expect("attachment");
    std::fs::write(fixtures.path().join("metadata.txt"), ";FFMETADATA1\ntitle=X01 title\n[CHAPTER]\nTIMEBASE=1/1000\nSTART=0\nEND=100\ntitle=X01 chapter\n").expect("chapters");
    run(Command::new(&ffmpeg)
        .args([
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "color=size=32x16:rate=24",
            "-f",
            "lavfi",
            "-i",
            "sine=sample_rate=48000",
            "-i",
        ])
        .arg(fixtures.path().join("sub.srt"))
        .arg("-i")
        .arg(fixtures.path().join("metadata.txt"))
        .args([
            "-map",
            "0:v",
            "-map",
            "1:a",
            "-map",
            "2:s",
            "-map_metadata",
            "3",
            "-map_chapters",
            "3",
            "-t",
            "0.2",
            "-c:v",
            "ffv1",
            "-c:a",
            "pcm_s16le",
            "-c:s",
            "srt",
            "-metadata:s:a:0",
            "language=eng",
            "-attach",
        ])
        .arg(fixtures.path().join("attachment.txt"))
        .args(["-metadata:s:t:0", "mimetype=text/plain"])
        .arg(&source));
    for case in [
        "adapt-content",
        "leader-exit",
        "identify-warning",
        "identify-errors",
        "identify-status",
        "malformed-identify",
        "missing-identify",
        "probe-failure",
        "malformed-probe",
        "probe-overflow",
        "stderr-overflow",
        "missing-probe",
        "metadata-only",
        "chapter-only",
        "unsupported-type",
        "empty-remux",
        "missing-remux",
        "invalid-remux",
        "track-loss",
        "codec-refusal",
        "metadata-loss",
        "force-probe",
        "force-remux",
    ] {
        let dir = tempfile::tempdir().expect("isolated case");
        let root = dir.path();
        std::fs::copy(&source, root.join("source 'quoted'.mkv")).expect("source");
        let bin = root.join("bin");
        std::fs::create_dir_all(&bin).expect("private bin");
        let script = format!(
            r#"#!{}
import os,sys,json,subprocess,time
from pathlib import Path
r=Path(os.environ['X01_ROOT']); case=os.environ['X01_CASE']
tool=Path(sys.argv[0]).name; args=sys.argv[1:]
real=os.environ['X01_REAL_'+tool.upper()]
source=str(r/"source 'quoted'.mkv")
def hold():
 p=subprocess.Popen([sys.executable,'-c','import time; time.sleep(30)'])
 (r/'pids').write_text(str(os.getpid())+' '+str(p.pid))
 (r/'active').write_text('ready')
 while True: time.sleep(0.01)
if tool=='mkvmerge' and '--identify' in args and args[-1]==source:
 if case=='malformed-identify': print('{{'); sys.exit(0)
 if case=='identify-status': print('{{"container":{{"recognized":false,"supported":false}},"errors":[]}}'); sys.exit(2)
 if case=='identify-errors': print('{{"container":{{"recognized":true,"supported":true}},"errors":["fixture error"]}}'); sys.exit(1)
 if case=='identify-warning':
  result=subprocess.run([real,*args],capture_output=True); sys.stdout.buffer.write(result.stdout); sys.exit(1)
 print('{{"container":{{"recognized":false,"supported":false}},"errors":[]}}'); sys.exit(0)
if tool=='ffprobe' and args[-1]==source:
 if case=='leader-exit':
  data=subprocess.check_output([real,*args])
  p=subprocess.Popen([sys.executable,'-c','import time; time.sleep(6)'])
  (r/'pids').write_text(str(os.getpid())+' '+str(p.pid))
  sys.stdout.buffer.write(data); sys.exit(0)
 if case=='force-probe': hold()
 if case=='probe-failure': sys.stderr.write('fixture probe failure'); sys.exit(1)
 if case=='malformed-probe': print('{{}}'); sys.exit(0)
 if case=='probe-overflow': sys.stdout.write(' '*(4*1024*1024+1)); sys.stdout.flush(); hold()
 if case=='stderr-overflow': sys.stderr.write('x'*(64*1024+1)); sys.stderr.flush(); hold()
 if case in ('metadata-only','chapter-only','unsupported-type'):
  data=json.loads(subprocess.check_output([real,*args]))
  if case=='unsupported-type': data['streams'][1]['codec_type']='data'
  else: data['streams']=data['streams'][:1]
  if case=='chapter-only': data['format']['tags']={{}}
  print(json.dumps(data)); sys.exit(0)
if tool=='ffmpeg':
 if case=='force-remux': Path(args[-1]).write_bytes(b'partial source'); hold()
 if case=='empty-remux': Path(args[-1]).write_bytes(b''); sys.exit(0)
 if case=='missing-remux': sys.exit(0)
 if case=='invalid-remux': Path(args[-1]).write_bytes(b'not a Matroska file'); sys.exit(0)
 if case=='track-loss': args=args[:-1]+['-map','-0:2',args[-1]]
 if case=='codec-refusal': sys.stderr.write('codec cannot be copied into Matroska'); sys.exit(1)
 if case=='metadata-loss': args=args[:-1]+['-metadata','title=lost',args[-1]]
os.execv(real,[real,*args])
"#,
            python.display()
        );
        for tool in ["ffmpeg", "ffprobe", "mkvmerge"] {
            if (case == "missing-probe" && tool == "ffprobe")
                || (case == "missing-identify" && tool == "mkvmerge")
            {
                continue;
            }
            let path = bin.join(tool);
            std::fs::write(&path, &script).expect("fixture executable");
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
                .expect("executable");
        }
        let result = Command::new(std::env::current_exe().expect("test binary"))
            .args([
                "--exact",
                "core::sequence::scene_concatenator::x01_tests::x01_preflight_child",
                "--nocapture",
            ])
            .env("X01_ROOT", root)
            .env("X01_CASE", case)
            .env("X01_SCENE0", &scenes[0])
            .env("X01_SCENE1", &scenes[1])
            .env("X01_REAL_FFMPEG", &ffmpeg)
            .env("X01_REAL_FFPROBE", &ffprobe)
            .env("X01_REAL_MKVMERGE", &mkvmerge)
            .env("PATH", bin)
            .output()
            .expect("isolated owning backend");
        assert!(
            result.status.success(),
            "{case}: {}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        eprintln!(
            "X01 isolated {case}: {}",
            String::from_utf8_lossy(&result.stdout)
        );
    }
}

#[cfg(unix)]
#[test]
fn x01_preflight_child() {
    let Some(root) = std::env::var_os("X01_ROOT").map(PathBuf::from) else {
        return;
    };
    let case = std::env::var("X01_CASE").expect("case");
    let source = root.join("source 'quoted'.mkv");
    let original = std::fs::read(&source).expect("source");
    let scenes = ["X01_SCENE0", "X01_SCENE1"]
        .map(|key| PathBuf::from(std::env::var_os(key).expect("scene")));
    let output = root.join("out.mkv");
    std::fs::write(&output, b"old output sentinel").expect("old output");
    let token = Cancellation::default();
    let controller = case.starts_with("force-").then(|| {
        let root = root.clone();
        let token = token.clone();
        thread::spawn(move || {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while !root.join("active").exists() && std::time::Instant::now() < deadline {
                thread::yield_now();
            }
            let active = root.join("active").exists();
            token.request().expect("graceful");
            token.request().expect("force");
            assert!(active, "active owned process handshake");
        })
    });
    let (tx, _rx) = sync::mpsc::channel();
    let started = std::time::Instant::now();
    let result = SceneConcatenator::mkvmerge_cancel(
        &root,
        &output,
        &scenes,
        Some(&source),
        Ratio::new(24, 1),
        None,
        &tx,
        &token,
    );
    if let Some(controller) = controller {
        controller.join().expect("joined force controller");
    }
    if case == "leader-exit" {
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "exited leader must release descendant-held pipes without waiting for its sleep"
        );
    }
    if case == "adapt-content" || case == "identify-warning" || case == "leader-exit" {
        result.expect("real FFmpeg copy and native final mkvmerge");
        let data = probe(&output);
        assert_eq!(data["format"]["tags"]["title"], "X01 title");
        assert_eq!(data["streams"].as_array().expect("streams").len(), 4);
        let chapters: serde_json::Value = serde_json::from_slice(&run(Command::new("ffprobe")
            .args(["-v", "error", "-show_chapters", "-of", "json"])
            .arg(&output)))
        .expect("chapters");
        assert_eq!(chapters["chapters"][0]["tags"]["title"], "X01 chapter");
        let attachment = run(Command::new("ffmpeg")
            .args(["-v", "error", "-dump_attachment:t:0", "pipe:1", "-i"])
            .arg(&output)
            .args(["-f", "null", "-"]));
        assert_eq!(attachment, b"X01 attachment identity");
    } else {
        let error = result.expect_err("preflight must fail before final mux");
        let text = format!("{error:#}");
        match case.as_str() {
            "force-probe" | "force-remux" => assert_eq!(
                error.downcast_ref::<crate::utils::cancellation::Cancelled>(),
                Some(&crate::utils::cancellation::Cancelled::Forced)
            ),
            "probe-overflow" | "stderr-overflow" => {
                assert!(text.contains("capture exceeded"), "{text}");
            },
            "codec-refusal" => assert!(
                text.contains("codec cannot be copied")
                    && text.contains("use FFmpeg concat method"),
                "{text}"
            ),
            other => {
                let expected = match other {
                    "identify-errors" => "identification errors",
                    "identify-status" => "failed (exit status: 2)",
                    "malformed-identify" => "Malformed mkvmerge identification",
                    "missing-identify" | "missing-probe" => "Running source preflight",
                    "probe-failure" => "fixture probe failure",
                    "malformed-probe" => "Malformed ffprobe source report",
                    "metadata-only" | "chapter-only" => "metadata/chapter-only source",
                    "unsupported-type" => "stream identity/type",
                    "empty-remux" | "missing-remux" => "empty/unrecognized source",
                    "invalid-remux" | "track-loss" => "cannot identify every remuxed",
                    "metadata-loss" => "did not verifiably preserve",
                    _ => panic!("unhandled case {other}"),
                };
                assert!(text.contains(expected), "{case}: {text}");
            },
        }
        assert_eq!(
            std::fs::read(output).expect("sentinel"),
            b"old output sentinel"
        );
    }
    assert_eq!(std::fs::read(source).expect("source intact"), original);
    let scratch = SceneConcatenator::scratch_directory(&root);
    assert!(
        !scratch.exists()
            || !std::fs::read_dir(scratch).expect("scratch listing").any(|entry| entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .starts_with(".condor-source-"))
    );
    #[cfg(target_os = "linux")]
    if root.join("pids").exists() {
        for pid in std::fs::read_to_string(root.join("pids"))
            .expect("owned PIDs")
            .split_whitespace()
        {
            let stat = std::fs::read_to_string(format!("/proc/{pid}/stat"));
            assert!(
                stat.is_err()
                    || stat
                        .expect("stat")
                        .split(')')
                        .nth(1)
                        .expect("state")
                        .trim_start()
                        .starts_with('Z'),
                "owned process {pid} remains live"
            );
        }
    }
    println!(
        "{case}: owning preflight, preservation/sentinels, readers/children and source scratch checked"
    );
}
