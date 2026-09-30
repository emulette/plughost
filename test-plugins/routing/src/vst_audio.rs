use super::*;
impl IAudioProcessorTrait for Routing {
    unsafe fn setBusArrangements(
        &self,
        inputs: *mut SpeakerArrangement,
        num_inputs: int32,
        outputs: *mut SpeakerArrangement,
        num_outputs: int32,
    ) -> tresult {
        if num_inputs != 2 || num_outputs != 2 {
            return kResultFalse;
        }
        let input = unsafe { std::slice::from_raw_parts(inputs, 2) };
        let output = unsafe { std::slice::from_raw_parts(outputs, 2) };
        if input[0] != SpeakerArr::kMono
            || output[0] != SpeakerArr::kStereo
            || input[1] != output[1]
            || ![
                SpeakerArr::kMono,
                SpeakerArr::kStereo,
                SpeakerArr::k51,
                SpeakerArr::k71Music,
            ]
            .contains(&input[1])
        {
            return kResultFalse;
        }
        *lock(&self.arrangement) = input[1];
        kResultOk
    }
    unsafe fn getBusArrangement(
        &self,
        direction: int32,
        index: int32,
        arrangement: *mut SpeakerArrangement,
    ) -> tresult {
        if !(0..2).contains(&index) {
            return kInvalidArgument;
        }
        unsafe {
            *arrangement = if index == 1 {
                *lock(&self.arrangement)
            } else if direction == BusDirections_::kInput as i32 {
                SpeakerArr::kMono
            } else {
                SpeakerArr::kStereo
            }
        };
        kResultOk
    }
    unsafe fn canProcessSampleSize(&self, size: int32) -> tresult {
        if size == SymbolicSampleSizes_::kSample32 as i32
            || size == SymbolicSampleSizes_::kSample64 as i32
        {
            kResultOk
        } else {
            kResultFalse
        }
    }
    unsafe fn getLatencySamples(&self) -> u32 {
        0
    }
    unsafe fn setupProcessing(&self, setup: *mut ProcessSetup) -> tresult {
        if unsafe { (*setup).sampleRate } == 12345.0 {
            lock(&self.values)[0] = 0.0;
            return kResultFalse;
        }
        kResultOk
    }
    unsafe fn setProcessing(&self, _state: TBool) -> tresult {
        kResultOk
    }
    unsafe fn process(&self, data: *mut ProcessData) -> tresult {
        let data = unsafe { &*data };
        let frames = data.numSamples as usize;
        let initial = *lock(&self.values);
        let mut points = Vec::new();
        if let Some(changes) = unsafe { ComRef::from_raw(data.inputParameterChanges) } {
            for index in 0..unsafe { changes.getParameterCount() } {
                if let Some(queue) = unsafe { ComRef::from_raw(changes.getParameterData(index)) } {
                    let id = unsafe { queue.getParameterId() } as usize;
                    if id >= 2 {
                        continue;
                    }
                    for point in 0..unsafe { queue.getPointCount() } {
                        let (mut offset, mut value) = (0, 0.0);
                        if unsafe { queue.getPoint(point, &mut offset, &mut value) } == kResultOk {
                            points.push((offset.max(0) as usize, id, value));
                            lock(&self.values)[id] = value;
                        }
                    }
                }
            }
        }
        if frames > 0 {
            *lock(&self.gain_points) = points.iter().filter(|(_, id, _)| *id == 0).count();
        }
        let values = crate::frame_values(initial, frames, points);
        unsafe { self.events(data) };
        if frames == 0 {
            return kResultOk;
        }
        if data.numInputs != 2 || data.numOutputs != 2 {
            return kResultFalse;
        }
        let inputs = unsafe { std::slice::from_raw_parts(data.inputs, 2) };
        let outputs = unsafe { std::slice::from_raw_parts_mut(data.outputs, 2) };
        if data.symbolicSampleSize == SymbolicSampleSizes_::kSample64 as i32 {
            unsafe { self.run::<f64>(inputs, outputs, frames, &values, |x| x) }
        } else {
            unsafe { self.run::<f32>(inputs, outputs, frames, &values, |x| x as f32) }
        };
        kResultOk
    }
    unsafe fn getTailSamples(&self) -> u32 {
        0
    }
}
impl Routing {
    unsafe fn run<S: Copy + Into<f64>>(
        &self,
        inputs: &[AudioBusBuffers],
        outputs: &mut [AudioBusBuffers],
        frames: usize,
        values: &[[f64; 2]],
        convert: fn(f64) -> S,
    ) {
        let active = *lock(&self.active);
        let mut captured = Vec::new();
        for (bus, input) in inputs.iter().enumerate() {
            let mut channels = Vec::new();
            for channel in 0..input.numChannels as usize {
                let pointers = unsafe { input.__field0.channelBuffers32 }.cast::<*mut S>();
                let samples = if active[0][bus] && !pointers.is_null() {
                    let p = unsafe { *pointers.add(channel) };
                    if p.is_null() {
                        vec![0.0; frames]
                    } else {
                        unsafe { std::slice::from_raw_parts(p, frames) }
                            .iter()
                            .map(|x| (*x).into())
                            .collect()
                    }
                } else {
                    vec![0.0; frames]
                };
                channels.push(samples);
            }
            captured.push(channels);
        }
        let read = |bus: usize, channel: usize, frame: usize| {
            captured[bus].get(channel).map_or(0.0, |s| s[frame])
        };
        for (bus, output) in outputs.iter_mut().enumerate() {
            let pointers = unsafe { output.__field0.channelBuffers32 }.cast::<*mut S>();
            if pointers.is_null() {
                continue;
            }
            for channel in 0..output.numChannels as usize {
                let ptr = unsafe { *pointers.add(channel) };
                if ptr.is_null() {
                    continue;
                }
                let samples = unsafe { std::slice::from_raw_parts_mut(ptr, frames) };
                for (frame, sample) in samples.iter_mut().enumerate() {
                    let key = read(0, 0, frame);
                    let main = read(1, channel, frame);
                    let value = if !active[1][bus] {
                        0.0
                    } else if bus == 0 {
                        if channel == 0 { read(1, 0, frame) } else { key }
                    } else if values[frame][1] >= 0.5 {
                        main
                    } else {
                        main * values[frame][0] * (1.0 - key.abs().min(1.0))
                    };
                    *sample = convert(value);
                }
            }
            output.silenceFlags = 0;
        }
    }
}
