//! I05: losing source options changes the image used as the metric reference.

use std::{fs, path::Path, process::Command};

use super::*;
use crate::{
    core::input::Ffms2Filter, models::input::ImportMethod,
    vapoursynth::script_builder::script::Lines,
};

fn run(command: &mut Command) -> Vec<u8> {
    // File-backed output plus timeout keeps fixture creation bounded without pipe deadlocks.
    let stdout = tempfile::NamedTempFile::new().expect("stdout");
    let stderr = tempfile::NamedTempFile::new().expect("stderr");
    let status = command
        .stdin(std::process::Stdio::null())
        .stdout(stdout.reopen().expect("stdout handle"))
        .stderr(stderr.reopen().expect("stderr handle"))
        .status()
        .expect("I05 requires ffmpeg, python3 and timeout on PATH");
    assert!(
        status.success(),
        "{command:?}: {}",
        fs::read_to_string(stderr.path()).expect("diagnostic")
    );
    fs::read(stdout.path()).expect("output")
}

struct Fixture {
    directory: tempfile::TempDir,
    source: std::path::PathBuf,
    original: Vec<u8>,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().expect("I05 fixture");
        let parent = directory.path().join("read only 映像");
        fs::create_dir_all(&parent).expect("source directory");
        let source = parent.join("two tracks.mkv");
        run(Command::new("timeout")
            .args(["30s", "ffmpeg", "-nostdin", "-v", "error", "-y"])
            .args(["-f", "lavfi", "-i", "nullsrc=s=32x16:r=24,geq=lum=37:cb=83:cr=201"])
            .args(["-f", "lavfi", "-i", "nullsrc=s=32x16:r=24,geq=lum=219:cb=41:cr=159"])
            .args([
                "-map",
                "0:v",
                "-map",
                "1:v",
                "-frames:v",
                "100",
                "-c:v",
                "ffv1",
                "-threads",
                "1",
            ])
            .arg(&source));
        let original = fs::read(&source).expect("source bytes");
        let fixture = Self {
            directory,
            source,
            original,
        };
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&fixture.source, fs::Permissions::from_mode(0o444))
                .expect("read-only file");
            fs::set_permissions(&parent, fs::Permissions::from_mode(0o555))
                .expect("read-only parent");
            let error = fs::write(parent.join("write probe"), b"must fail")
                .expect_err("OS must enforce read-only parent (run as non-root)");
            assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
        }
        fixture
    }

    fn cache(&self, name: &str) -> std::path::PathBuf {
        let directory = self.directory.path().join(name);
        fs::create_dir_all(&directory).expect("writable cache directory");
        directory.join("explicit cache 索引")
    }

    fn unchanged(&self) {
        assert_eq!(fs::read(&self.source).expect("source bytes"), self.original);
        assert_eq!(
            fs::read_dir(self.source.parent().expect("parent"))
                .expect("source listing")
                .count(),
            1,
            "no indexes beside read-only source"
        );
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(
                self.source.parent().expect("parent"),
                fs::Permissions::from_mode(0o755),
            )
            .expect("restore parent for cleanup");
            fs::set_permissions(&self.source, fs::Permissions::from_mode(0o644))
                .expect("restore source for cleanup");
        }
    }
}

fn files(path: &Path) -> Vec<std::path::PathBuf> {
    if path.is_file() {
        return vec![path.to_owned()];
    }
    if !path.exists() {
        return Vec::new();
    }
    fs::read_dir(path)
        .expect("cache listing")
        .flat_map(|entry| files(&entry.expect("cache entry").path()))
        .collect()
}

fn expected_frame(track: Option<u8>, width: usize) -> Vec<u8> {
    let [y, u, v] = if track == Some(1) {
        [219, 41, 159]
    } else {
        [37, 83, 201]
    };
    [b"FRAME\n".to_vec(), vec![y; width * 16], vec![u; width * 4], vec![v; width * 4]].concat()
}

