//! Minimal fixture-specific chain adapter; it is not a second general routing engine.
use plughost::{HostIdentity, Layout, PluginRef, ProcessMode, SampleFormat};
use plughost_core::render::{Process, Tail};
use plughost_core::{AutomationEvent, BlockContext, MidiEvent, ParameterChange, ProcessConfig};
use plughost_formats::{BlockProcessor, HostedPlugin};

use crate::cases::Case;
use crate::errors::{self, BenchError, Result};
use crate::lifecycle::{self, Lifecycle, StateCost};

pub struct Direct {
    case: Case,
    processors: Vec<Box<dyn BlockProcessor>>,
    buffers: [Vec<Vec<f32>>; 2],
    changes: Vec<ParameterChange>,
}

pub struct Prepared {
    pub owners: Vec<Box<dyn HostedPlugin>>,
    pub processor: Direct,
    pub lifecycle: Lifecycle,
}

/// Keep the owners on the loading thread until the worker has returned and dropped its handles.
pub fn prepare(case: Case, plugin: &PluginRef) -> Result<Prepared> {
    let (mut owners, load) = lifecycle::timed(|| {
        (0..case.plugins)
            .map(|_| plughost_formats::load(plugin, &HostIdentity::default()))
            .collect::<std::result::Result<Vec<_>, _>>()
    })?;
    let ((), prepare) = lifecycle::timed(|| -> Result<()> {
        for plugin in &mut owners {
            if case.multiple() {
                plugin.prepare_audio(&case.native_config())?;
            } else {
                plugin.prepare(&ProcessConfig {
                    sample_rate: case.sample_rate.into(),
                    max_block_size: case.max_block_size,
                    sample_format: SampleFormat::F32,
                    input: Layout::Stereo,
                    output: Layout::Stereo,
                    mode: ProcessMode::Offline,
                })?;
            }
            if plugin.latency()? != 0 || plugin.tail()? != Tail::Samples(0) {
                return Err(errors::fail(errors::TIMING));
            }
        }
        Ok(())
    })?;
    let mut states = Vec::new();
    for plugin in &mut owners {
        let (saved, save) =
            lifecycle::timed(|| plugin.save_state(plughost::StatePurpose::Project))?;
        plugin.set_parameter(0, 0.125)?;
        if plugin.save_state(plughost::StatePurpose::Project)? == saved {
            return Err(errors::fail(errors::STATE));
        }
        let ((), restore) =
            lifecycle::timed(|| plugin.restore_state(&saved, plughost::StatePurpose::Project))?;
        if plugin.save_state(plughost::StatePurpose::Project)? != saved {
            return Err(errors::fail(errors::STATE));
        }
        states.push(StateCost {
            save_seconds: save,
            restore_seconds: restore,
            payload_bytes: saved.component.len() + saved.controller.len(),
        });
    }
    let direct = Direct {
        case,
        processors: owners.iter().map(|p| p.processor()).collect(),
        buffers: std::array::from_fn(|_| vec![vec![0.0; case.max_block_size]; case.outputs()]),
        changes: Vec::with_capacity(1),
    };
    Ok(Prepared {
        owners,
        processor: direct,
        lifecycle: Lifecycle {
            load_seconds: load,
            prepare_seconds: prepare,
            states,
        },
    })
}

impl Process<f32> for Direct {
    type Error = BenchError;
    fn sample_rate(&self) -> f64 {
        self.case.sample_rate.into()
    }
    fn max_block_size(&self) -> usize {
        self.case.max_block_size
    }
    fn input_channels(&self) -> usize {
        self.case.inputs()
    }
    fn output_channels(&self) -> usize {
        self.case.outputs()
    }
    fn latency(&self) -> Result<u32> {
        self.processors
            .iter()
            .try_fold(0, |sum, p| Ok(sum + p.latency()?))
    }
    fn tail(&self) -> Result<Tail> {
        for p in &self.processors {
            if p.tail()? != Tail::Samples(0) {
                return Err(errors::fail(errors::TIMING));
            }
        }
        Ok(Tail::Samples(0))
    }
    fn validate_automation(&mut self, automation: &[AutomationEvent]) -> Result<()> {
        if automation
            .iter()
            .any(|e| e.slot + 1 != self.case.plugins || e.change.id != 0)
        {
            return Err(errors::fail(errors::AUTOMATION));
        }
        Ok(())
    }
    fn process(
        &mut self,
        context: &BlockContext,
        input: &[&[f32]],
        output: &mut [&mut [f32]],
        automation: &[AutomationEvent],
        events: &[MidiEvent],
        produced: &mut Vec<MidiEvent>,
    ) -> Result<()> {
        let frames = context.frames;
        self.changes.clear();
        self.changes
            .extend(automation.iter().map(|event| event.change));
        let main = if self.case.multiple() { 2 } else { 0 };
        let external_main = usize::from(self.case.multiple());
        self.buffers[0][main][..frames].copy_from_slice(input[external_main]);
        self.buffers[0][main + 1][..frames].copy_from_slice(input[external_main + 1]);
        for (slot, processor) in self.processors.iter().enumerate() {
            let [a, b] = &mut self.buffers;
            let changes = if slot + 1 == self.case.plugins {
                self.changes.as_slice()
            } else {
                &[]
            };
            if self.case.multiple() {
                let [monitor, key, left, right] = b.as_mut_slice() else {
                    unreachable!()
                };
                processor.process_audio_f32(
                    context,
                    &[input[0], &a[2][..frames], &a[3][..frames]],
                    &mut [
                        &mut monitor[..frames],
                        &mut key[..frames],
                        &mut left[..frames],
                        &mut right[..frames],
                    ],
                    changes,
                    events,
                    produced,
                )?;
            } else {
                let [left, right] = b.as_mut_slice() else {
                    unreachable!()
                };
                processor.process_audio_f32(
                    context,
                    &[&a[0][..frames], &a[1][..frames]],
                    &mut [&mut left[..frames], &mut right[..frames]],
                    changes,
                    events,
                    produced,
                )?;
            }
            self.buffers.swap(0, 1);
        }
        for (output, samples) in output.iter_mut().zip(&self.buffers[0]) {
            output.copy_from_slice(&samples[..frames]);
        }
        Ok(())
    }
}
