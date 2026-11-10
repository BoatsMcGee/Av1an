use super::*;
use std::{fs, os::unix::fs::PermissionsExt};

const HEADER: &str = "YUV4MPEG2 C420 W64 H64 F30000:1001 Ip A0:0 XLENGTH=3\n";

// Catch the missing final pass at the process boundary, not by inspecting the loop.
// The fake consumes real Y4M and requires pass one's statistics before writing output.
const FAKE: &str = r#"#!/bin/sh
set -eu
argv=$(printf '%s\n' "$@")
pass=1 total=1 stats= output= records=
while [ "$#" -gt 0 ]; do
    case "$1" in
        --first-pass) pass=1; total=2; stats=$2; shift ;;
        --second-pass) pass=2; total=2; stats=$2; shift ;;
        --output) output=$2; shift ;;
        --e07-records) records=$2; shift ;;
    esac
    shift
done
printf '%s/%s\n' "$pass" "$total" >> "$records/order"
printf '%s\n' "$argv" > "$records/pass$pass.argv"
cat > "$records/pass$pass.y4m"
[ ! -e "$records/scene0.ivf" ]
if [ "$pass/$total" = 1/2 ]; then
    [ "$output" = /dev/null ]
    printf first > "$stats"
    printf 'E07 first 1/2\n'
else
    [ "$output" = "$records/scene0.ivf" ]
    if [ "$total" = 2 ]; then
        [ "$(cat "$stats")" = first ]
    fi
    printf final > "$output"
    printf 'E07 final %s/%s\n' "$pass" "$total"
fi
"#;

#[test]
fn e07_public_input_and_stream_execute_each_pass_once() {
    bounded("fake");
}

#[test]
fn e07_installed_rav1e_public_two_pass_jobs_decode_exactly_three_frames() {
    bounded("native");
}

fn bounded(case: &str) {
    let temp = tempfile::tempdir().expect("fixture");
    let root = temp.path();
    fs::write(root.join("watchdog.rs"), include_str!("e08_fixture.rs")).expect("watchdog source");
    e02_tests::run_bounded(
        Command::new("rustc")
            .arg(root.join("watchdog.rs"))
            .arg("-o")
            .arg(root.join("watchdog")),
        &root.join("rustc.log"),
    );
    let cwd = root.join("cwd");
    fs::create_dir_all(&cwd).expect("isolated cwd");
    fs::write(cwd.join("scene0.stat"), "unrelated CWD sentinel").expect("sentinel");
    let log = e02_tests::run_bounded(
        Command::new(root.join("watchdog"))
            .arg("--e08-watchdog")
            .arg(std::env::current_exe().expect("test binary"))
            .arg(root)
            .arg(case)
            .arg("core::encoder::e07_tests::e07_runtime_child")
            .current_dir(&cwd),
        &root.join("child.log"),
    );
    assert!(
        log.lines()
            .any(|line| line == "E08 watchdog reaped 0 descendants; timeout=false")
    );
    assert_eq!(fs::read_dir(&cwd).expect("cwd listing").count(), 1);
    assert_eq!(
        fs::read_to_string(cwd.join("scene0.stat")).expect("sentinel"),
        "unrelated CWD sentinel"
    );
    eprint!("{log}");
}

pub(super) fn value<'a>(argv: &'a [String], key: &str) -> &'a str {
    &argv
        .windows(2)
        .find(|args| args[0] == key)
        .unwrap_or_else(|| panic!("missing {key}: {argv:?}"))[1]
}

pub(super) fn check_input(input: &mut Input) -> ClipInfo {
    let info = input.clip_info().expect("clip info");
    assert_eq!(info.num_frames, 5);
    assert_eq!(
        (*info.frame_rate.numer(), *info.frame_rate.denom()),
        (30000, 1001)
    );
    assert_eq!(input.y4m_header(Some(3)).expect("range header"), HEADER);
    info
}

pub(super) fn expected_y4m() -> Vec<u8> {
    let mut bytes = HEADER.as_bytes().to_vec();
    for luma in [48, 80, 112] {
        bytes.extend_from_slice(b"FRAME\n");
        bytes.extend(vec![luma; 64 * 64]);
        bytes.extend(vec![128; 64 * 64 / 2]);
    }
    bytes
}

pub(super) fn frames(input: &mut Input) -> crossbeam_channel::Receiver<Cursor<Vec<u8>>> {
    let (tx, rx) = crossbeam_channel::unbounded();
    let header = input.y4m_header(Some(3)).expect("stream header");
    let mut bytes = header.as_bytes().to_vec();
    tx.send(Cursor::new(header.into_bytes())).expect("header");
    let mut reader = input.frame_reader().expect("frame reader");
    for index in 1..4 {
        let frame = reader.y4m_frame(index).expect("stream frame");
        bytes.extend_from_slice(frame.get_ref());
        tx.send(frame).expect("frame");
    }
    assert_eq!(bytes, expected_y4m(), "nonzero-offset stream fixture");
    drop(tx);
    rx
}

