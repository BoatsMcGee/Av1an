use std::sync::{self, Arc, atomic::AtomicBool};

use crate::{
    core::{
        Condor,
        input::Input,
        sequence::{Sequence, SequenceDetails, SequenceStatus},
    },
    models::sequence::{
        SequenceConfigHandler,
        SequenceDataHandler,
        quality_check::QualityCheckDataHandler,
        target_quality::types::QualityMetric,
    },
};

static DETAILS: SequenceDetails = SequenceDetails {
    name:        "Quality Check",
    description: "Measure the quality of the video per scene",
    version:     "0.0.1",
};

#[derive(Default)]
pub struct QualityCheck {
    pub metric: QualityMetric,
    pub input:  Option<Input>,
}

impl<Data, Config> Sequence<Data, Config> for QualityCheck
where
    Data: SequenceDataHandler + QualityCheckDataHandler,
    Config: SequenceConfigHandler,
{
    #[inline]
    fn details(&self) -> SequenceDetails {
        DETAILS
    }

    #[inline]
    fn validate(
        &mut self,
        _condor: &mut Condor<Data, Config>,
    ) -> anyhow::Result<((), Vec<anyhow::Error>)> {
        let warnings = vec![];

        Ok(((), warnings))
    }

    #[inline]
    fn initialize(
        &mut self,
        _condor: &mut Condor<Data, Config>,
        _progress_tx: sync::mpsc::Sender<SequenceStatus>,
    ) -> anyhow::Result<((), Vec<anyhow::Error>)> {
        let warnings = vec![];

        Ok(((), warnings))
    }

    #[inline]
    fn execute(
        &mut self,
        _condor: &mut Condor<Data, Config>,
        _progress_tx: sync::mpsc::Sender<SequenceStatus>,
        _cancelled: Arc<AtomicBool>,
    ) -> anyhow::Result<((), Vec<anyhow::Error>)> {
        let warnings = vec![];

        Ok(((), warnings))
    }
}

impl QualityCheck {
    pub const DETAILS: SequenceDetails = DETAILS;
}
