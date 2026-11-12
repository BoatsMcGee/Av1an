#[path = "common.rs"]
mod common;

#[cfg(test)]
mod tests {
    use super::common;
    use std::{
        path::Path,
        process::Command,
        sync::{Arc, atomic::AtomicBool, mpsc},
    };

    use andean_condor::{
        core::sequence::scene_concatenator::SceneConcatenator,
        models::sequence::scene_concatenator::{ConcatMethod, MkvmergeConfig},
    };
    use california_condor::{
        commands::{DiscardCategory, handlers::concatenate::concatenate_handler},
        configuration::Configuration,
        test_helpers::{TestVideo, default_config},
    };
    use serde_json::{Value, json};

    fn run(command: &mut Command) -> Vec<u8> {
        let result = command.output().expect("required native tool");
        assert!(
            result.status.success(),
            "{command:?}: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        result.stdout
    }

    fn identify(path: &Path) -> Value {
        serde_json::from_slice(&run(Command::new("mkvmerge").arg("-J").arg(path)))
            .expect("native identify")
    }

    fn fixture(root: &Path) -> (std::path::PathBuf, Vec<std::path::PathBuf>) {
        let attachment = root.join("source attachment.txt");
        std::fs::write(&attachment, b"X08 exact attachment bytes\x00\xff").expect("attachment");
        let base = root.join("base.mkv");
        run(Command::new("ffmpeg")
            .args([
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "color=red:size=32x16:rate=24",
                "-f",
                "lavfi",
                "-i",
                "sine=sample_rate=48000",
                "-t",
                "0.2",
                "-c:v",
                "ffv1",
                "-c:a",
                "flac",
            ])
            .arg(&base));
        let source = root.join("source.mkv");
        run(Command::new("mkvmerge")
            .arg("-o")
            .arg(&source)
            .args([
                "--attachment-name",
                "original.txt",
                "--attachment-description",
                "original description",
                "--attachment-mime-type",
                "text/plain",
                "--attach-file",
            ])
            .arg(&attachment)
            .arg(&base));
        let ids = identify(&source);
        assert!(
            ids["tracks"]
                .as_array()
                .expect("tracks")
                .iter()
                .any(|t| t["id"] == 1 && t["type"] == "audio")
        );
        assert_eq!(
            ids["attachments"][0]["id"], 1,
            "real independent ID collision"
        );
        let chunks = ["black", "white"]
            .into_iter()
            .enumerate()
            .map(|(i, color)| {
                let path = root.join(format!("chunk-{i}.mkv"));
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
            .collect();
        (source, chunks)
    }

    fn native_discard(category: DiscardCategory) {
        let dir = tempfile::tempdir().expect("fixture");
        let (source, chunks) = fixture(dir.path());
        let original = std::fs::read(&source).expect("original");
        let output = dir.path().join("out.mkv");
        let video = TestVideo {
            path: source.clone(),
            width: 32,
            height: 16,
            frames: 4,
            fps_rational: (24, 1),
            format: andean_condor::ffmpeg::FFPixelFormat::YUV420P,
            scenes: vec![],
        };
        let mut config = default_config(&video, &output, &dir.path().join("work"));
        config.condor.sequence_config.scene_concatenator.method = ConcatMethod::MKVMerge(serde_json::from_value(json!({
        "tracks":{"0":{"type":"video", "name":"encoded scenes"},"1":{"type":"audio", "name":"kept audio", "language":"jpn", "default_track":false}},
        "attachments":{"1":{"copy":true}}
    })).expect("canonical config"));
        let file = dir.path().join("condor.json");
        config.save(&file).expect("save");
        let (config, _) = concatenate_handler(Some(&file), None, None, Some(&[category]), false)
            .expect("real source probe/discard/save");
        let ConcatMethod::MKVMerge(settings) =
            config.condor.sequence_config.scene_concatenator.method
        else {
            panic!("mkvmerge")
        };
        let (tx, _rx) = mpsc::channel();
        SceneConcatenator::mkvmerge(
            dir.path(),
            &output,
            &chunks,
            Some(&source),
            (24, 1).into(),
            Some(&settings),
            &tx,
            &Arc::new(AtomicBool::new(false)),
        )
        .expect("native final mux");
        let ids = identify(&output);
        let audio: Vec<_> = ids["tracks"]
            .as_array()
            .expect("tracks")
            .iter()
            .filter(|t| t["type"] == "audio")
            .collect();
        if matches!(category, DiscardCategory::Attachment) {
            assert_eq!(audio.len(), 1, "discarding attachment1 must keep audio1");
            assert!(ids["attachments"].as_array().expect("attachments").is_empty());
            assert_eq!(audio[0]["properties"]["language"], "jpn");
            assert_eq!(audio[0]["properties"]["track_name"], "kept audio");
            assert_eq!(audio[0]["properties"]["default_track"], false);
            let pcm = |path: &Path| {
                run(Command::new("ffmpeg")
                    .args(["-v", "error", "-i"])
                    .arg(path)
                    .args(["-map", "0:a:0", "-f", "s16le", "-"]))
            };
            assert_eq!(pcm(&source), pcm(&output), "exact decoded audio identity");
            let packets = |path: &Path| {
                let data: Value = serde_json::from_slice(&run(Command::new("ffprobe")
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
                    .map(|p| p["data_hash"].clone())
                    .collect::<Vec<_>>()
            };
            let original_packets = packets(&source);
            assert!(!original_packets.is_empty());
            assert_eq!(
                packets(&output),
                original_packets,
                "compressed audio bytes unchanged"
            );
        } else {
            assert!(audio.is_empty(), "discarding audio1 must drop audio1");
            assert_eq!(
                ids["attachments"].as_array().expect("attachments").len(),
                1,
                "discarding audio1 must keep attachment1"
            );
            assert_eq!(ids["attachments"][0]["file_name"], "original.txt");
            assert_eq!(ids["attachments"][0]["description"], "original description");
            assert_eq!(ids["attachments"][0]["content_type"], "text/plain");
            let extracted = dir.path().join("extracted");
            run(Command::new("mkvextract")
                .arg(&output)
                .arg("attachments")
                .arg(format!("1:{}", extracted.display())));
            assert_eq!(
                std::fs::read(extracted).expect("extracted"),
                b"X08 exact attachment bytes\x00\xff"
            );
        }
        let pixels = run(Command::new("ffmpeg")
            .args(["-v", "error", "-i"])
            .arg(&output)
            .args(["-map", "0:v:0", "-pix_fmt", "gray", "-f", "rawvideo", "-"]));
        assert_eq!(pixels.len(), 32 * 16 * 4);
        assert!(pixels[..32 * 16 * 2].iter().all(|&p| p < 5));
        assert!(pixels[32 * 16 * 2..].iter().all(|&p| p > 250));
        assert_eq!(std::fs::read(source).expect("unchanged source"), original);
        assert_eq!(
            Configuration::load(&file)
                .expect("load")
                .expect("saved")
                .condor
                .sequence_config
                .scene_concatenator
                .method,
            ConcatMethod::MKVMerge(settings)
        );
    }

    #[test]
    fn x08_native_discard_attachment1_keeps_audio1() {
        native_discard(DiscardCategory::Attachment);
    }

    #[test]
    fn x08_native_discard_audio1_keeps_attachment1() {
        native_discard(DiscardCategory::Audio);
    }

    #[test]
    fn x08_reader_migrates_legacy_and_refuses_ambiguous_ids() {
        let legacy: MkvmergeConfig = serde_json::from_str(r#"{"tracks":{"1":{"type":"attachment","copy":false,"path":"font.ttf","attachment_name":"exact name","attachment_description":"exact description","attachment_mime_type":"font/ttf"}},"extra_inputs":[{"path":"extra.mkv","tracks":{"1":{"type":"attachment"}}}]}"#).expect("unambiguous legacy");
        let canonical = serde_json::to_value(&legacy).expect("canonical write");
        assert!(
            canonical["tracks"].is_null()
                || canonical["tracks"].as_object().expect("map").is_empty()
        );
        assert_eq!(canonical["attachments"]["1"]["path"], "font.ttf");
        assert_eq!(canonical["attachments"]["1"]["copy"], false);
        assert_eq!(
            canonical["attachments"]["1"]["attachment_name"],
            "exact name"
        );
        assert_eq!(
            canonical["attachments"]["1"]["attachment_description"],
            "exact description"
        );
        assert_eq!(
            canonical["attachments"]["1"]["attachment_mime_type"],
            "font/ttf"
        );
        assert!(canonical["attachments"]["1"].get("type").is_none());
        assert_eq!(
            canonical["extra_inputs"][0]["attachments"]["1"]["copy"],
            true
        );
        assert_eq!(
            serde_json::from_value::<MkvmergeConfig>(canonical).expect("reload"),
            legacy
        );
        for text in [
            r#"{"tracks":{"1":{"type":"attachment"}},"attachments":{"1":{}}}"#,
            r#"{"tracks":{"1":{"type":"attachment"},"1":{"type":"audio"}}}"#,
            r#"{"tracks":{"1":{"type":"attachment"},"1":{"type":"attachment"}}}"#,
            r#"{"tracks":{"1":{"type":"audio"},"1":{"type":"attachment"}}}"#,
            r#"{"attachments":{"1":{}},"tracks":{"1":{"type":"attachment"}}}"#,
            r#"{"attachments":{"1":{},"1":{}}}"#,
            r#"{"extra_inputs":[{"path":"extra.mkv","tracks":{"1":{"type":"attachment"},"1":{"type":"audio"}}}]}"#,
            r#"{"extra_inputs":[{"path":"extra.mkv","tracks":{"1":{"type":"attachment"}},"attachments":{"1":{}}}]}"#,
        ] {
            let error = serde_json::from_str::<MkvmergeConfig>(text)
                .expect_err("ambiguous ID must not overwrite")
                .to_string();
            assert!(
                error.contains('1') && (error.contains("duplicate") || error.contains("collision")),
                "{error}"
            );
        }
    }

    #[test]
    fn x08_cli_canonical_info_save_method_reset_and_ambiguous_load() {
        let dir = tempfile::tempdir().expect("fixture");
        let (source, _) = fixture(dir.path());
        common::condor_cmd(&dir)
            .args([
                "init",
                source.to_str().expect("path"),
                dir.path().join("out.mkv").to_str().expect("path"),
                "--concat",
                "mkvmerge",
                "--discard",
                "audio,attachment",
            ])
            .assert()
            .success();
        let saved = Configuration::load(&dir.path().join("condor.json"))
            .expect("load")
            .expect("saved");
        let saved = serde_json::to_value(saved).expect("canonical CLI save");
        let method =
            &saved["condor"]["sequence_config"]["scene_concatenator"]["method"]["mkvmerge"];
        assert_eq!(method["tracks"]["1"]["type"], "audio");
        assert_eq!(method["tracks"]["1"]["copy"], false);
        assert_eq!(method["attachments"]["1"]["copy"], false);
        let video = TestVideo {
            path: source.clone(),
            width: 32,
            height: 16,
            frames: 4,
            fps_rational: (24, 1),
            format: andean_condor::ffmpeg::FFPixelFormat::YUV420P,
            scenes: vec![],
        };
        let mut config = default_config(
            &video,
            &dir.path().join("out.mkv"),
            &dir.path().join("work"),
        );
        let settings: MkvmergeConfig = serde_json::from_value(json!({
        "tracks":{"1":{"type":"audio","language":"jpn"}}, "attachments":{"1":{"copy":true}},
        "extra_inputs":[{"path":source,"tracks":{"0":{"type":"video","name":"extra video"},"1":{"type":"audio"}},"attachments":{"1":{"copy":false}}}]
    })).expect("canonical");
        config.condor.sequence_config.scene_concatenator.method =
            ConcatMethod::MKVMerge(settings.clone());
        let file = dir.path().join("condor.json");
        config.save(&file).expect("save");
        let info = common::condor_cmd(&dir)
            .args(["concatenate", "info", "method"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let info = String::from_utf8(info).expect("text");
        assert!(
            info.contains("attachments") && info.contains("jpn"),
            "{info}"
        );
        common::condor_cmd(&dir)
            .args(["concatenate", "set", "method", "mkvmerge"])
            .assert()
            .success();
        assert_eq!(
            Configuration::load(&file)
                .expect("load")
                .expect("saved")
                .condor
                .sequence_config
                .scene_concatenator
                .method,
            ConcatMethod::MKVMerge(settings)
        );
        common::condor_cmd(&dir)
            .args(["concatenate", "set", "method", "ffmpeg"])
            .assert()
            .success();
        common::condor_cmd(&dir)
            .args(["concatenate", "set", "method", "mkvmerge"])
            .assert()
            .success();
        assert_eq!(
            Configuration::load(&file)
                .expect("load")
                .expect("saved")
                .condor
                .sequence_config
                .scene_concatenator
                .method,
            ConcatMethod::MKVMerge(MkvmergeConfig::default())
        );
        config.save(&file).expect("restore nondefault namespaces");
        common::condor_cmd(&dir)
            .args(["concatenate", "reset", "method"])
            .assert()
            .success();
        assert_eq!(
            Configuration::load(&file)
                .expect("load")
                .expect("saved")
                .condor
                .sequence_config
                .scene_concatenator
                .method,
            ConcatMethod::MKVMerge(MkvmergeConfig::default())
        );
        let mut text = serde_json::to_value(&config).expect("JSON");
        text["condor"]["sequence_config"]["scene_concatenator"]["method"] =
            json!({"mkvmerge":{"tracks":{"1":{"type":"attachment"}},"attachments":{"1":{}}}});
        let before = serde_json::to_vec_pretty(&text).expect("ambiguous");
        std::fs::write(&file, &before).expect("legacy job");
        let result = common::condor_cmd(&dir)
            .args(["concatenate", "set", "method", "mkvmerge"])
            .output()
            .expect("CLI");
        assert!(!result.status.success());
        let error = String::from_utf8_lossy(&result.stderr);
        assert!(
            error.contains("attachment ID 1") && error.contains("collision"),
            "{error}"
        );
        assert_eq!(std::fs::read(&file).expect("no partial save"), before);
        // The duplicate-key check is also exercised through the full root reader.
        let mut job = serde_json::to_string(&config).expect("job");
        let method =
            serde_json::to_string(&config.condor.sequence_config.scene_concatenator.method)
                .expect("method");
        job = job.replace(
            &method,
            r#"{"mkvmerge":{"tracks":{"1":{"type":"attachment"},"1":{"type":"audio"}}}}"#,
        );
        std::fs::write(&file, job.as_bytes()).expect("duplicate legacy key");
        let result = common::condor_cmd(&dir)
            .args(["concatenate", "reset", "method"])
            .output()
            .expect("CLI");
        assert!(!result.status.success());
        assert!(
            String::from_utf8_lossy(&result.stderr)
                .contains("duplicate mkvmerge track/attachment ID 1")
        );
        assert_eq!(std::fs::read(&file).expect("no save"), job.as_bytes());
    }

    #[test]
    fn x08_schema_and_default_controls_are_canonical() {
        let schema = serde_json::to_value(schemars::schema_for!(MkvmergeConfig)).expect("schema");
        assert_eq!(
            schema["properties"]["attachments"]["patternProperties"][r"^\d+$"]["$ref"],
            "#/$defs/AttachmentTrack"
        );
        assert!(schema["$defs"]["MkvmergeExtraInput"]["properties"].get("attachments").is_some());
        let track_shapes = schema["$defs"]["MkvmergeTrack"].to_string();
        assert!(track_shapes.contains("audio") && !track_shapes.contains("attachment"));
        for text in ["{}", r#"{"tracks":null,"attachments":null}"#] {
            assert_eq!(
                serde_json::from_str::<MkvmergeConfig>(text).expect("defaults"),
                MkvmergeConfig::default()
            );
        }
        let empty: MkvmergeConfig = serde_json::from_str(r#"{"tracks":{},"attachments":{},"extra_inputs":[{"path":"extra","tracks":null,"attachments":{}}]}"#).expect("empty maps");
        assert!(empty.input_args().is_empty());
        assert!(empty.extra_inputs[0].input_args().is_empty());
        assert_eq!(
            serde_json::from_value::<MkvmergeConfig>(serde_json::to_value(&empty).expect("write"))
                .expect("reload"),
            empty
        );
    }

    #[test]
    fn x08_native_extra_video1_attachment1_and_output_video0_stay_independent() {
        let dir = tempfile::tempdir().expect("fixture");
        let (source, chunks) = fixture(dir.path());
        let extra = dir.path().join("extra.mkv");
        let extra_attachment = dir.path().join("extra attachment.txt");
        std::fs::write(&extra_attachment, b"distinct extra attachment bytes")
            .expect("extra attachment");
        run(Command::new("mkvmerge")
            .arg("-o")
            .arg(&extra)
            .args([
                "--track-order",
                "0:1,0:0",
                "--attachment-name",
                "extra.txt",
                "--attachment-mime-type",
                "text/plain",
                "--attach-file",
            ])
            .arg(&extra_attachment)
            .arg(dir.path().join("base.mkv")));
        let ids = identify(&extra);
        assert_eq!(ids["tracks"][1]["id"], 1);
        assert_eq!(ids["tracks"][1]["type"], "video");
        assert_eq!(ids["attachments"][0]["id"], 1);
        let original = std::fs::read(&extra).expect("extra original");
        let added = dir.path().join("new attachment.txt");
        std::fs::write(&added, b"X08 new attachment identity").expect("new attachment");
        let extra_added = dir.path().join("new extra attachment.txt");
        std::fs::write(&extra_added, b"X08 extra new attachment identity")
            .expect("extra new attachment");
        for drop_video in [false, true] {
            let settings: MkvmergeConfig = serde_json::from_value(json!({
            "tracks":{"0":{"type":"video","copy":!drop_video,"name":"encoded only"},"1":{"type":"audio","language":"jpn"}},
            "attachments":{"1":{"path":added,"attachment_name":"new exact.txt","attachment_description":"new exact description","attachment_mime_type":"text/plain"},"2":{"copy":false,"path":"must not read","attachment_mime_type":"text/plain"}},
            "extra_inputs":[{"path":extra,"tracks":{"0":{"type":"audio","copy":false},"1":{"type":"video","copy":drop_video,"name":"extra actual video1"}},"attachments":{"1":{"copy":!drop_video},"7":{"path":extra_added,"attachment_name":"extra new.txt","attachment_description":"extra new description","attachment_mime_type":"text/plain"},"8":{"copy":false,"path":"must not read extra"}}}]
        })).expect("coexisting canonical maps");
            let output = dir.path().join(format!("extra-{drop_video}.mkv"));
            let (tx, _rx) = mpsc::channel();
            SceneConcatenator::mkvmerge(
                dir.path(),
                &output,
                &chunks,
                Some(&source),
                (24, 1).into(),
                Some(&settings),
                &tx,
                &Arc::new(AtomicBool::new(false)),
            )
            .expect("native extra namespaces");
            let ids = identify(&output);
            let tracks = ids["tracks"].as_array().expect("tracks");
            let video: Vec<_> = tracks.iter().filter(|t| t["type"] == "video").collect();
            assert_eq!(video.len(), 1);
            assert_eq!(
                video[0]["properties"]["track_name"],
                if drop_video {
                    "extra actual video1"
                } else {
                    "encoded only"
                }
            );
            let attachments = ids["attachments"].as_array().expect("attachments");
            assert_eq!(
                attachments.len(),
                if drop_video { 3 } else { 4 },
                "main copied1 plus new1 and extra new7, optionally extra copied1"
            );
            let new = attachments
                .iter()
                .find(|a| a["file_name"] == "new exact.txt")
                .expect("new attachment");
            assert_eq!(new["description"], "new exact description");
            assert_eq!(new["content_type"], "text/plain");
            let extracted = dir.path().join(format!("new-{drop_video}"));
            run(
                Command::new("mkvextract").arg(&output).arg("attachments").arg(format!(
                    "{}:{}",
                    new["id"],
                    extracted.display()
                )),
            );
            assert_eq!(
                std::fs::read(extracted).expect("bytes"),
                b"X08 new attachment identity"
            );
            let new = attachments
                .iter()
                .find(|a| a["file_name"] == "extra new.txt")
                .expect("extra new attachment");
            assert_eq!(new["description"], "extra new description");
            assert_eq!(new["content_type"], "text/plain");
            let extracted = dir.path().join(format!("extra-new-{drop_video}"));
            run(
                Command::new("mkvextract").arg(&output).arg("attachments").arg(format!(
                    "{}:{}",
                    new["id"],
                    extracted.display()
                )),
            );
            assert_eq!(
                std::fs::read(extracted).expect("extra bytes"),
                b"X08 extra new attachment identity"
            );
            let audio: Vec<_> = tracks.iter().filter(|t| t["type"] == "audio").collect();
            assert_eq!(audio.len(), 1);
            assert_eq!(audio[0]["properties"]["language"], "jpn");
        }
        assert_eq!(std::fs::read(&extra).expect("extra unchanged"), original);
        for drop_audio in [false, true] {
            let settings: MkvmergeConfig = serde_json::from_value(json!({"extra_inputs":[{"path":source,"tracks":{"0":{"type":"video","copy":false},"1":{"type":"audio","copy":!drop_audio,"language":"jpn"}},"attachments":{"1":{"copy":drop_audio}}}]})).expect("extra audio1 attachment1");
            assert_eq!(
                serde_json::from_value::<MkvmergeConfig>(
                    serde_json::to_value(&settings).expect("write")
                )
                .expect("reload"),
                settings
            );
            let output = dir.path().join(format!("extra-audio-{drop_audio}.mkv"));
            let (tx, _rx) = mpsc::channel();
            SceneConcatenator::mkvmerge(
                dir.path(),
                &output,
                &chunks,
                None,
                (24, 1).into(),
                Some(&settings),
                &tx,
                &Arc::new(AtomicBool::new(false)),
            )
            .expect("extra independent audio/attachment");
            let ids = identify(&output);
            let audio: Vec<_> = ids["tracks"]
                .as_array()
                .expect("tracks")
                .iter()
                .filter(|t| t["type"] == "audio")
                .collect();
            assert_eq!(audio.len(), usize::from(!drop_audio));
            assert_eq!(
                ids["attachments"].as_array().expect("attachments").len(),
                usize::from(drop_audio)
            );
            if !drop_audio {
                assert_eq!(audio[0]["properties"]["language"], "jpn");
            }
        }
    }

    #[test]
    fn x08_direct_rust_invalid_video_refuses_before_writes_and_unsupported_ids_stay_conservative() {
        let dir = tempfile::tempdir().expect("fixture");
        let (source, chunks) = fixture(dir.path());
        let output = dir.path().join("sentinel.mkv");
        std::fs::write(&output, b"existing output").expect("sentinel");
        let (tx, _rx) = mpsc::channel();
        let settings = MkvmergeConfig {
            tracks: Some(std::collections::HashMap::from([(
                9,
                andean_condor::models::sequence::scene_concatenator::MkvmergeTrack::plain(
                    andean_condor::models::sequence::scene_concatenator::MkvmergeTrackKind::Video,
                    true,
                ),
            )])),
            ..Default::default()
        };
        let scratch = dir.path().join("not-created/scenes");
        let error = SceneConcatenator::mkvmerge(
            &scratch,
            &output,
            &chunks,
            Some(&source),
            (24, 1).into(),
            Some(&settings),
            &tx,
            &Arc::new(AtomicBool::new(false)),
        )
        .expect_err("direct invalid config");
        assert!(error.to_string().contains("video track entry must use track ID 0"));
        assert!(!dir.path().join("not-created").exists());
        assert_eq!(
            std::fs::read(&output).expect("unchanged"),
            b"existing output"
        );
        let y4m = dir.path().join("source.y4m");
        run(Command::new("ffmpeg")
            .args([
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "color=size=32x16:rate=24",
                "-frames:v",
                "2",
            ])
            .arg(&y4m));
        let added = dir.path().join("independent.txt");
        std::fs::write(&added, b"new file not source ID").expect("new file");
        let settings = serde_json::from_value(
            json!({"attachments":{"1":{"path":added,"attachment_mime_type":"text/plain"}}}),
        )
        .expect("new path");
        SceneConcatenator::mkvmerge(
            dir.path(),
            &output,
            &chunks,
            Some(&y4m),
            (24, 1).into(),
            Some(&settings),
            &tx,
            &Arc::new(AtomicBool::new(false)),
        )
        .expect("new file needs no unsupported source ID");
        assert_eq!(
            identify(&output)["attachments"].as_array().expect("attachments").len(),
            1
        );
        for copy in [true, false] {
            std::fs::write(&output, b"existing output").expect("sentinel");
            let settings = serde_json::from_value(json!({"attachments":{"1":{"copy":copy}}}))
                .expect("source ID");
            let error = SceneConcatenator::mkvmerge(
                dir.path(),
                &output,
                &chunks,
                Some(&y4m),
                (24, 1).into(),
                Some(&settings),
                &tx,
                &Arc::new(AtomicBool::new(false)),
            )
            .expect_err("unsupported source has no IDs");
            assert!(error.to_string().contains("no provable mkvmerge track/attachment ID mapping"));
            assert_eq!(
                std::fs::read(&output).expect("unchanged"),
                b"existing output"
            );
        }
    }
}
