use plughost::*;
use plughost_core::AudioConfig;
use serde::Serialize;

use crate::errors::{self, Result};

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Path {
    Direct,
    Isolated,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Buses {
    Main,
    Multiple,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Automation {
    None,
    Points,
    Ramp,
}

#[derive(Clone, Copy, Debug, Serialize)]
pub struct Case {
    pub path: Path,
    pub buses: Buses,
    pub automation: Automation,
    pub plugins: usize,
    pub sample_rate: u32,
    pub max_block_size: usize,
    pub frames: usize,
}

pub fn matrix(seconds: f64, blocks: &[usize]) -> Vec<Case> {
    let mut cases = Vec::new();
    for sample_rate in [48_000, 96_000] {
        for &max_block_size in blocks {
            for plugins in [1, 8] {
                for buses in [Buses::Main, Buses::Multiple] {
                    for automation in [Automation::None, Automation::Points, Automation::Ramp] {
                        // Alternate paired path order across conditions; retain it in the report.
                        let paths = if cases.len() % 4 == 0 {
                            [Path::Direct, Path::Isolated]
                        } else {
                            [Path::Isolated, Path::Direct]
                        };
                        for path in paths {
                            cases.push(Case {
                                path,
                                buses,
                                automation,
                                plugins,
                                sample_rate,
                                max_block_size,
                                frames: (seconds * f64::from(sample_rate)).round() as usize,
                            });
                        }
                    }
                }
            }
        }
    }
    cases
}

fn bus(id: u64, layout: Layout) -> AudioBusConfig {
    AudioBusConfig {
        id,
        layout,
        active: true,
    }
}

impl Case {
    pub fn multiple(self) -> bool {
        matches!(self.buses, Buses::Multiple)
    }

    pub fn inputs(self) -> usize {
        if self.multiple() { 3 } else { 2 }
    }
    pub fn outputs(self) -> usize {
        if self.multiple() { 4 } else { 2 }
    }

    pub fn native_config(self) -> AudioConfig {
        AudioConfig {
            events: Default::default(),
            sample_rate: self.sample_rate.into(),
            max_block_size: self.max_block_size,
            sample_format: SampleFormat::F32,
            mode: ProcessMode::Offline,
            configuration: None,
            inputs: vec![bus(0, Layout::Mono), bus(1, Layout::Stereo)],
            outputs: vec![bus(0, Layout::Stereo), bus(1, Layout::Stereo)],
        }
    }

    pub fn routed_config(self) -> RoutedChainConfig {
        RoutedChainConfig {
            event_inputs: 0,
            sample_rate: self.sample_rate.into(),
            max_block_size: self.max_block_size,
            sample_format: SampleFormat::F32,
            mode: ProcessMode::Offline,
            inputs: vec![Layout::Mono, Layout::Stereo],
            slots: (0..self.plugins)
                .map(|slot| SlotAudioConfig {
                    events: Default::default(),
                    configuration: None,
                    inputs: vec![
                        AudioInputRoute {
                            bus: bus(0, Layout::Mono),
                            source: AudioSource::External { bus: 0 },
                            adaptation: ChannelAdaptation::Exact,
                        },
                        AudioInputRoute {
                            bus: bus(1, Layout::Stereo),
                            source: if slot == 0 {
                                AudioSource::External { bus: 1 }
                            } else {
                                AudioSource::Previous { bus: 1 }
                            },
                            adaptation: ChannelAdaptation::Exact,
                        },
                    ],
                    outputs: vec![bus(0, Layout::Stereo), bus(1, Layout::Stereo)],
                })
                .collect(),
        }
    }

    pub fn input(self) -> Vec<Vec<f32>> {
        // A varying key exposes sidechain routing mistakes, including stale block offsets.
        let left: Vec<_> = (0..self.frames)
            .map(|i| 0.2 + (i % 31) as f32 / 100.0)
            .collect();
        let right: Vec<_> = left.iter().map(|x| -0.5 * x).collect();
        if self.multiple() {
            vec![
                (0..self.frames)
                    .map(|i| 0.125 + (i % 17) as f32 / 128.0)
                    .collect(),
                left,
                right,
            ]
        } else {
            vec![left, right]
        }
    }

    pub fn schedule(self) -> (Vec<AutomationEvent>, Vec<AutomationRamp>) {
        match self.automation {
            Automation::None => (Vec::new(), Vec::new()),
            Automation::Points => (
                (0..8)
                    .map(|i| AutomationEvent {
                        slot: self.plugins - 1,
                        change: ParameterChange {
                            id: 0,
                            offset: i * self.frames / 8,
                            value: if i % 2 == 0 { 0.25 } else { 0.75 },
                        },
                    })
                    .collect(),
                Vec::new(),
            ),
            Automation::Ramp => (
                Vec::new(),
                vec![AutomationRamp {
                    slot: self.plugins - 1,
                    id: 0,
                    start: 0,
                    end: self.frames - 1,
                    from: 0.25,
                    to: 0.75,
                }],
            ),
        }
    }

    pub fn verify(
        self,
        input: &[Vec<f32>],
        output: &plughost_core::render::Rendered<f32>,
    ) -> Result<()> {
        if output.latency != 0
            || output.tail != 0
            || output.channels.len() != self.outputs()
            || output.channels.iter().any(|c| c.len() != self.frames)
        {
            return Err(errors::fail(errors::AUDIO));
        }
        for frame in 0..self.frames {
            let gain = match self.automation {
                Automation::None => 1.0,
                Automation::Points => {
                    let index = ((frame + 1) * 8 - 1) / self.frames;
                    if index.is_multiple_of(2) { 0.25 } else { 0.75 }
                }
                Automation::Ramp => 0.25 + 0.5 * frame as f64 / (self.frames - 1) as f64,
            };
            let key = if self.multiple() {
                input[0][frame]
            } else {
                0.0
            };
            let main = usize::from(self.multiple());
            let mut pair = [input[main][frame], input[main + 1][frame]];
            let mut monitor = pair[0];
            for slot in 0..self.plugins {
                monitor = pair[0];
                let gain = if slot + 1 == self.plugins { gain } else { 1.0 };
                for value in &mut pair {
                    *value = (f64::from(*value) * gain * (1.0 - f64::from(key))) as f32;
                }
            }
            let expected = [monitor, key, pair[0], pair[1]];
            let expected = if self.multiple() {
                &expected[..]
            } else {
                &expected[2..]
            };
            for (channel, value) in output.channels.iter().zip(expected) {
                if !channel[frame].is_finite() || (channel[frame] - value).abs() > 2e-6 {
                    return Err(errors::fail(errors::AUDIO));
                }
            }
        }
        Ok(())
    }
}
