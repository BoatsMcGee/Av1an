use std::sync::{mpsc::Sender, Arc, Condvar, Mutex};

use anyhow::Result;
use itertools::Itertools;
use vapoursynth::{
    core::CoreRef,
    map::{OwnedMap, ValueType},
    node::Node,
    plugin::Plugin,
};

use crate::{
    core::sequence::{SequenceStatus, Status},
    utils::semaphore::Semaphore,
    vapoursynth::{get_api, VapourSynthError},
};

pub mod bestsource;
pub mod dgdecodenv;
pub mod ffms2;
// pub mod julek;
pub mod bm3d;
pub mod lsmash;
pub mod rescale;
pub mod resize;
pub mod standard;
pub mod vship;
pub mod vszip;

pub trait PluginFunction {
    const PLUGIN_NAME: &'static str;
    const PLUGIN_ID: &'static str;
    const FUNCTION_NAME: &'static str;
    const REQUIRED_ARGUMENTS: &'static [(&'static str, &'static ValueType)];
    const OPTIONAL_ARGUMENTS: &'static [(&'static str, &'static ValueType)];

    #[inline]
    fn new_error(message: String) -> VapourSynthError {
        VapourSynthError::PluginFunctionError {
            plugin: Self::PLUGIN_NAME.to_owned(),
            function: Self::FUNCTION_NAME.to_owned(),
            message,
        }
    }

