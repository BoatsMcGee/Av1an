use std::{
    path::Path,
    sync::{Arc, atomic::AtomicBool},
};

use anyhow::{Context, Result};
use serde::Serialize;

use crate::{
    core::{
        input::Input,
        output::Output,
        sequence::{SequenceDetails, SequenceStatus, Sequences},
    },
    models::{
        Condor as CondorModel,
        encoder::Encoder,
        scene::Scene,
        sequence::{
            DefaultSequenceConfig,
            DefaultSequenceData,
            SequenceConfigHandler,
            SequenceDataHandler,
        },
    },
};

pub mod encoder;
pub mod input;
pub mod output;
pub mod sequence;
// pub mod scene;

pub type SaveCallback<Data, Config> = Box<dyn Fn(CondorModel<Data, Config>) -> Result<()>>;

pub struct Condor<Data, Config>
where
    Data: SequenceDataHandler,
    Config: SequenceConfigHandler,
{
    pub input:           Input,
    pub output:          Output,
    pub encoder:         Encoder,
    pub scenes:          Vec<Scene<Data>>,
    pub sequence_config: Config,
    pub save_callback:   SaveCallback<Data, Config>,
}

impl<Data, Config> Condor<Data, Config>
where
    Data: SequenceDataHandler,
    Config: SequenceConfigHandler,
{
    #[inline]
    pub fn new(
        input: Input,
        output: Output,
        encoder: Encoder,
        scenes: Vec<Scene<Data>>,
        processor_config: Option<Config>,
        save_callback: SaveCallback<Data, Config>,
    ) -> Self {
        Self {
            input,
            output,
            encoder,
            scenes,
            sequence_config: processor_config.unwrap_or_default(),
            save_callback,
        }
    }

    #[inline]
    pub fn as_data(&self) -> CondorModel<Data, Config> {
        CondorModel {
            input:           self.input.as_data(),
            output:          self.output.as_data(),
            encoder:         self.encoder.clone(),
            scenes:          self.scenes.clone(),
            sequence_config: self.sequence_config.clone(),
        }
    }

    #[inline]
    pub fn save(&self) -> Result<Option<CondorModel<Data, Config>>> {
        let data = CondorModel {
            input:           self.input.as_data(),
            output:          self.output.as_data(),
            encoder:         self.encoder.clone(),
            scenes:          self.scenes.clone(),
            sequence_config: self.sequence_config.clone(),
        };

        (self.save_callback)(data.clone())?;
        // Self::save_data(save_file, &data)?;

        Ok(Some(data))
    }

    #[inline]
    pub fn save_data(save_file: &Path, data: &CondorModel<Data, Config>) -> Result<()> {
        let save_file_temp = save_file.extension().map_or_else(
            || save_file.with_extension("temp"),
            |extension| save_file.with_extension(format!("temp.{}", extension.to_string_lossy())),
        );
        let mut buffer = vec![];
        let formatter = serde_json::ser::PrettyFormatter::with_indent(b"    ");
        let mut serializer = serde_json::Serializer::with_formatter(&mut buffer, formatter);
        data.serialize(&mut serializer)?;
        std::fs::write(&save_file_temp, buffer)?;
        std::fs::rename(&save_file_temp, save_file)?;

        Ok(())
    }
}

pub trait AndeanCondor<Data = DefaultSequenceData, Config = DefaultSequenceConfig>
where
    Data: SequenceDataHandler,
    Config: SequenceConfigHandler,
{
    fn condor(&self) -> &Condor<Data, Config>;
    fn condor_mut(&mut self) -> &mut Condor<Data, Config>;
    fn validate(&mut self) -> Result<((), SequenceWarnings)>;
    fn load(save_file_path: &Path) -> Result<Option<CondorModel<Data, Config>>>;
    fn process_one(
        &mut self,
        processor_index: Option<usize>,
        progress_event_tx: std::sync::mpsc::Sender<SequenceProgressEvent>,
        cancelled: Arc<AtomicBool>,
    ) -> Result<((), SequenceWarningsTuple)>;
    fn process_all(
        &mut self,
        progress_event_tx: std::sync::mpsc::Sender<SequenceProgressEvent>,
        cancelled: Arc<AtomicBool>,
    ) -> Result<Vec<((), SequenceWarningsTuple)>>;
}

