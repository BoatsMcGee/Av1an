use std::{
    iter::repeat_with,
    mem,
    sync::{
        Arc,
        Condvar,
        Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::Sender,
    },
};

use anyhow::Result;
use itertools::Itertools;
use vapoursynth::{
    core::CoreRef,
    frame::FrameRef,
    map::{OwnedMap, ValueType},
    node::Node,
    plugin::Plugin as VapourSynthPlugin,
};

use crate::{
    core::sequence::{SequenceCompletion, SequenceStatus, Status},
    utils::semaphore::Semaphore,
    vapoursynth::{VapourSynthError, get_api},
};

pub mod bestsource;
pub mod bm3d;
pub mod dgdecodenv;
pub mod ffms2;
pub mod fgs;
pub mod lsmash;
pub mod mvutensils;
pub mod rescale;
pub mod resize;
pub mod standard;
pub mod vship;
pub mod vszip;
pub mod zoomvtools;

pub struct VapourSynthPluginInfo {
    pub name:      &'static str,
    pub id:        &'static str,
    pub docs:      Option<&'static str>,
    // pub version:   String, // vapoursynth-rs does not expose `getPluginVersion()`
    pub installed: bool,
}

/// A single argument of a plugin function, parsed from the plugin's
/// registered argument specification string (`name:type[:opt];...`, e.g.
/// `clip:vnode;fgs_data:data;dynamic_seed:int:opt;`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginArgument {
    /// Argument name, e.g. `clip`.
    pub name:       String,
    /// Argument value type, e.g. `vnode` or `int`.
    pub value_type: String,
    /// `true` when the argument is marked optional (`:opt`).
    pub optional:   bool,
}

impl PluginArgument {
    /// Parse a single `name:type[:opt]` segment into a [`PluginArgument`].
    #[inline]
    pub fn parse(segment: &str) -> Option<PluginArgument> {
        let mut parts = segment.split(':');
        let name = parts.next()?.to_owned();
        let value_type = parts.next()?.to_owned();
        let optional = parts.next() == Some("opt");
        Some(PluginArgument {
            name,
            value_type,
            optional,
        })
    }
}

pub trait Plugin {
    const PLUGIN_NAME: &'static str;
    const PLUGIN_ID: &'static str;
    const PLUGIN_DOCS: Option<&'static str> = None;
}

pub trait PluginFunction
where
    Self: Plugin,
{
    const FUNCTION_NAME: &'static str;
    const FUNCTION_DOCS: Option<&'static str> = None;
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
    fn info<'core>(core: CoreRef<'core>) -> Result<VapourSynthPluginInfo> {
        Ok(VapourSynthPluginInfo {
            name:      Self::PLUGIN_NAME,
            id:        Self::PLUGIN_ID,
            docs:      Self::PLUGIN_DOCS,
            installed: Self::plugin_is_installed(core),
        })
    }

    /// Query the plugin's registered function and return its parsed argument
    /// list. This reflects the actual plugin build loaded at runtime, so
    /// callers can adapt to version differences (e.g. arguments added in
    /// newer plugin releases).
    #[inline]
    fn plugin_function_arguments<'core>(
        core: CoreRef<'core>,
    ) -> Result<Vec<PluginArgument>, VapourSynthError> {
        let plugin = Self::plugin(core)?;
        let Some(plugin_function) = plugin
            .get_plugin_function_by_name(Self::FUNCTION_NAME)
            .map_err(|e| VapourSynthError::Internal {
                message: format!("Failed to query function {}: {e}", Self::FUNCTION_NAME),
            })?
        else {
            return Err(VapourSynthError::PluginFunctionNotFound {
                plugin:   Self::PLUGIN_NAME.to_owned(),
                function: Self::FUNCTION_NAME.to_owned(),
            });
        };

        let arguments = plugin_function.arguments().to_string_lossy();

        Ok(arguments
            .split(';')
            .filter(|part| !part.is_empty())
            .filter_map(PluginArgument::parse)
            .collect())
    }

    #[inline]
    fn plugin<'core>(core: CoreRef<'core>) -> Result<VapourSynthPlugin<'core>, VapourSynthError> {
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
    /// Set a single signed integer argument.
    fn argument_set_int(
        arguments: &mut OwnedMap,
        key: &str,
        value: Option<i64>,
    ) -> Result<(), VapourSynthError> {
        if let Some(number) = value {
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

    #[inline]
    /// Invoke the plugin function and retrieve an array of nodes from the
    /// result map. This is used by functions such as MVUtensils' `AnalyseMany`
    /// that return multiple video nodes under the same key rather than a single
    /// node.
    fn invoke_and_get_node_array<'core>(
        core: CoreRef<'core>,
        arguments: OwnedMap<'core>,
        key: &str,
    ) -> Result<Vec<Node<'core>>, VapourSynthError> {
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

        let nodes = result
            .get_video_node_iter(key)
            .map_err(|_| VapourSynthError::PluginFunctionError {
                plugin:   Self::PLUGIN_NAME.to_owned(),
                function: Self::FUNCTION_NAME.to_owned(),
                message:  format!("Failed to get video nodes for key \"{}\"", key),
            })?
            .collect::<Vec<_>>();
        Ok(nodes)
    }
}