    #[inline]
    fn plugin<'core>(core: CoreRef<'core>) -> Result<Plugin<'core>, VapourSynthError> {
        let plugin = core
            .get_plugin_by_id(Self::PLUGIN_ID)
            .map_err(|_| VapourSynthError::PluginNotFound {
                plugin: Self::PLUGIN_ID.to_owned(),
            })?
            .ok_or_else(|| VapourSynthError::PluginLoadError {
                plugin:  Self::PLUGIN_ID.to_owned(),
                message: "Failed to load plugin".to_string(),
            })?;
        Ok(plugin)
    }

    #[inline]
    fn plugin_is_installed<'core>(core: CoreRef<'core>) -> bool {
        Self::plugin(core).is_ok()
    }

    #[inline]
    fn arguments() -> Result<OwnedMap<'static>, VapourSynthError> {
        let api = get_api()?;
        let arguments = OwnedMap::new(api);
        Ok(arguments)
    }

    #[inline]
    fn argument_set_ints<MaybeInt: TryInto<i64>>(
        arguments: &mut OwnedMap,
        values: Vec<(&str, Option<MaybeInt>)>,
    ) -> Result<(), VapourSynthError> {
        for (key, value) in values {
            if value.is_none() {
                continue;
            }
            let number = value.expect("Value is Some").try_into().map_err(|_| {
                VapourSynthError::PluginArgumentsError {
                    plugin:   Self::PLUGIN_NAME.to_owned(),
                    argument: key.to_owned(),
                    message:  "Value is out of range".to_owned(),
                }
            })?;

            arguments
                .set_int(key, number)
                .map_err(|e| VapourSynthError::PluginArgumentsError {
                    plugin:   Self::PLUGIN_NAME.to_owned(),
                    argument: key.to_owned(),
                    message:  e.to_string(),
                })?;
        }
        Ok(())
    }

    #[inline]
    fn argument_set_int_arrays<MaybeInt: TryInto<i64>>(
        arguments: &mut OwnedMap,
        values: Vec<(&str, Option<Vec<MaybeInt>>)>,
    ) -> Result<(), VapourSynthError> {
        for (key, value) in values {
            if value.is_none() {
                continue;
            }
            let numbers = value
                .expect("Value is Some")
                .into_iter()
                .map(|v| {
                    v.try_into().map_err(|_| VapourSynthError::PluginArgumentsError {
                        plugin:   Self::PLUGIN_NAME.to_owned(),
                        argument: key.to_owned(),
                        message:  "Value is out of range".to_owned(),
                    })
                })
                .collect::<Result<Vec<i64>, VapourSynthError>>()?;

            arguments.set_int_array(key, &numbers).map_err(|e| {
                VapourSynthError::PluginArgumentsError {
                    plugin:   Self::PLUGIN_NAME.to_owned(),
                    argument: key.to_owned(),
                    message:  e.to_string(),
                }
            })?;
        }
        Ok(())
    }

    #[inline]
    fn arguments_set_floats<MaybeFloat: TryInto<f64>>(
        arguments: &mut OwnedMap,
        values: Vec<(&str, Option<MaybeFloat>)>,
    ) -> Result<(), VapourSynthError> {
        for (key, value) in values {
            if value.is_none() {
                continue;
            }
            let number = value.expect("Value is Some").try_into().map_err(|_| {
                VapourSynthError::PluginArgumentsError {
                    plugin:   Self::PLUGIN_NAME.to_owned(),
                    argument: key.to_owned(),
                    message:  "Value is out of range".to_owned(),
                }
            })?;

            arguments.set_float(key, number).map_err(|e| {
                VapourSynthError::PluginArgumentsError {
                    plugin:   Self::PLUGIN_NAME.to_owned(),
                    argument: key.to_owned(),
                    message:  e.to_string(),
                }
            })?;
        }
        Ok(())
    }

    #[inline]
    fn arguments_set_float_arrays<MaybeFloat: TryInto<f64>>(
        arguments: &mut OwnedMap,
        values: Vec<(&str, Option<Vec<MaybeFloat>>)>,
    ) -> Result<(), VapourSynthError> {
        for (key, value) in values {
            if value.is_none() {
                continue;
            }
            let numbers = value
                .expect("Value is Some")
                .into_iter()
                .map(|v| {
                    v.try_into().map_err(|_| VapourSynthError::PluginArgumentsError {
                        plugin:   Self::PLUGIN_NAME.to_owned(),
                        argument: key.to_owned(),
                        message:  "Value is out of range".to_owned(),
                    })
                })
                .collect::<Result<Vec<f64>, VapourSynthError>>()?;

            arguments.set_float_array(key, &numbers).map_err(|e| {
                VapourSynthError::PluginArgumentsError {
                    plugin:   Self::PLUGIN_NAME.to_owned(),
                    argument: key.to_owned(),
                    message:  e.to_string(),
                }
            })?;
        }
        Ok(())
    }

    #[inline]
    fn arguments_set<MaybeStringOrBytes: TryInto<Vec<u8>>>(
        arguments: &mut OwnedMap,
        values: Vec<(&str, Option<MaybeStringOrBytes>)>,
    ) -> Result<(), VapourSynthError> {
        for (key, value) in values {
            if value.is_none() {
                continue;
            }
            let string_or_bytes = value.expect("Value is Some").try_into().map_err(|_| {
                VapourSynthError::PluginArgumentsError {
                    plugin:   Self::PLUGIN_NAME.to_owned(),
                    argument: key.to_owned(),
                    message:  "Value is out of range".to_owned(),
                }
            })?;

            arguments.set(key, &string_or_bytes.as_slice()).map_err(|e| {
                VapourSynthError::PluginArgumentsError {
                    plugin:   Self::PLUGIN_NAME.to_owned(),
                    argument: key.to_owned(),
                    message:  e.to_string(),
                }
            })?;
        }
        Ok(())
    }

    #[inline]
    fn validate(arguments: &OwnedMap) -> Result<(), VapourSynthError> {
        let mut keys = arguments.keys();
        for (name, value_type) in Self::REQUIRED_ARGUMENTS {
            if !keys.contains(name) {
                return Err(VapourSynthError::PluginArgumentsError {
                    plugin:   Self::PLUGIN_NAME.to_owned(),
                    argument: (*name).to_string(),
                    message:  "Required argument is missing".to_owned(),
                });
            }
            match arguments.value_type(name) {
                Ok(argument_type) if argument_type != **value_type => {
                    return Err(VapourSynthError::PluginArgumentsError {
                        plugin:   Self::PLUGIN_NAME.to_owned(),
                        argument: (*name).to_string(),
                        message:  "Invalid argument type".to_owned(),
                    });
                },
                _ => (),
            }
        }

        for (name, value_type) in Self::OPTIONAL_ARGUMENTS {
            match arguments.value_type(name) {
                Ok(opt_vt) if opt_vt != **value_type => {
                    return Err(VapourSynthError::PluginArgumentsError {
                        plugin:   Self::PLUGIN_NAME.to_owned(),
                        argument: (*name).to_string(),
                        message:  "Invalid argument type".to_owned(),
                    });
                },
                _ => (),
            }
        }

        Ok(())
    }

    #[inline]
    fn invoke<'core>(
        core: CoreRef<'core>,
        arguments: OwnedMap<'core>,
    ) -> Result<OwnedMap<'core>, VapourSynthError> {
        Self::validate(&arguments)?;
        let plugin = Self::plugin(core)?;
        let result = plugin.invoke(Self::FUNCTION_NAME, &arguments).map_err(|e| {
            VapourSynthError::PluginFunctionError {
                plugin:   Self::PLUGIN_NAME.to_owned(),
                function: Self::FUNCTION_NAME.to_owned(),
                message:  e.to_string(),
            }
        })?;

        if let Some(err) = result.error() {
            return Err(VapourSynthError::PluginFunctionError {
                plugin:   Self::PLUGIN_NAME.to_owned(),
                function: Self::FUNCTION_NAME.to_owned(),
                message:  err.to_string(),
            });
        }

        Ok(result)
    }

    #[inline]
    /// Get a node from the result map and return it. If key is None, uses
    /// "clip" as default.
    fn invoke_and_get_node<'core>(
        core: CoreRef<'core>,
        arguments: OwnedMap<'core>,
        key: Option<&str>,
    ) -> Result<Node<'core>, VapourSynthError> {
        let key = key.unwrap_or("clip");
        let plugin = Self::plugin(core)?;
        let result = plugin.invoke(Self::FUNCTION_NAME, &arguments).map_err(|e| {
            VapourSynthError::PluginFunctionError {
                plugin:   Self::PLUGIN_NAME.to_owned(),
                function: Self::FUNCTION_NAME.to_owned(),
                message:  e.to_string(),
            }
        })?;
        if let Some(err) = result.error() {
            return Err(VapourSynthError::PluginFunctionError {
                plugin:   Self::PLUGIN_NAME.to_owned(),
                function: Self::FUNCTION_NAME.to_owned(),
                message:  err.to_string(),
            });
        }
        let mut result_keys = result.keys();
        if !result_keys.contains(key) {
            return Err(VapourSynthError::PluginFunctionError {
                plugin:   Self::PLUGIN_NAME.to_owned(),
                function: Self::FUNCTION_NAME.to_owned(),
                message:  format!(
                    "Failed to get video node. \"{}\" key not found. Keys found: {}",
                    key,
                    result_keys.join(", ")
                ),
            });
        }

        let node: Node =
            result.get_video_node(key).map_err(|_| VapourSynthError::PluginFunctionError {
                plugin:   Self::PLUGIN_NAME.to_owned(),
                function: Self::FUNCTION_NAME.to_owned(),
                message:  "Failed to get video node".to_owned(),
            })?;

        Ok(node)
    }
}