pub struct DefaultAndeanCondor<Data = DefaultSequenceData, Config = DefaultSequenceConfig>
where
    Data: SequenceDataHandler,
    Config: SequenceConfigHandler,
{
    pub condor:    Condor<Data, Config>,
    pub sequences: Sequences<Data, Config>,
}

impl AndeanCondor<DefaultSequenceData, DefaultSequenceConfig>
    for DefaultAndeanCondor<DefaultSequenceData, DefaultSequenceConfig>
{
    #[inline]
    fn condor(&self) -> &Condor<DefaultSequenceData, DefaultSequenceConfig> {
        &self.condor
    }

    #[inline]
    fn condor_mut(&mut self) -> &mut Condor<DefaultSequenceData, DefaultSequenceConfig> {
        &mut self.condor
    }

    #[inline]
    fn load(
        save_file_path: &Path,
    ) -> Result<Option<CondorModel<DefaultSequenceData, DefaultSequenceConfig>>> {
        if !save_file_path.exists() {
            return Ok(None);
        }
        let data = std::fs::read_to_string(save_file_path)?;
        let data = serde_json::from_str(&data).with_context(|| {
            format!(
                "Failed to deserialize Condor data from {}",
                save_file_path.display()
            )
        })?;
        Ok(Some(data))
    }

    #[inline]
    fn process_one(
        &mut self,
        processor_index: Option<usize>,
        progress_event_tx: std::sync::mpsc::Sender<SequenceProgressEvent>,
        cancelled: Arc<AtomicBool>,
    ) -> Result<((), SequenceWarningsTuple)> {
        let index = processor_index.unwrap_or(0);
        let sequence_count = self.sequences.len();
        let processor = self.sequences.get_mut(index).ok_or_else(|| {
            anyhow::anyhow!(
                "Sequence index {index} is out of range. There are only {sequence_count} sequences"
            )
        })?;
        let details = processor.details();

        // Processors could have their own Inputs, Outputs, Encoders, and Scenes that
        // should be validated
        let (_, validation_warnings) = processor.validate(&mut self.condor)?;
        // Processors could have their own Inputs that should be initialized
        let (progress_tx, progress_rx) = std::sync::mpsc::channel();
        let event_tx = progress_event_tx.clone();
        let initialization_progress_thread = std::thread::spawn(move || -> Result<()> {
            for progress in progress_rx {
                let event = SequenceProgressEvent {
                    sequence_type: SequenceType::Initialization,
                    index,
                    details,
                    progress,
                };
                event_tx.send(event)?;
            }
            Ok(())
        });
        let (_, initialization_warnings) = processor.initialize(&mut self.condor, progress_tx)?;
        let _ = initialization_progress_thread
            .join()
            .expect("progress event thread should join");
        // Finally, process the Processor (But who processes the process processor?)
        let (progress_tx, progress_rx) = std::sync::mpsc::channel();
        let process_progress_thread = std::thread::spawn(move || -> Result<()> {
            for progress in progress_rx {
                let event = SequenceProgressEvent {
                    sequence_type: SequenceType::Processing,
                    index,
                    details,
                    progress,
                };
                progress_event_tx.send(event)?;
            }
            Ok(())
        });
        let (_, process_warnings) = processor.execute(&mut self.condor, progress_tx, cancelled)?;
        let _ = process_progress_thread.join().expect("progress event thread should join");

        Ok((
            (),
            (
                validation_warnings,
                initialization_warnings,
                process_warnings,
            ),
        ))
    }

    #[inline]
    fn process_all(
        &mut self,
        progress_event_tx: std::sync::mpsc::Sender<SequenceProgressEvent>,
        cancelled: Arc<AtomicBool>,
    ) -> Result<Vec<((), SequenceWarningsTuple)>> {
        let mut processor_warnings_tuples = Vec::with_capacity(self.sequences.len());
        for i in 0..self.sequences.len() {
            if cancelled.load(std::sync::atomic::Ordering::Relaxed) {
                break;
            }
            let progress_event_tx = progress_event_tx.clone();
            let (_, (validation_warnings, initialization_warnings, process_warnings)) =
                self.process_one(Some(i), progress_event_tx, Arc::clone(&cancelled))?;

            processor_warnings_tuples.push((
                (),
                (
                    validation_warnings,
                    initialization_warnings,
                    process_warnings,
                ),
            ));
        }

        Ok(processor_warnings_tuples)
    }

    #[inline]
    fn validate(&mut self) -> Result<((), SequenceWarnings)> {
        let mut warnings = vec![];
        // Validate inputs, outputs, encoders, scenes, and processors

        Input::validate(&self.condor.input.as_data())?;
        Output::validate(&self.condor.output.as_data())?;
        self.condor.encoder.validate()?;
        for scene in &self.condor.scenes {
            scene.encoder.validate()?;
        }

        // Processors could have their own Inputs, Outputs, Encoders, and Scenes that
        // should be validated
        for processor in &mut self.sequences {
            let (_, process_warnings) = processor.validate(&mut self.condor)?;

            warnings.extend(process_warnings);
        }

        Ok(((), warnings))
    }
}

