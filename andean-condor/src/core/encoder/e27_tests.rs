use super::{
    e07_tests::{check_input, expected_y4m, frames, value},
    *,
};
use std::{fs, os::unix::fs::PermissionsExt};

// Consume the whole finite input before failing: this isolates exit status from
// E08 writer errors. Distinct raw diagnostics identify the pass that actually failed.
const FAKE: &str = r#"#!/bin/sh
set -eu
argv=$(printf '%s\n' "$@")
pass=1 total=1 stats= output= records= fail= code=
while [ "$#" -gt 0 ]; do
    case "$1" in
        --first-pass) pass=1; total=2; stats=$2; shift ;;
        --second-pass) pass=2; total=2; stats=$2; shift ;;
        --output) output=$2; shift ;;
        --e27-records) records=$2; shift ;;
        --e27-fail-pass) fail=$2; shift ;;
        --e27-code) code=$2; shift ;;
    esac
    shift
done
printf '%s/%s\n' "$pass" "$total" >> "$records/order"
printf '%s\n' "$argv" > "$records/pass$pass.argv"
cat > "$records/pass$pass.y4m"
if [ "$pass/$total" = 1/2 ]; then
    [ "$output" = /dev/null ]
    printf first > "$stats"
elif [ "$total" = 2 ]; then
    [ "$(cat "$stats")" = first ]
fi
printf 'E27 stdout %s/%s\nbad\377\r\nstdout tail %s' "$pass" "$total" "$pass"
printf 'encoded 1 frames\nencoded 3 frames\rE27 stderr %s/%s\nbad\376\r\nstderr tail %s' "$pass" "$total" "$pass" >&2
if [ -n "$stats" ]; then
    [ -d "${stats%/*}" ]
    [ "$(cat "$stats")" = first ]
    printf stats_alive=true > "$records/pass$pass.complete"
else
    printf no_stats > "$records/pass$pass.complete"
fi
[ "$pass" != "$fail" ] || exit "$code"
"#;