pub trait MetricPluginFunction: PluginFunction {
    const PROPERTY_NAMES: &'static [&'static str];

    #[inline]
    fn get_scores<'core>(
        node: &Node<'core>,
        property_names: Option<&[&str]>,
        progress_tx: Sender<SequenceStatus>,
    ) -> Result<Vec<f64>, VapourSynthError> {
        let _ = progress_tx.send(SequenceStatus::Whole(Status::Processing {
            id:         Self::FUNCTION_NAME.to_owned(),
            completion: crate::core::sequence::SequenceCompletion::Frames {
                completed: 0,
                total:     node.info().num_frames as u64,
            },
        }));

        let total_frames = node.info().num_frames;
        let scores = Arc::new((Mutex::new(vec![0.0; total_frames]), Condvar::new()));
        let frame_semaphore = Arc::new(Semaphore::new(12));
        let state = Arc::new((Mutex::new(0usize), Condvar::new()));

        // Send progress in the same order as original frames
        let frame_waiter = {
            let state = Arc::clone(&state);

            std::thread::spawn(move || {
                for index in 0..total_frames {
                    let (lock, condvar) = &*state;
                    let mut completed = lock.lock().expect("mutex should acquire lock");
                    while *completed <= index {
                        completed = condvar
                            .wait_while(completed, |c| *c <= index)
                            .expect("Condvar should be notified");
                    }
                    drop(completed);

                    let _ = progress_tx.send(SequenceStatus::Whole(Status::Processing {
                        id:         Self::FUNCTION_NAME.to_owned(),
                        completion: crate::core::sequence::SequenceCompletion::Frames {
                            completed: (index + 1) as u64,
                            total:     total_frames as u64,
                        },
                    }));
                }

                let _ = progress_tx.send(SequenceStatus::Whole(Status::Completed {
                    id: Self::FUNCTION_NAME.to_owned(),
                }));
            })
        };

        // Request all frames asynchronously (up to 12 concurrent)
        for index in 0..total_frames {
            frame_semaphore.acquire();
            let scores_clone = Arc::clone(&scores);
            let state_clone = Arc::clone(&state);
            let frame_semaphore_clone = Arc::clone(&frame_semaphore);

            node.get_frame_async(index, move |frame, _idx, _node| {
                let frame = frame.expect("Failed to get frame");
                let mut score = None;
                for &property_name in property_names.unwrap_or(Self::PROPERTY_NAMES) {
                    if let Ok(s) = frame.props().get_float(property_name) {
                        score = Some(s);
                        break;
                    }
                }
                let score = score.unwrap_or_else(|| {
                    panic!(
                        "Score not found on any of the following properties: {:?}",
                        Self::PROPERTY_NAMES.iter().join(", ")
                    )
                });

                let (scores_lock, _condvar) = &*scores_clone;
                let mut scores_vec = scores_lock.lock().expect("scores mutex should acquire lock");
                scores_vec[index] = score;
                drop(scores_vec);

                let (lock, condvar) = &*state_clone;
                let mut completed = lock.lock().expect("mutex should acquire lock");
                *completed += 1;
                drop(completed);
                condvar.notify_one();
                frame_semaphore_clone.release();
            });
        }

        frame_waiter.join().map_err(|_| VapourSynthError::PluginFunctionError {
            plugin:   Self::PLUGIN_ID.to_owned(),
            function: Self::FUNCTION_NAME.to_owned(),
            message:  "Failed to get scores".to_owned(),
        })?;
        let (lock, _) = &*scores;
        let scores_vec = lock.lock().expect("scores mutex should acquire lock");
        Ok(scores_vec.clone())
    }
}