pub trait MetricPluginFunction: PluginFunction {
    const PROPERTY_NAMES: &'static [&'static str];

    /// Asynchronously retrieve frames, extracting the value and reporting the frame score as they are computed
    #[inline]
    fn collect_frame_values<'core, Value, Extract, OnFrame>(
        node: &Node<'core>,
        progress_tx: Sender<SequenceStatus>,
        on_frame: OnFrame,
        extract: Extract,
    ) -> Result<Vec<Value>, VapourSynthError>
    where
        Value: Send + 'core,
        Extract: Fn(&FrameRef<'core>) -> Result<Value, VapourSynthError> + Send + Sync + 'core,
        OnFrame: Fn(usize, &Value) -> Result<(), VapourSynthError> + Send + Sync + 'core,
    {
        let concurrency = std::thread::available_parallelism().map_or(12, |n| n.get());
        let total_frames = node.info().num_frames;

        let _ = progress_tx.send(SequenceStatus::Whole(Status::Processing {
            id:         Self::FUNCTION_NAME.to_owned(),
            completion: SequenceCompletion::Frames {
                completed: 0,
                total:     total_frames as u64,
            },
        }));

        let extract = Arc::new(extract);
        let on_frame = Arc::new(on_frame);
        // `Value` is only known to be `Send`, so the slots cannot be preallocated
        // with a default value the way a `Vec<f64>` could be.
        let values: Arc<Mutex<Vec<Option<Value>>>> = Arc::new(Mutex::new(
            repeat_with(|| None).take(total_frames).collect(),
        ));
        let frame_semaphore = Arc::new(Semaphore::new(concurrency));
        let state = Arc::new((Mutex::new(0usize), Condvar::new()));
        // First error reported by any frame callback. Once set, no further frame
        // requests are issued and this error is returned.
        let first_error = Arc::new(Mutex::new(None::<VapourSynthError>));
        let aborted = Arc::new(AtomicBool::new(false));

        // Send progress in the same order as original frames
        let frame_waiter = {
            let state = Arc::clone(&state);
            let aborted = Arc::clone(&aborted);

            std::thread::spawn(move || {
                for index in 0..total_frames {
                    let (lock, condvar) = &*state;
                    let mut completed = lock.lock().expect("mutex should acquire lock");
                    while *completed <= index && !aborted.load(Ordering::Relaxed) {
                        completed = condvar
                            .wait_while(completed, |c| {
                                *c <= index && !aborted.load(Ordering::Relaxed)
                            })
                            .expect("Condvar should be notified");
                    }
                    drop(completed);

                    if aborted.load(Ordering::Relaxed) {
                        return;
                    }

                    let _ = progress_tx.send(SequenceStatus::Whole(Status::Processing {
                        id:         Self::FUNCTION_NAME.to_owned(),
                        completion: SequenceCompletion::Frames {
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

        // Request frames asynchronously
        for index in 0..total_frames {
            if aborted.load(Ordering::Relaxed) {
                break;
            }

            frame_semaphore.acquire();
            let values_clone = Arc::clone(&values);
            let state_clone = Arc::clone(&state);
            let frame_semaphore_clone = Arc::clone(&frame_semaphore);
            let first_error_clone = Arc::clone(&first_error);
            let aborted_clone = Arc::clone(&aborted);
            let extract_clone = Arc::clone(&extract);
            let on_frame_clone = Arc::clone(&on_frame);

            node.get_frame_async(index, move |frame, _idx, _node| {
                let value = match frame {
                    Ok(frame) => extract_clone(&frame),
                    Err(error) => Err(Self::new_error(format!(
                        "Failed to get frame {index}: {error}"
                    ))),
                };

                match value {
                    Ok(value) => {
                        if let Err(error) = on_frame_clone(index, &value) {
                            let mut first_error =
                                first_error_clone.lock().expect("error mutex should acquire lock");
                            // Keep the first error encountered.
                            if first_error.is_none() {
                                *first_error = Some(error);
                            }
                            drop(first_error);
                            aborted_clone.store(true, Ordering::Relaxed);
                        } else {
                            let mut values_vec =
                                values_clone.lock().expect("values mutex should acquire lock");
                            values_vec[index] = Some(value);
                            drop(values_vec);
                        }
                    },
                    Err(error) => {
                        let mut first_error =
                            first_error_clone.lock().expect("error mutex should acquire lock");
                        // Keep the first error encountered.
                        if first_error.is_none() {
                            *first_error = Some(error);
                        }
                        drop(first_error);
                        aborted_clone.store(true, Ordering::Relaxed);
                    },
                }

                let (lock, condvar) = &*state_clone;
                let mut completed = lock.lock().expect("mutex should acquire lock");
                *completed += 1;
                drop(completed);
                condvar.notify_all();
                frame_semaphore_clone.release();
            });
        }

        // Notify progress thread in case it is waiting for an aborted frame
        let (_, state_condvar) = &*state;
        state_condvar.notify_all();

        // Wait for all ongoing callbacks
        for _ in 0..concurrency {
            frame_semaphore.acquire();
        }

        frame_waiter
            .join()
            .map_err(|_| Self::new_error("Failed to get frame values".to_owned()))?;

        let first_error = first_error.lock().expect("error mutex should acquire lock").take();
        if let Some(error) = first_error {
            return Err(error);
        }

        let mut values_guard = values.lock().expect("values mutex should acquire lock");
        let values_vec = mem::take(&mut *values_guard);
        drop(values_guard);

        values_vec
            .into_iter()
            .enumerate()
            .map(|(index, value)| {
                value.ok_or_else(|| Self::new_error(format!("Missing value for frame {index}")))
            })
            .collect()
    }

    /// Get a single score per frame, taking the first property in
    /// `property_names` that is present on the frame.
    ///
    /// `property_names` defaults to [`Self::PROPERTY_NAMES`] and is treated as
    /// a list of candidates: plugins may expose the same score under more
    /// than one name (for example `SSIMULACRA2` and `_SSIMULACRA2`). It is
    /// only an error when a frame carries none of them.
    #[inline]
    fn get_scores<'core>(
        node: &Node<'core>,
        property_names: Option<&'core [&'core str]>,
        progress_tx: Sender<SequenceStatus>,
    ) -> Result<Vec<f64>, VapourSynthError> {
        let property_names = property_names.unwrap_or(Self::PROPERTY_NAMES);

        Self::collect_frame_values(node, progress_tx, |_index, _value| Ok(()), move |frame| {
            property_names
                .iter()
                .find_map(|property_name| frame.props().get_float(property_name).ok())
                .ok_or_else(|| {
                    Self::new_error(format!(
                        "Score not found on any of the following properties: {}",
                        property_names.iter().join(", ")
                    ))
                })
        })
    }

    /// Get several scores per frame, one for each name in `property_names`.
    ///
    /// Unlike [`Self::get_scores`], every property is required: a frame missing
    /// any of them is an error. The outer `Vec` is indexed by frame and each
    /// inner `Vec` holds that frame's scores in `property_names` order.
    #[inline]
    fn get_multiple_scores<'core>(
        node: &Node<'core>,
        property_names: &'core [&'core str],
        progress_tx: Sender<SequenceStatus>,
    ) -> Result<Vec<Vec<f64>>, VapourSynthError> {
        Self::collect_frame_values(node, progress_tx, |_index, _value| Ok(()), move |frame| {
            property_names
                .iter()
                .map(|property_name| {
                    frame.props().get_float(property_name).map_err(|error| {
                        Self::new_error(format!(
                            "Score not found on required property \"{property_name}\": {error}"
                        ))
                    })
                })
                .collect()
        })
    }
}