pub type SequenceWarnings = Vec<anyhow::Error>;
pub type SequenceWarningsTuple = (SequenceWarnings, SequenceWarnings, SequenceWarnings);

pub struct SequenceProgressEvent {
    pub sequence_type: SequenceType,
    pub index:         usize,
    pub details:       SequenceDetails,
    pub progress:      SequenceStatus,
}

pub enum SequenceType {
    Validation,
    Initialization,
    Processing,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The temp file is written next to the save file and renamed onto it, so
    /// its name must not collide with the save file itself and must not leave a
    /// stray dot behind.
    #[test]
    fn the_temp_save_file_keeps_the_original_extension() {
        let save_file = Path::new("condor.json");
        let temp = save_file.extension().map_or_else(
            || save_file.with_extension("temp"),
            |extension| save_file.with_extension(format!("temp.{}", extension.to_string_lossy())),
        );

        assert_eq!(temp, Path::new("condor.temp.json"));
        assert_ne!(
            temp, save_file,
            "the temp file must not overwrite the save file"
        );
    }

    /// A save file with no extension still gets a temp file, and one with
    /// several dots keeps only its last extension replaced.
    #[test]
    fn the_temp_save_file_handles_paths_without_or_with_extra_dots() {
        let temp_of = |path: &str| {
            let path = Path::new(path);
            path.extension().map_or_else(
                || path.with_extension("temp"),
                |extension| path.with_extension(format!("temp.{}", extension.to_string_lossy())),
            )
        };

        assert_eq!(temp_of("condor"), Path::new("condor.temp"));
        assert_eq!(temp_of("a.b.json"), Path::new("a.b.temp.json"));
        assert_eq!(
            temp_of("dir.d/condor.json"),
            Path::new("dir.d/condor.temp.json")
        );
    }

    /// A round trip through `save_data` leaves the save file readable and no
    /// temp file behind.
    #[test]
    fn saving_removes_the_temp_file() {
        let directory = tempfile::tempdir().expect("create a temp directory");
        let save_file = directory.path().join("condor.json");
        let data = CondorModel::<DefaultSequenceData, DefaultSequenceConfig> {
            input:           crate::models::input::Input::Video {
                path:          "input.mkv".into(),
                import_method: crate::models::input::ImportMethod::FFMS2 {
                    index:      None,
                    cache_path: None,
                },
                filters:       Vec::new(),
            },
            output:          crate::models::output::Output {
                path:       "output.mkv".into(),
                tags:       std::collections::HashMap::new(),
                video_tags: std::collections::HashMap::new(),
            },
            encoder:         Encoder::default(),
            scenes:          Vec::new(),
            sequence_config: DefaultSequenceConfig::default(),
        };

        Condor::<DefaultSequenceData, DefaultSequenceConfig>::save_data(&save_file, &data)
            .expect("save the data");

        assert!(save_file.exists(), "the save file should exist");
        assert!(
            !directory.path().join("condor.temp.json").exists(),
            "the temp file should have been renamed away"
        );
        assert!(
            !directory.path().join("condor..temp.json").exists(),
            "no double-dot temp file should be left behind"
        );
    }
}