#[test]
fn e07_runtime_child() {
    let Some(root) = std::env::var_os("E08_ROOT") else {
        return;
    };
    let root = PathBuf::from(root);
    let native = std::env::var("E08_CASE").expect("case") == "native";
    let executable = if native {
        which::which("rav1e").expect("installed rav1e is required for E07")
    } else {
        let path = root.join("fake encoder");
        fs::write(&path, FAKE).expect("fake");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("executable");
        path
    };
    if native {
        let version = e02_tests::run_bounded(
            Command::new(&executable).arg("--version"),
            &root.join("version.log"),
        );
        eprintln!("E07 installed {}: {version}", executable.display());
    }
    let script = root.join("five frames.vpy");
    fs::write(&script, "from vapoursynth import core, YUV420P8\ncore.std.Splice([core.std.BlankClip(width=64,height=64,length=1,format=YUV420P8,fpsnum=30000,fpsden=1001,color=[y,128,128]) for y in [16,48,80,112,144]]).set_output()\n").expect("finite VS input");
    let model = crate::models::input::Input::VapourSynthScript {
        source: crate::models::input::VapourSynthScriptSource::Path(script),
        variables: HashMap::new(),
        index: 0,
        filters: vec![],
        stream_concurrently: false,
    };
    let mut attempts = Vec::new();
    for total in if native { &[2][..] } else { &[1, 2][..] } {
        for api in ["input", "stream"] {
            let job = root.join(format!("{api}{total} space 映像"));
            fs::create_dir_all(&job).expect("independent job");
            let output = job.join("scene0.ivf");
            let mut encoder = Encoder::default_from_base(&EncoderBase::RAV1E, true);
            encoder.set_executable(Some(executable.clone()));
            encoder.set_append_color_data(false);
            *encoder.passes_mut().expect("passes") = EncoderPasses::All(*total);
            encoder.parameters_mut().extend(CLIParameter::new_numbers(
                "--",
                " ",
                &[("speed", 10.0), ("threads", 1.0), ("bitrate", 100.0)],
            ));
            if !native {
                encoder.parameters_mut().insert(
                    "e07-records".into(),
                    CLIParameter::new_string("--", " ", job.to_str().expect("utf8")),
                );
            }
            let mut input = Input::from_data(&model).expect("actual input");
            let info = check_input(&mut input);
            let (progress, rx) = mpsc::channel();
            drop(rx);
            let result = if api == "input" {
                encoder.encode_with_input(&mut input, &output, 1, 4, progress)
            } else {
                encoder.encode_with_stream(frames(&mut input), &output, progress, &info)
            }
            .expect("successful public orchestration");
            assert!(result.status.success(), "{result}");
            check_input(&mut input);
            if !native {
                assert_eq!(
                    fs::read_to_string(job.join("order")).expect("pass order"),
                    if *total == 1 { "1/1\n" } else { "1/2\n2/2\n" },
                    "{api} All({total}) must execute each pass exactly once"
                );
                for pass in 1..=*total {
                    assert_eq!(
                        fs::read(job.join(format!("pass{pass}.y4m"))).expect("consumed Y4M"),
                        expected_y4m(),
                        "{api} pass {pass}: identical header, range, and bytes"
                    );
                    let argv: Vec<_> = fs::read_to_string(job.join(format!("pass{pass}.argv")))
                        .expect("actual argv")
                        .lines()
                        .map(str::to_owned)
                        .collect();
                    if pass == *total {
                        assert_eq!(argv, result.parameters, "return the final pass result");
                    } else {
                        assert_eq!(value(&argv, "--output"), NULL_OUTPUT);
                        assert_eq!(
                            value(&argv, "--first-pass"),
                            value(&result.parameters, "--second-pass")
                        );
                    }
                }
                assert_eq!(fs::read_to_string(&output).expect("final output"), "final");
                assert_eq!(
                    result.stdout.to_string(),
                    format!("E07 final {total}/{total}\n")
                );
                eprintln!(
                    "E07 {api} All({total}): exact order {}, final marker, identical 3-frame Y4M",
                    if *total == 1 {
                        "[(1,1)]"
                    } else {
                        "[(1,2),(2,2)]"
                    }
                );
            } else {
                let probe = e02_tests::run_bounded(
                    Command::new(which::which("ffprobe").expect("installed ffprobe required"))
                        .args([
                            "-v",
                            "error",
                            "-select_streams",
                            "v:0",
                            "-count_frames",
                            "-show_entries",
                            "stream=codec_name,r_frame_rate,nb_read_frames",
                            "-of",
                            "default=noprint_wrappers=1",
                        ])
                        .arg(&output)
                        .stdin(Stdio::null()),
                    &root.join(format!("{api}-probe.log")),
                );
                assert_eq!(
                    probe.trim(),
                    "codec_name=av1\nr_frame_rate=30000/1001\nnb_read_frames=3"
                );
                assert_eq!(
                    fs::read_dir(&job).expect("job listing").count(),
                    1,
                    "only final output remains"
                );
                eprintln!(
                    "E07 {api} public All(2) final argv: {:?}\n{probe}",
                    result.parameters
                );
            }
            assert_eq!(
                value(&result.parameters, "--output"),
                output.to_str().expect("utf8")
            );
            if *total == 2 {
                let stats = PathBuf::from(value(&result.parameters, "--second-pass"));
                assert_eq!(stats.file_name().expect("stats basename"), "scene0.stat");
                assert_eq!(
                    stats.parent().expect("private directory").parent(),
                    Some(job.as_path())
                );
                assert!(
                    !stats.parent().expect("private directory").exists(),
                    "owned cleanup"
                );
                assert!(
                    !attempts.contains(&stats),
                    "independent API statistics contexts"
                );
                attempts.push(stats);
            }
        }
    }
}