#[test]
fn e27_bounded_public_failure_matrix() {
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
    let mut failures = Vec::new();
    for api in ["input", "stream"] {
        for case in ["first", "final", "single", "specific1", "specific2", "direct1", "direct2"] {
            let job = root.join(format!("{api}-{case} space 映像"));
            let cwd = job.join("cwd");
            fs::create_dir_all(&cwd).expect("independent job");
            fs::write(cwd.join("scene0.stat"), "CWD sentinel").expect("sentinel");
            let file = fs::File::create(job.join("child.log")).expect("log");
            let status = Command::new(root.join("watchdog"))
                .arg("--e08-watchdog")
                .arg(std::env::current_exe().expect("test binary"))
                .arg(&job)
                .arg(format!("{api}-{case}"))
                .arg("core::encoder::e27_tests::e27_runtime_child")
                .current_dir(&cwd)
                .stdout(file.try_clone().expect("clone log"))
                .stderr(file)
                .status()
                .expect("bounded child");
            let log = fs::read_to_string(job.join("child.log")).expect("child log");
            if !status.success()
                || !job.join("executed").exists()
                || !job.join("verified").exists()
                || !log.contains("test result: ok. 1 passed;")
                || !log
                    .lines()
                    .any(|line| line == "E08 watchdog reaped 0 descendants; timeout=false")
            {
                failures.push(format!("{api}-{case}: {status}\n{log}"));
            }
            assert_eq!(fs::read_dir(&cwd).expect("cwd listing").count(), 1);
            assert_eq!(
                fs::read_to_string(cwd.join("scene0.stat")).expect("sentinel"),
                "CWD sentinel"
            );
            eprint!("{log}");
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn e27_runtime_child() {
    let Some(root) = std::env::var_os("E08_ROOT") else {
        return;
    };
    let root = PathBuf::from(root);
    let case = std::env::var("E08_CASE").expect("case");
    let (api, case) = case.split_once('-').expect("api-case");
    let (passes, failing, code, order, executed): (_, _, _, _, &[u8]) = match case {
        "first" => (EncoderPasses::All(2), (1, 2), 42, "1/2\n", &[1]),
        "final" => (EncoderPasses::All(2), (2, 2), 43, "1/2\n2/2\n", &[1, 2]),
        "single" => (EncoderPasses::All(1), (1, 1), 44, "1/1\n", &[1]),
        "specific1" | "direct1" => (EncoderPasses::Specific(1, 2), (1, 2), 45, "1/2\n", &[1]),
        "specific2" | "direct2" => (EncoderPasses::Specific(2, 2), (2, 2), 46, "2/2\n", &[2]),
        _ => panic!("unknown case {case}"),
    };
    fs::write(root.join("executed"), format!("{api}-{case}")).expect("execution handshake");
    let executable = root.join("fake encoder");
    fs::write(&executable, FAKE).expect("fake");
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).expect("executable");
    let output = root.join("scene0.ivf");
    fs::write(&output, "preexisting final output").expect("sentinel");
    fs::write(root.join("unrelated.stat"), "unrelated stats").expect("sentinel");
    let mut encoder = Encoder::default_from_base(&EncoderBase::RAV1E, true);
    encoder.set_executable(Some(executable));
    encoder.set_append_color_data(false);
    *encoder.passes_mut().expect("passes") = passes;
    for (key, val) in [
        ("e27-records", root.to_str().expect("utf8").to_owned()),
        ("e27-fail-pass", failing.0.to_string()),
        ("e27-code", code.to_string()),
    ] {
        encoder
            .parameters_mut()
            .insert(key.into(), CLIParameter::new_string("--", " ", &val));
    }
    let direct = case.starts_with("direct");
    let stats = direct.then(|| PassStatistics::new(&output).expect("manual owned statistics"));
    if matches!(passes, EncoderPasses::Specific(2, 2)) {
        let base = stats
            .as_ref()
            .map_or_else(|| output.with_extension(""), |s| s.base().to_path_buf());
        fs::write(base.with_extension("stat"), "first").expect("manual pass one's stats");
    }
    let script = root.join("five frames.vpy");
    fs::write(&script, "from vapoursynth import core, YUV420P8\ncore.std.Splice([core.std.BlankClip(width=64,height=64,length=1,format=YUV420P8,fpsnum=30000,fpsden=1001,color=[y,128,128]) for y in [16,48,80,112,144]]).set_output()\n").expect("finite VS input");
    let mut input = Input::from_data(&crate::models::input::Input::VapourSynthScript {
        source: crate::models::input::VapourSynthScriptSource::Path(script),
        variables: HashMap::new(),
        index: 0,
        filters: vec![],
        stream_concurrently: false,
    })
    .expect("actual input");
    let info = check_input(&mut input);
    let (progress, rx) = mpsc::channel();
    let result = match (api, direct) {
        ("input", false) => encoder.encode_with_input(&mut input, &output, 1, 4, progress),
        ("input", true) => encoder.encode_pass_with_input(
            &mut input,
            &output,
            1,
            4,
            failing,
            progress,
            stats.as_ref(),
        ),
        ("stream", false) => {
            encoder.encode_with_stream(frames(&mut input), &output, progress, &info)
        },
        ("stream", true) => encoder.encode_pass_with_stream(
            failing,
            frames(&mut input),
            &output,
            progress,
            &info,
            stats.as_ref(),
        ),
        _ => panic!("unknown API {api}"),
    };
    assert_eq!(
        fs::read_to_string(root.join("order")).expect("pass order"),
        order,
        "no pass after failure"
    );
    let error = result.expect_err("nonzero pass status must be Err, never a prior/later success");
    let failed = error.downcast_ref::<EncoderResult>().expect("typed failing pass result");
    assert_eq!(failed.status.code(), Some(code));
    assert_eq!(failed.encoder, EncoderBase::RAV1E);
    let argv: Vec<_> = fs::read_to_string(root.join(format!("pass{}.argv", failing.0)))
        .expect("failing argv")
        .lines()
        .map(str::to_owned)
        .collect();
    assert_eq!(failed.parameters, argv);
    assert_eq!(
        value(&argv, "--output"),
        if failing.0 == failing.1 {
            output.to_str().expect("utf8")
        } else {
            NULL_OUTPUT
        }
    );
    let mut stdout = format!("E27 stdout {}/{}\nbad", failing.0, failing.1).into_bytes();
    stdout.extend_from_slice(b"\xff\r\nstdout tail ");
    stdout.extend_from_slice(failing.0.to_string().as_bytes());
    let mut stderr = format!(
        "encoded 1 frames\nencoded 3 frames\rE27 stderr {}/{}\nbad",
        failing.0, failing.1
    )
    .into_bytes();
    stderr.extend_from_slice(b"\xfe\r\nstderr tail ");
    stderr.extend_from_slice(failing.0.to_string().as_bytes());
    let StringOrBytes::Bytes(actual_stdout) = &failed.stdout else {
        panic!("raw stdout lost");
    };
    let StringOrBytes::Bytes(actual_stderr) = &failed.stderr else {
        panic!("raw stderr lost");
    };
    assert_eq!(actual_stdout, &stdout);
    assert_eq!(actual_stderr, &stderr);
    let mut expected_progress = Vec::new();
    let mut stats_path = None;
    for &pass in executed {
        assert_eq!(
            fs::read(root.join(format!("pass{pass}.y4m"))).expect("consumed input"),
            expected_y4m()
        );
        assert_eq!(
            fs::read_to_string(root.join(format!("pass{pass}.complete")))
                .expect("completion before API return"),
            if failing.1 == 2 {
                "stats_alive=true"
            } else {
                "no_stats"
            }
        );
        if api == "stream" && !direct {
            expected_progress.push(((pass, failing.1), 0));
        }
        expected_progress.extend([((pass, failing.1), 1), ((pass, failing.1), 3)]);
        if failing.1 == 2 {
            let argv: Vec<_> = fs::read_to_string(root.join(format!("pass{pass}.argv")))
                .expect("argv")
                .lines()
                .map(str::to_owned)
                .collect();
            let path = PathBuf::from(value(
                &argv,
                if pass == 1 {
                    "--first-pass"
                } else {
                    "--second-pass"
                },
            ));
            if let Some(previous) = &stats_path {
                assert_eq!(&path, previous, "same stats across passes");
            }
            stats_path = Some(path);
        }
    }
    assert_eq!(
        rx.try_iter().map(|p| (p.pass, p.frame)).collect::<Vec<_>>(),
        expected_progress,
        "all progress workers completed before return"
    );
    assert!(
        matches!(rx.try_recv(), Err(mpsc::TryRecvError::Disconnected)),
        "pass progress senders must already be dropped on return"
    );
    if let Some(path) = stats_path {
        if matches!(passes, EncoderPasses::All(_)) {
            assert!(
                !path.parent().expect("owned directory").exists(),
                "owned stats cleaned on error"
            );
        } else {
            assert_eq!(
                fs::read_to_string(&path).expect("manual stats retained"),
                "first"
            );
            drop(stats);
            if direct {
                assert!(!path.parent().expect("owned directory").exists());
            } else {
                assert!(path.exists(), "caller stats are never deleted");
            }
        }
    }
    assert!(
        !fs::read_dir(&root).expect("listing").any(|e| e
            .expect("entry")
            .file_name()
            .to_string_lossy()
            .starts_with(".av1an-stats-"))
    );
    assert_eq!(
        fs::read_to_string(&output).expect("final sentinel"),
        "preexisting final output"
    );
    assert_eq!(
        fs::read_to_string(root.join("unrelated.stat")).expect("stats sentinel"),
        "unrelated stats"
    );
    fs::write(root.join("verified"), "all assertions executed").expect("verification handshake");
    eprintln!(
        "E27 {api}-{case}: Err{code}, exact failing argv/raw stdout+stderr, order={executed:?}, progress={expected_progress:?}, stats alive through completion then owned cleanup/caller retention; sentinels intact"
    );
}
