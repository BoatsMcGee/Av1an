use super::*;
use crate::core::sequence::scene_concatenator::SceneConcatenator;
use std::process::Command;

fn tasks(dir: &Path, counts: &[usize]) -> Vec<Task> {
    counts
        .iter()
        .enumerate()
        .map(|(index, &count)| Task {
            original_index: index,
            index,
            frame_indices: (0..count).collect(),
            sub_scenes: None,
            encoder: Encoder::default_from_base(&EncoderBase::AOM, false),
            output: dir.join(format!("part{index}.ivf")),
        })
        .collect()
}

// Independent standard-byte oracle, not production constants or demuxer metadata.
fn records(bytes: &[u8]) -> Vec<(u64, Vec<u8>)> {
    assert_eq!(&bytes[..4], b"DKIF");
    assert_eq!(
        u16::from_le_bytes(bytes[6..8].try_into().expect("header size")),
        32
    );
    let mut cursor = 32;
    let mut frames = Vec::new();
    while cursor < bytes.len() {
        let size =
            u32::from_le_bytes(bytes[cursor..cursor + 4].try_into().expect("frame size")) as usize;
        let timestamp =
            u64::from_le_bytes(bytes[cursor + 4..cursor + 12].try_into().expect("timestamp"));
        frames.push((timestamp, bytes[cursor + 12..cursor + 12 + size].to_vec()));
        cursor += 12 + size;
    }
    assert_eq!(cursor, bytes.len());
    frames
}

fn set_timestamps(bytes: &mut [u8], timestamps: &[u64]) {
    let mut cursor = 32;
    for timestamp in timestamps {
        let size =
            u32::from_le_bytes(bytes[cursor..cursor + 4].try_into().expect("frame size")) as usize;
        bytes[cursor + 4..cursor + 12].copy_from_slice(&timestamp.to_le_bytes());
        cursor += 12 + size;
    }
    assert_eq!(cursor, bytes.len());
}

fn fixture(path: &Path, timestamps: &[u64]) {
    let mut bytes = vec![0; 32];
    bytes[..4].copy_from_slice(b"DKIF");
    bytes[6..8].copy_from_slice(&32u16.to_le_bytes());
    bytes[8..12].copy_from_slice(b"AV01");
    bytes[12..16].copy_from_slice(&[64, 0, 64, 0]);
    bytes[16..20].copy_from_slice(&24000u32.to_le_bytes());
    bytes[20..24].copy_from_slice(&1001u32.to_le_bytes());
    bytes[24..28].copy_from_slice(&(timestamps.len() as u32).to_le_bytes());
    for timestamp in timestamps {
        bytes.extend_from_slice(&1u32.to_le_bytes());
        bytes.extend_from_slice(&timestamp.to_le_bytes());
        bytes.push(0xAB);
    }
    fs::write(path, bytes).expect("write synthetic IVF");
}

fn concatenate(output: &Path, paths: &[PathBuf]) -> Result<()> {
    let (tx, _rx) = std::sync::mpsc::channel();
    SceneConcatenator::ivf_cancel(output, paths, &tx, &Cancellation::default())
}

