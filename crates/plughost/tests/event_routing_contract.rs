//! Native event routing through the public chain configuration and processing API.
use plughost::*;

mod support;

fn notes(events: &[Event]) -> Vec<(usize, usize, u8)> {
    events
        .iter()
        .filter_map(|event| {
            let [status, key, velocity] = event.data.to_midi()?;
            (status & 0xf0 == 0x90 && velocity != 0).then_some((event.offset, event.port, key))
        })
        .collect()
}

fn process(chain: &mut Chain, events: &[Event]) -> Vec<Event> {
    let input = [0.0f32; 16];
    let (mut left, mut right) = ([0.0; 16], [0.0; 16]);
    let mut output = Vec::new();
    chain
        .process_audio_f32(
            &BlockContext::new(16),
            &[&input, &input],
            &mut [&mut left, &mut right],
            &[],
            events,
            &mut output,
        )
        .unwrap();
    output
}

#[test]
#[ignore = "needs helper and routing VST3/CLAP fixtures"]
fn main_bus_defaults_carry_events_between_slots_and_stop_at_a_missing_port() {
    for format in [PluginFormat::Vst3, PluginFormat::Clap] {
        let mut chain = support::spawn(&[support::routing(format), support::routing(format)]);
        support::prepare(&mut chain);
        let events = [Event::note_on(2, 0, 40, 127), Event::note_on(5, 0, 45, 127)];
        // Each routing fixture transposes its first native input by one octave.
        assert_eq!(
            notes(&process(&mut chain, &events)),
            [(2, 0, 64), (5, 0, 69)]
        );

        let mut broken = support::spawn(&[
            support::routing(format),
            support::delay(format),
            support::routing(format),
        ]);
        support::prepare(&mut broken);
        assert!(
            process(&mut broken, &events).is_empty(),
            "a slot without event ports breaks the path"
        );
    }
}

#[test]
#[ignore = "needs helper and routing VST3/CLAP fixtures"]
fn merged_native_inputs_order_by_offset_then_route_then_source() {
    for format in [PluginFormat::Vst3, PluginFormat::Clap] {
        let mut chain = support::spawn(&[support::routing(format)]);
        let mut config = support::stereo_config(&mut chain);
        let port = config.slots[0].events.inputs[0].port;
        config.event_inputs = 2;
        config.slots[0].events.inputs = [1, 0]
            .map(|source| EventInputRoute {
                port,
                source: EventSource::External { port: source },
            })
            .into();
        chain.prepare_audio(&config).unwrap();
        let events = [
            Event::note_on(2, 0, 24, 127),
            Event::note_on(5, 0, 25, 127),
            Event::note_on(5, 0, 35, 127).on_port(1),
            Event::note_on(5, 0, 26, 127),
            Event::note_on(5, 0, 36, 127).on_port(1),
        ];
        assert_eq!(
            notes(&process(&mut chain, &events)),
            [(2, 0, 36), (5, 0, 47), (5, 0, 48), (5, 0, 37), (5, 0, 38),],
            "{format:?}"
        );
    }
}

#[test]
#[ignore = "needs helper and routing fixtures"]
fn an_unconfigured_external_port_is_rejected_before_native_processing() {
    for format in [PluginFormat::Vst3, PluginFormat::Clap] {
        let mut chain = support::spawn(&[support::routing(format)]);
        support::prepare(&mut chain);
        let input = [1.0f32; 16];
        let (mut left, mut right) = ([7.0; 16], [7.0; 16]);
        let error = chain
            .process_audio_f32(
                &BlockContext::new(16),
                &[&input, &input],
                &mut [&mut left, &mut right],
                &[],
                &[Event::note_on(0, 0, 40, 127).on_port(1)],
                &mut Vec::new(),
            )
            .unwrap_err();
        assert!(matches!(
            error,
            Error::Input {
                slot: None,
                error: InputError::EventPort { port: 1 }
            }
        ));
        assert_eq!(left, [7.0; 16]);
        assert_eq!(right, left);
        assert_eq!(
            notes(&process(&mut chain, &[Event::note_on(0, 0, 40, 127)])),
            [(0, 0, 52)]
        );
    }
}
