#[cfg(test)]
mod tests {
    use andean_condor::models::encoder::{
        Encoder, EncoderBase, EncoderPasses, cli_parameter::CLIParameter,
    };

    #[test]
    fn c04_library_rejects_invalid_settings() {
        for (base, passes, key, value) in [
            (EncoderBase::AOM, EncoderPasses::All(0), "cq-level", 30.0),
            (EncoderBase::AOM, EncoderPasses::All(3), "cq-level", 30.0),
            (
                EncoderBase::AOM,
                EncoderPasses::Specific(0, 2),
                "cq-level",
                30.0,
            ),
            (
                EncoderBase::AOM,
                EncoderPasses::Specific(3, 2),
                "cq-level",
                30.0,
            ),
            (EncoderBase::SVTAV1, EncoderPasses::All(2), "crf", 25.0),
            (EncoderBase::SVTAV1, EncoderPasses::All(1), "rc", 1.5),
            (EncoderBase::SVTAV1, EncoderPasses::All(1), "crf", 70.25),
            (
                EncoderBase::RAV1E,
                EncoderPasses::All(1),
                "quantizer",
                256.0,
            ),
            (
                EncoderBase::X264,
                EncoderPasses::All(1),
                "custom",
                f64::INFINITY,
            ),
            (EncoderBase::X265, EncoderPasses::All(1), "crf", f64::NAN),
        ] {
            let mut encoder = Encoder::default_from_base(&base, false);
            *encoder.passes_mut().expect("passes") = passes;
            encoder
                .parameters_mut()
                .insert(key.into(), CLIParameter::new_number("--", " ", value));
            assert!(
                encoder.validate_settings().is_err(),
                "accepted {base}/{passes:?}/{key}={value}"
            );
        }
    }

    #[test]
    fn c04_known_numeric_strings_and_types_cannot_bypass_validation() {
        for (base, name, values) in [
            (EncoderBase::RAV1E, "speed", vec!["11", "1.5", "fast"]),
            (EncoderBase::SVTAV1, "rc", vec!["3", "1.5", "NaN"]),
            (
                EncoderBase::SVTAV1,
                "crf",
                vec!["-0.25", "70.25", "25.1", "inf"],
            ),
            (EncoderBase::X264, "crf", vec!["-13", "52"]),
            (EncoderBase::X265, "crf", vec!["-1", "52"]),
            (EncoderBase::AVM, "qp", vec!["-97", "256"]),
            (EncoderBase::VVenC, "qp", vec!["-1", "64"]),
            (EncoderBase::AOM, "passes", vec!["0", "3", "1.5"]),
        ] {
            for value in values {
                let mut encoder = Encoder::default_from_base(&base, false);
                encoder
                    .parameters_mut()
                    .insert(name.into(), CLIParameter::new_string("--", " ", value));
                assert!(
                    encoder.validate_settings().is_err(),
                    "accepted {base}/{name}={value}"
                );
            }
            let mut encoder = Encoder::default_from_base(&base, false);
            encoder.parameters_mut().insert(name.into(), CLIParameter::new_bool("--", true));
            assert!(
                encoder.validate_settings().is_err(),
                "accepted boolean {base}/{name}"
            );
        }
    }

    #[test]
    fn c04_pass_pairs_and_valid_modes_are_pure() {
        for base in [
            EncoderBase::AOM,
            EncoderBase::VPX,
            EncoderBase::RAV1E,
            EncoderBase::SVTAV1,
            EncoderBase::AVM,
            EncoderBase::X264,
            EncoderBase::X265,
            EncoderBase::VVenC,
            EncoderBase::FFmpeg,
        ] {
            let mut encoder = Encoder::default_from_base(&base, false);
            encoder.set_executable(Some("/does-not-exist/c04".into()));
            assert!(encoder.validate_settings().is_ok(), "{base} defaults");
            for pass in [(0, 1), (2, 1), (1, 0), (3, 3), (1, 255)] {
                assert!(
                    base.pass_parameters(pass, "stats").is_err(),
                    "{base}/{pass:?}"
                );
            }
            for pass in [(1, 1), (1, 2), (2, 2)] {
                if pass.1 == 2 && matches!(base, EncoderBase::AVM | EncoderBase::FFmpeg) {
                    assert!(base.pass_parameters(pass, "stats").is_err());
                    continue;
                }
                assert!(
                    base.pass_parameters(pass, "stats").is_ok(),
                    "{base}/{pass:?}"
                );
                if base == EncoderBase::SVTAV1 && pass.1 == 2 {
                    encoder.parameters_mut().remove("crf");
                    encoder
                        .parameters_mut()
                        .insert("rc".into(), CLIParameter::new_number("--", " ", 1.0));
                    encoder
                        .parameters_mut()
                        .insert("keyint".into(), CLIParameter::new_number("--", " ", 48.0));
                }
                if let Some(passes) = encoder.passes_mut() {
                    *passes = EncoderPasses::Specific(pass.0, pass.1);
                }
                assert!(encoder.validate_settings().is_ok(), "{base}/{pass:?}");
            }
        }
        for (base, name, values) in [
            (EncoderBase::SVTAV1, "crf", vec![0.0, 0.25, 25.5, 70.0]),
            (EncoderBase::SVTAV1, "qp", vec![0.0, 63.0]),
            (EncoderBase::X264, "crf", vec![-12.0, 25.5, 51.0]),
            (EncoderBase::X264, "qp", vec![0.0, 81.0]),
            (EncoderBase::VVenC, "qp", vec![0.0, 25.5, 63.0]),
            (EncoderBase::AVM, "qp", vec![-96.0, -48.0, 255.0]),
        ] {
            for value in values {
                let mut encoder = Encoder::default_from_base(&base, false);
                encoder
                    .parameters_mut()
                    .insert(name.into(), CLIParameter::new_number("--", " ", value));
                encoder.parameters_mut().insert(
                    "custom-sentinel".into(),
                    CLIParameter::new_number("--", " ", -1.0),
                );
                assert!(encoder.validate_settings().is_ok(), "{base}/{name}={value}");
            }
        }
    }