fn routes(method: fn(Option<u8>) -> VapourSynthImportMethod) {
    let fixture = Fixture::new();
    // Start with nonzero so a lost selection fails on actual image data, not just kwargs.
    for track in [Some(1), Some(0), None] {
        for scripted in [false, true] {
            let cache = fixture.cache(&format!("{track:?}-{scripted}"));
            let data = InputModel::VapourSynth {
                path: fixture.source.clone(),
                import_method: method(track),
                cache_path: Some(cache.clone()),
                filters: vec![VapourSynthFilter::Crop {
                    left: Some(2),
                    right: Some(2),
                    top: None,
                    bottom: None,
                }],
            };
            // Catch dropped cache options before a regression can write BestSource's default
            // index outside this fixture. The image check below still uses the real plugin.
            let (_, lines) = source_script(&fixture.source, &method(track), Some(&cache))
                .expect("source options");
            let options = recorded(lines);
            let key = if matches!(method(track), VapourSynthImportMethod::BestSource { .. }) {
                "cachepath"
            } else {
                "cachefile"
            };
            assert_eq!(options[0][key], cache.display().to_string());
            let mut input = if scripted {
                Input::from_vapoursynth_scripted(&data)
            } else {
                Input::from_data(&data)
            }
            .expect("installed source plugin must open with explicit track/cache");
            assert_eq!(
                input.y4m_frame(0).expect("selected image").into_inner(),
                expected_frame(track, 28),
                "track={track:?}, scripted={scripted}"
            );
            let info = input.clip_info().expect("clip metadata");
            assert_eq!(info.resolution, (28, 16));
            assert_eq!(info.num_frames, 100);
            assert_eq!(
                (*info.frame_rate.numer(), *info.frame_rate.denom()),
                (24, 1)
            );
            assert!(input.streams_concurrently());
            if !scripted {
                let sources = input.frame_sources(5).expect("additional source instances");
                // FFMS2/LSMASH reopen after reader 1, BestSource after reader 4.
                let crate::core::input::FrameSourceKind::VapourSynth(node) = &sources[4].kind
                else {
                    panic!("VS frame source");
                };
                let image = write_y4m_frame(&node.get_frame(0).expect("additional source frame"))
                    .expect("additional source pixels");
                assert_eq!(image, expected_frame(track, 28)[6..]);
            }
            drop(input);
            let caches = files(cache.parent().expect("cache parent"));
            assert!(
                !caches.is_empty(),
                "explicit cache must be created: {cache:?}"
            );
            if matches!(method(track), VapourSynthImportMethod::BestSource { .. }) {
                assert!(caches.iter().all(|path| path.starts_with(&cache)));
            } else {
                assert_eq!(caches, [cache]);
            }
            eprintln!("I05 {track:?} scripted={scripted}: {caches:?}");
            fixture.unchanged();
        }
    }
}

#[test]
fn i05_ffms2_direct_and_scripted_track_cache() {
    routes(|index| VapourSynthImportMethod::FFMS2 { index });
}

#[test]
fn i05_bestsource_direct_and_scripted_track_cache() {
    routes(|index| VapourSynthImportMethod::BestSource { index });
}

#[test]
fn i05_lsmash_direct_and_scripted_track_cache() {
    routes(|index| VapourSynthImportMethod::LSMASHWorks { index });
}

#[test]
fn i05_native_metric_reference_preserves_track_cache_and_filter_baseline() {
    let fixture = Fixture::new();
    let cache = fixture.cache("native shared index");
    for track in [Some(1), Some(0), None] {
        let mut native = Input::from_data(&InputModel::Video {
            path: fixture.source.clone(),
            import_method: ImportMethod::FFMS2 {
                index: track,
                cache_path: Some(cache.clone()),
            },
            filters: vec![Ffms2Filter::OutputFormat {
                bit_depth: None,
                chroma: None,
                width: Some(16),
                height: None,
            }],
        })
        .expect("native FFMS2 input");
        let native_image = native.y4m_frame(0).expect("encode feed").into_inner();
        assert_eq!(native_image, expected_frame(track, 16));
        let original_index = fs::read(&cache).expect("native index");
        let mut reference = native
            .as_vapoursynth_script()
            .expect("metric reference")
            .expect("native becomes VS");
        assert_eq!(
            reference.y4m_frame(0).expect("reference image").into_inner(),
            native_image,
            "requested {track:?} must be the metric reference"
        );
        assert_eq!(
            reference.clip_info().expect("metadata").resolution,
            (16, 16)
        );
        let Input::Video { source_details, .. } = native else {
            panic!("native source");
        };
        assert_eq!(source_details.width, 32, "filter baseline stays unfiltered");
        assert_eq!(
            fs::read(&cache).expect("shared index"),
            original_index,
            "VS must read the native FFMS2 index without replacing it"
        );
        fixture.unchanged();
    }
}

fn recorded(lines: Lines) -> serde_json::Value {
    let mut script = VapourSynthScript::default();
    script.add_lines(lines);
    script.outputs.insert(0, "clip".to_owned());
    recorded_script(&script.to_string(), &HashMap::new())
}

