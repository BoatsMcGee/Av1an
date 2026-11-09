#[path = "common.rs"]
mod common;

#[cfg(test)]
mod tests {
    use super::common::condor_cmd;
    use andean_condor::models::input::{ImportMethod, Input};
    use california_condor::configuration::Configuration;

    fn rejects_without_saving(filter: &str, field: &str) {
        let temp = tempfile::tempdir().expect("filter CLI fixture");
        let script = temp.path().join("blank.vpy");
        std::fs::write(&script, "from vapoursynth import core, YUV420P8\ncore.std.BlankClip(width=32, height=16, format=YUV420P8).set_output()\n").expect("blank script");
        let mut config = Configuration::new(
            &script,
            &temp.path().join("out.mkv"),
            Some(temp.path()),
            None,
            None,
        )
        .expect("seed config");
        let source = temp.path().join("must-not-be-decoded.mkv");
        std::fs::write(&source, b"not video; parser must fail before decoding").expect("source");
        config.condor.input = Input::Video {
            path: source,
            import_method: ImportMethod::FFMS2 {
                index: Some(0),
                cache_path: Some(temp.path().join("must-not-exist.ffindex")),
            },
            filters: Vec::new(),
        };
        let path = temp.path().join("condor.json");
        config.save(&path).expect("seed config");
        let before = std::fs::read(&path).expect("snapshot");
        // Exercise typed Clap flags and the CRUD string-list parser, including a valid first item.
        let outputs = [
            vec!["encode", "--filters", "crop:top=0", "--filters", filter],
            vec!["init", "set", "input", "--filters", "crop:top=0", "--filters", filter],
            vec!["detect-noise", "set", "reference-filters", "crop:top=0", filter],
        ]
        .map(|args| {
            let output = condor_cmd(&temp).args(args).output().expect("run real condor");
            let saved = std::fs::read(&path).expect("config after command");
            (output, saved)
        });
        for (output, saved) in outputs {
            let error = String::from_utf8_lossy(&output.stderr);
            assert!(!output.status.success(), "{filter}: {output:?}");
            assert!(
                error.contains(filter.split(':').next().expect("name")) && error.contains(field),
                "{filter}: {error}"
            );
            if let Some(value) = filter.strip_prefix("zoom_degrain:plane=") {
                assert!(
                    error.contains(value) && error.contains("0..=4"),
                    "{filter}: {error}"
                );
            }
            for forbidden in ["panicked", "backtrace", "index out of bounds", "Failed to open"] {
                assert!(!error.contains(forbidden), "{filter}: {error}");
            }
            assert!(
                saved == before,
                "{filter}: failed parse changed the saved config"
            );
        }
        assert!(!temp.path().join("must-not-exist.ffindex").exists());
        assert!(!temp.path().join("condor.temp").exists());
        let mut entries = std::fs::read_dir(temp.path())
            .expect("directory")
            .map(|entry| entry.expect("entry").file_name().to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        entries.sort();
        assert_eq!(
            entries,
            ["blank.vpy", "condor.json", "logs", "must-not-be-decoded.mkv"]
        );
    }

    macro_rules! cli_error {
        ($name:ident, $filter:expr, $field:expr) => {
            #[test]
            fn $name() {
                rejects_without_saving($filter, $field);
            }
        };
    }

    cli_error!(i07_cli_crop, "crop:top=abc", "top");
    cli_error!(i07_cli_resize, "resize:width=0", "width");
    cli_error!(i07_cli_trim, "trim:start=10;end=5", "end");
    cli_error!(
        i07_cli_rescale,
        "rescale:width=1280;height=720;doubler=ArtCNN",
        "kernel"
    );
    cli_error!(i07_cli_wnnm, "wnnm:sigma=1,abc", "sigma");
    cli_error!(i07_cli_bilateral, "bilateral:planes=true,maybe", "planes");
    cli_error!(i07_cli_degrain, "degrain:recalculate=maybe", "recalculate");
    cli_error!(i07_cli_zoom_degrain, "zoom_degrain:hpad=abc", "hpad");
    cli_error!(
        i07_cli_zoom_plane_negative,
        "zoom_degrain:plane=-1",
        "plane"
    );
    cli_error!(
        i07_cli_zoom_plane_above_range,
        "zoom_degrain:plane=5",
        "plane"
    );
    cli_error!(i07_cli_fgs, "fgs:", "iso");
    cli_error!(i07_cli_output_format, "output-format:typo=1", "typo");
    cli_error!(i07_cli_missing_delimiter, "crop", ":");
}