    #[test]
    fn c04_svt_modes_do_not_depend_on_argument_order() {
        for (name, value) in [("passes", 2.0), ("pass", 1.0), ("pass", 2.0)] {
            let mut encoder = Encoder::default_from_base(&EncoderBase::SVTAV1, false);
            encoder
                .parameters_mut()
                .insert(name.into(), CLIParameter::new_number("--", " ", value));
            assert!(
                encoder.validate_settings().is_err(),
                "CRF with native {name}={value}"
            );
        }
        for (rc, total, preset, pred, valid) in [
            (0.0, 2, 4.0, 2.0, false),
            (1.0, 2, 9.0, 2.0, true),
            (1.0, 2, 10.0, 2.0, false),
            (2.0, 2, 4.0, 1.0, false),
            (2.0, 1, 4.0, 1.0, true),
            (1.0, 1, 13.0, 2.0, true),
        ] {
            let mut encoder = Encoder::default_from_base(&EncoderBase::SVTAV1, false);
            encoder.parameters_mut().remove("crf");
            encoder.parameters_mut().extend(CLIParameter::new_numbers(
                "--",
                " ",
                &[("rc", rc), ("preset", preset), ("pred-struct", pred), ("keyint", 48.0)],
            ));
            *encoder.passes_mut().expect("passes") = EncoderPasses::All(total);
            assert_eq!(
                encoder.validate_settings().is_ok(),
                valid,
                "{rc}/{total}/{preset}/{pred}"
            );
            if rc != 0.0 {
                encoder
                    .parameters_mut()
                    .insert("crf".into(), CLIParameter::new_number("--", " ", 25.0));
                assert!(
                    encoder.validate_settings().is_err(),
                    "contradictory loaded config"
                );
            }
        }
    }

    #[test]
    fn c04_svt_vbr_rejects_explicit_infinite_keyint() {
        for keyint in ["0", "-1", "-2", "1.5"] {
            let mut encoder = Encoder::default_from_base(&EncoderBase::SVTAV1, false);
            encoder.parameters_mut().remove("crf");
            encoder
                .parameters_mut()
                .insert("rc".into(), CLIParameter::new_number("--", " ", 1.0));
            encoder
                .parameters_mut()
                .insert("keyint".into(), CLIParameter::new_string("--", " ", keyint));
            assert!(encoder.validate_settings().is_err(), "VBR keyint {keyint}");
        }
    }

    #[test]
    fn c04_noise_coefficients_and_loaded_passes_are_validated() {
        let json = r#"{"AOM":{"executable":null,"pass":{"Specific":[3,3]},"options":{}}}"#;
        let encoder: Encoder = serde_json::from_str(json).expect("loaded encoder");
        assert!(encoder.validate_settings().is_err());
        let mut encoder = Encoder::default_from_base(&EncoderBase::AOM, false);
        let noise = andean_condor::models::encoder::photon_noise::PhotonNoise {
            iso: 0,
            chroma_iso: Some(0),
            width: None,
            height: None,
            c_y: Some(vec![0; 24]),
            ccb: Some(vec![0; 25]),
            ccr: Some(vec![0; 25]),
        };
        encoder.set_photon_noise(Some(noise.clone()));
        assert!(
            encoder.validate_settings().is_ok(),
            "existing coefficient maxima and ISO zero"
        );
        for channel in ["c_y", "ccb", "ccr"] {
            let mut invalid = noise.clone();
            match channel {
                "c_y" => invalid.c_y.as_mut(),
                "ccb" => invalid.ccb.as_mut(),
                _ => invalid.ccr.as_mut(),
            }
            .expect("coefficients")
            .push(0);
            encoder.set_photon_noise(Some(invalid));
            assert!(encoder.validate_settings().is_err(), "{channel}");
        }
    }
}
