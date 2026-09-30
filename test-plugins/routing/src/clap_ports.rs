use super::*;
fn channels(config: u32) -> Option<u32> {
    match config {
        100 => Some(1),
        101 => Some(2),
        102 => Some(6),
        103 => Some(8),
        _ => None,
    }
}
fn kind(channels: u32) -> AudioPortType<'static> {
    match channels {
        1 => AudioPortType::MONO,
        2 => AudioPortType::STEREO,
        _ => AudioPortType::SURROUND,
    }
}
fn port(config: u32, index: u32, input: bool, writer: &mut AudioPortInfoWriter) {
    let Some(main) = channels(config) else { return };
    if index >= 2 {
        return;
    }
    let count = if index == 1 {
        main
    } else if input {
        1
    } else {
        2
    };
    writer.set(&AudioPortInfo {
        id: ClapId::new(if input { 20 + index } else { 30 + index }),
        name: if index == 1 {
            b"Main"
        } else if input {
            b"External key"
        } else {
            b"Monitor"
        },
        channel_count: count,
        flags: AudioPortFlags::SUPPORTS_64BITS
            | if index == 1 {
                AudioPortFlags::IS_MAIN
            } else {
                AudioPortFlags::empty()
            },
        port_type: Some(kind(count)),
        in_place_pair: None,
    });
}
impl PluginAudioPortsImpl for MainThread<'_> {
    fn count(&self, _input: bool) -> u32 {
        2
    }
    fn get(&self, index: u32, input: bool, writer: &mut AudioPortInfoWriter) {
        port(*lock(&self.shared.configuration), index, input, writer);
    }
}
impl PluginAudioPortsConfigImpl for MainThread<'_> {
    fn count(&self) -> u32 {
        4
    }
    fn get(&self, index: u32, writer: &mut AudioPortConfigWriter) {
        let Some(count) = channels(index + 100) else {
            return;
        };
        let main = Some(MainPortInfo {
            channel_count: count,
            port_type: Some(kind(count)),
        });
        writer.write(&AudioPortsConfiguration {
            id: ClapId::new(index + 100),
            name: match index {
                0 => b"Mono",
                1 => b"Stereo",
                2 => b"Surround 5.1",
                _ => b"Surround 7.1",
            },
            input_port_count: 2,
            output_port_count: 2,
            main_input: main,
            main_output: main,
        });
    }
    fn select(&self, id: ClapId) -> Result<(), PluginError> {
        if channels(id.get()).is_none() {
            return Err(PluginError::Message(crate::errors::CONFIGURATION));
        }
        *lock(&self.shared.configuration) = id.get();
        *lock(&self.shared.active) = [[true; 2]; 2];
        Ok(())
    }
}
impl PluginAudioPortsConfigInfoImpl for MainThread<'_> {
    fn current_config(&self) -> Option<ClapId> {
        Some(ClapId::new(*lock(&self.shared.configuration)))
    }
    fn get(&self, id: ClapId, index: u32, input: bool, writer: &mut AudioPortInfoWriter) {
        port(id.get(), index, input, writer);
    }
}
impl PluginAudioPortsActivationImpl for MainThread<'_> {
    fn can_activate_while_processing(&self) -> bool {
        false
    }
}
impl PluginAudioPortsActivationSetImpl for MainThread<'_> {
    fn set_active(&self, input: bool, index: u32, active: bool, _sample_size: SampleSize) -> bool {
        activate(self.shared, input, index, active)
    }
}
impl PluginAudioPortsActivationSetImpl for audio::Processor<'_> {
    fn set_active(&self, input: bool, index: u32, active: bool, _sample_size: SampleSize) -> bool {
        activate(self.shared, input, index, active)
    }
}
fn activate(shared: &Shared, input: bool, index: u32, active: bool) -> bool {
    if index >= 2 {
        return false;
    }
    lock(&shared.active)[usize::from(!input)][index as usize] = active;
    true
}
impl PluginSurroundImpl for MainThread<'_> {
    fn is_channel_mask_supported(&self, mask: SurroundChannels) -> bool {
        let surround51 = SurroundChannels::FRONT_LEFT
            | SurroundChannels::FRONT_RIGHT
            | SurroundChannels::FRONT_CENTER
            | SurroundChannels::LOW_FREQUENCY
            | SurroundChannels::BACK_LEFT
            | SurroundChannels::BACK_RIGHT;
        mask == surround51
            || mask == (surround51 | SurroundChannels::SIDE_LEFT | SurroundChannels::SIDE_RIGHT)
    }
    fn get_channel_map(&self, _input: bool, index: u32, writer: &mut SurroundMapWriter) {
        let configuration = *lock(&self.shared.configuration);
        if index == 1 && matches!(configuration, 102 | 103) {
            let map = [
                SurroundChannel::FrontLeft,
                SurroundChannel::FrontRight,
                SurroundChannel::FrontCenter,
                SurroundChannel::LowFrequency,
                SurroundChannel::BackLeft,
                SurroundChannel::BackRight,
                SurroundChannel::SideLeft,
                SurroundChannel::SideRight,
            ];
            writer.set(
                map.into_iter()
                    .take(if configuration == 102 { 6 } else { 8 }),
            );
        }
    }
}