fn native(tool: &str, args: &[&str], input: Option<&Path>) -> Vec<u8> {
    // A failed/hung native tool is a test failure, never an optional skip or benchmark.
    let mut command = Command::new("timeout");
    command.args(["30s", tool]).args(args);
    if let Some(input) = input {
        command.arg(input);
    }
    let output = command.output().expect("required native tool/watchdog");
    assert!(
        output.status.success(),
        "{command:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}

fn decoded(path: &Path) -> Vec<u8> {
    let args = [
        "-v",
        "error",
        "-i",
        path.to_str().expect("UTF-8 test path"),
        "-f",
        "rawvideo",
        "-pix_fmt",
        "yuv420p",
        "-fps_mode",
        "passthrough",
        "-",
    ];
    native("ffmpeg", &args, None)
}

fn assert_output(path: &Path, count: u32, rate: &[u8], timestamps: &[u64]) {
    let bytes = fs::read(path).expect("read native output");
    assert_eq!(
        u32::from_le_bytes(bytes[24..28].try_into().expect("frame count")),
        count
    );
    assert_eq!(&bytes[16..24], rate);
    assert_eq!(
        records(&bytes).iter().map(|r| r.0).collect::<Vec<_>>(),
        timestamps
    );
    let probe = native(
        "ffprobe",
        &[
            "-v",
            "error",
            "-count_frames",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=r_frame_rate,nb_read_frames",
            "-of",
            "default=nw=1",
        ],
        Some(path),
    );
    let probe = String::from_utf8(probe).expect("UTF-8 probe output");
    assert!(
        probe.contains(&format!("nb_read_frames={count}\n")),
        "{probe}"
    );
    assert!(probe.contains("r_frame_rate=24000/1001\n"), "{probe}");
}

#[test]
fn x07_native_nonzero_whole_split_three_parts_and_concat() {
    let dir = tempfile::tempdir().expect("whole-stream fixture directory");
    let whole = dir.path().join("whole.ivf");
    native(
        "ffmpeg",
        &[
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=64x64:rate=24000/1001",
            "-frames:v",
            "9",
            "-c:v",
            "libaom-av1",
            "-cpu-used",
            "8",
            "-crf",
            "0",
            "-g",
            "1",
        ],
        Some(&whole),
    );
    let mut bytes = fs::read(&whole).expect("read whole IVF");
    set_timestamps(&mut bytes, &[900, 901, 902, 903, 904, 905, 906, 907, 908]);
    fs::write(&whole, &bytes).expect("set nonzero whole origin");
    let original_pixels = decoded(&whole);
    assert_eq!(original_pixels.len(), 64 * 64 * 3 / 2 * 9);
    assert!(original_pixels.as_chunks::<6144>().0.windows(2).all(|p| p[0] != p[1]));
    let tasks = tasks(dir.path(), &[2, 3, 4]);
    let split = split_ivf(&whole, &tasks, "ivf", true).expect("split native whole IVF");
    let paths = tasks.iter().map(|t| t.output.clone()).collect::<Vec<_>>();
    let mut payloads = Vec::new();
    for (index, (path, expected)) in
        paths.iter().zip([vec![0, 1], vec![0, 1, 2], vec![0, 1, 2, 3]]).enumerate()
    {
        assert_output(path, expected.len() as u32, &bytes[16..24], &expected);
        let part = fs::read(path).expect("read split part");
        assert_eq!(split[index], (part.len() as u64, expected.len()));
        payloads.extend(records(&part).into_iter().map(|r| r.1));
    }
    assert_eq!(
        payloads,
        records(&bytes).into_iter().map(|r| r.1).collect::<Vec<_>>()
    );
    let output = dir.path().join("joined.ivf");
    concatenate(&output, &paths).expect("concat normalized split parts");
    assert_output(&output, 9, &bytes[16..24], &[0, 1, 2, 3, 4, 5, 6, 7, 8]);
    assert_eq!(decoded(&output), original_pixels);
    // Concat also accepts independently nonzero origins, not only normalized splitter output.
    for (path, timestamps) in
        paths
            .iter()
            .zip([vec![900, 901], vec![902, 903, 904], vec![905, 906, 907, 908]])
    {
        let mut part = fs::read(path).expect("read split part");
        set_timestamps(&mut part, &timestamps);
        fs::write(path, part).expect("set nonzero part origin");
    }
    concatenate(&output, &paths).expect("concat absolute-origin parts");
    assert_output(&output, 9, &bytes[16..24], &[0, 1, 2, 3, 4, 5, 6, 7, 8]);
    assert_eq!(decoded(&output), original_pixels);
    assert_eq!(
        fs::read(&whole).expect("read whole after concat"),
        bytes,
        "whole input untouched"
    );
}

#[test]
fn x07_native_zero_origin_encodes_keep_supplied_order_and_fractional_rate() {
    let dir = tempfile::tempdir().expect("independent encode fixture directory");
    let paths = (0..3).map(|i| dir.path().join(format!("{i}.ivf"))).collect::<Vec<_>>();
    for (path, color) in paths.iter().zip(["white", "black", "red"]) {
        native(
            "ffmpeg",
            &[
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                &format!("color={color}:size=64x64:rate=24000/1001"),
                "-frames:v",
                "2",
                "-c:v",
                "libaom-av1",
                "-cpu-used",
                "8",
                "-g",
                "1",
            ],
            Some(path),
        );
    }
    let originals = paths
        .iter()
        .map(|p| fs::read(p).expect("read independent encode"))
        .collect::<Vec<_>>();
    let pixels = paths.iter().map(|p| decoded(p)).collect::<Vec<_>>();
    let output = dir.path().join("joined.ivf");
    concatenate(
        &output,
        &[paths[2].clone(), paths[0].clone(), paths[1].clone(), paths[2].clone()],
    )
    .expect("concat independent zero-origin encodes");
    assert_output(&output, 8, &originals[0][16..24], &[0, 1, 2, 3, 4, 5, 6, 7]);
    assert_eq!(
        decoded(&output),
        [&pixels[2][..], &pixels[0], &pixels[1], &pixels[2]].concat()
    );
    for (path, original) in paths.iter().zip(originals) {
        assert_eq!(fs::read(path).expect("read original encode"), original);
    }
}

#[test]
fn x07_checked_gaps_boundaries_and_invalid_inputs() {
    let dir = tempfile::tempdir().expect("boundary fixture directory");
    let a = dir.path().join("a.ivf");
    let b = dir.path().join("b.ivf");
    let output = dir.path().join("joined.ivf");
    fixture(&a, &[50, 52, 55]);
    fixture(&b, &[100, 103]);
    concatenate(&output, &[a.clone(), b.clone()]).expect("concat gapped timestamps");
    assert_eq!(
        records(&fs::read(&output).expect("read gapped output"))
            .iter()
            .map(|r| r.0)
            .collect::<Vec<_>>(),
        [0, 2, 5, 6, 9]
    );
    fixture(&b, &[100, 103]);
    let mut equivalent = fs::read(&b).expect("read equivalent timebase fixture");
    equivalent[16..20].copy_from_slice(&48000u32.to_le_bytes());
    equivalent[20..24].copy_from_slice(&2002u32.to_le_bytes());
    fs::write(&b, equivalent).expect("write equivalent timebase fixture");
    concatenate(&output, &[a.clone(), b.clone()]).expect("concat equivalent timebases");
    assert_eq!(
        &fs::read(&output).expect("read rational rate")[16..24],
        &[192, 93, 0, 0, 233, 3, 0, 0]
    );
    // Last frame occupies one nominal IVF tick, even when earlier frames have gaps.
    fixture(&a, &[0, u64::MAX - 1]);
    fixture(&b, &[u64::MAX]);
    concatenate(&output, &[a.clone(), b.clone()]).expect("concat at u64 boundary");
    assert_eq!(
        records(&fs::read(&output).expect("read boundary output"))
            .iter()
            .map(|r| r.0)
            .collect::<Vec<_>>(),
        [0, u64::MAX - 1, u64::MAX]
    );
    fixture(&a, &[0, u64::MAX]);
    concatenate(&output, std::slice::from_ref(&a)).expect("allow final u64 max timestamp");
    assert_eq!(
        records(&fs::read(&output).expect("read final boundary output"))
            .iter()
            .map(|r| r.0)
            .collect::<Vec<_>>(),
        [0, u64::MAX]
    );
    for timestamps in [vec![0, u64::MAX], vec![3, 2], vec![4, 4]] {
        fixture(&a, &timestamps);
        let error = concatenate(&output, &[a.clone(), b.clone()]).expect_err("invalid timestamps");
        assert!(format!("{error:#}").contains("timestamp"), "{error:#}");
    }
    fixture(&a, &[0, u64::MAX - 1]);
    fixture(&b, &[0, 1]);
    let error = concatenate(&output, &[a.clone(), b.clone()]).expect_err("offset overflow");
    assert!(format!("{error:#}").contains("timestamp"), "{error:#}");
    fixture(&a, &[0]);
    for (offset, value) in [
        (16, 0u32),
        (20, 0),
        (16, 30),
        (20, 1),
        (16, u32::MAX),
        (8, u32::from_le_bytes(*b"VP90")),
        (12, 1),
    ] {
        fixture(&b, &[0]);
        let mut bytes = fs::read(&b).expect("read invalid header fixture");
        bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        fs::write(&b, bytes).expect("write invalid header fixture");
        assert!(
            concatenate(&output, &[a.clone(), b.clone()]).is_err(),
            "invalid/incompatible field {offset}: {value}"
        );
    }
    fixture(&b, &[0]);
    let mut bytes = fs::read(&b).expect("read count overflow fixture");
    bytes[24..28].copy_from_slice(&u32::MAX.to_le_bytes());
    fs::write(&b, bytes).expect("write count overflow fixture");
    let error = concatenate(&output, &[a.clone(), b]).expect_err("header count overflow");
    assert!(format!("{error:#}").contains("frame count"), "{error:#}");
    let split_tasks = tasks(dir.path(), &[2]);
    for timestamps in [vec![9, 8], vec![9, 9]] {
        fixture(&a, &timestamps);
        let error = split_ivf(&a, &split_tasks, "ivf", false)
            .expect_err("decreasing/duplicate split timestamp");
        assert!(format!("{error:#}").contains("timestamp"), "{error:#}");
        assert!(!split_tasks[0].output.exists());
        assert!(!split_tasks[0].output.with_extension("temp.ivf").exists());
    }
    for offset in [16, 20] {
        fixture(&a, &[0, 1]);
        let mut bytes = fs::read(&a).expect("read zero rate/scale fixture");
        bytes[offset..offset + 4].fill(0);
        fs::write(&a, bytes).expect("write zero rate/scale fixture");
        assert!(split_ivf(&a, &split_tasks, "ivf", false).is_err());
    }
    let boundary_dir = dir.path().join("boundary");
    fs::create_dir_all(&boundary_dir).expect("split boundary directory");
    let boundary_tasks = tasks(&boundary_dir, &[2, 1]);
    fixture(&a, &[u64::MAX - 5, u64::MAX - 3, u64::MAX]);
    split_ivf(&a, &boundary_tasks, "ivf", false).expect("split near u64 max");
    assert_eq!(
        records(&fs::read(&boundary_tasks[0].output).expect("read first boundary part"))
            .iter()
            .map(|r| r.0)
            .collect::<Vec<_>>(),
        [0, 2]
    );
    assert_eq!(
        records(&fs::read(&boundary_tasks[1].output).expect("read second boundary part"))
            .iter()
            .map(|r| r.0)
            .collect::<Vec<_>>(),
        [0]
    );
}