fn recorded_script(script: &str, variables: &HashMap<String, String>) -> serde_json::Value {
    let script_file = tempfile::NamedTempFile::new().expect("generated script");
    fs::write(script_file.path(), script).expect("write generated script");
    let output = run(Command::new("timeout").args(["10s", "python3", "-c", r#"
import json, sys, types
calls = []
class Clip:
    def set_output(self, *args, **kwargs): pass
def source(**kwargs):
    calls.append(kwargs)
    return Clip()
core = types.SimpleNamespace(**{n: types.SimpleNamespace(**{f: source}) for n, f in [('ffms2','Source'), ('bs','VideoSource'), ('lsmas','LWLibavSource'), ('dgdecodenv','DGSource')]})
sys.modules['vapoursynth'] = types.SimpleNamespace(core=core)
exec(compile(open(sys.argv[1]).read(), sys.argv[1], 'exec'), json.loads(sys.argv[2]))
print(json.dumps(calls))
"#]).arg(script_file.path()).arg(serde_json::to_string(variables).expect("script variables")));
    serde_json::from_slice(&output).expect("recorded generated kwargs")
}

#[test]
fn i05_standalone_generators_keep_zero_nonzero_and_default_tracks() {
    for track in [Some(0), Some(1), None] {
        let mut ffms = Source::new(Path::new("source 映像.mkv"));
        ffms.track = track;
        ffms.cachefile = Some("cache 索引".into());
        let mut best = VideoSource::new(&ffms.source);
        best.track = track;
        best.cachepath = ffms.cachefile.clone();
        let mut lsmash = LWLibavSource::new(&ffms.source);
        lsmash.stream_index = track;
        lsmash.cachefile = ffms.cachefile.clone();
        for (plugin, track_key, cache_key) in [
            (&ffms as &dyn VapourSynthPluginScript, "track", "cachefile"),
            (&best, "track", "cachepath"),
            (&lsmash, "stream_index", "cachefile"),
        ] {
            let (_, lines) = plugin.generate_script("clip".to_owned()).expect("source generator");
            let actual = recorded(lines);
            let mut expected =
                serde_json::json!({"source": "source 映像.mkv", cache_key: "cache 索引"});
            if let Some(track) = track {
                expected[track_key] = track.into();
            }
            assert_eq!(actual, serde_json::json!([expected]));
        }
    }
}

#[test]
fn i05_dg_generator_uses_dgi_source_not_unverified_cache_keywords() {
    for alias in [false, true] {
        let mut dg = DGSource::new(Path::new(if alias {
            "original media.mkv"
        } else {
            "chosen index.dgi"
        }));
        if alias {
            dg.indexing_path = Some("chosen index.dgi".into());
        }
        let (_, lines) = dg.generate_script("clip".to_owned()).expect("DG generator");
        assert_eq!(
            recorded(lines),
            serde_json::json!([{"source": "chosen index.dgi"}])
        );
    }
}

#[test]
fn i05_dg_production_routes_plan_existing_index_as_source_without_indexer() {
    let directory = tempfile::tempdir().expect("DG planning fixture, not a native decode test");
    let media = directory.path().join("media 映像.mkv");
    fs::write(&media, b"CPU planning fixture").expect("media placeholder");
    let explicit = directory.path().join("chosen cache 索引.dgi");
    let default = media.with_extension("dgi");
    fs::write(&explicit, b"existing index fixture").expect("explicit index fixture");
    fs::write(&default, b"existing default index fixture").expect("default index fixture");
    let method = VapourSynthImportMethod::DGDecNV {
        dgindexnv_executable: Some(directory.path().join("must-not-run")),
    };
    for (path, cache, expected) in [
        (&media, Some(explicit.as_path()), &explicit),
        (&media, None, &default),
        (&explicit, None, &explicit),
    ] {
        // All opening/reopening routes share this planner, including frame_sources.
        let source =
            source_path(path, &method, cache).expect("reuse existing .dgi; no indexer installed");
        assert_eq!(&source, expected);
        let call = SourceCall::new(&method);
        let (script, variables) = call.script(&source, cache).expect("direct source script");
        let expected = serde_json::json!([{"source": expected.display().to_string()}]);
        assert_eq!(recorded_script(&script, &variables), expected);
        let (_, lines) = source_script(path, &method, cache).expect("generated source route");
        assert_eq!(recorded(lines), expected);
        assert!(
            call.cache_argument.is_none(),
            "additional nodes must not receive an invented cache keyword"
        );
    }
}
