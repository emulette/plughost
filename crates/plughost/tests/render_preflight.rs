//! Render rejection must happen before any fixture processes audio.
use plughost::*;

mod support;

fn options() -> RenderOptions {
    RenderOptions::new(TailPolicy::Reported, 0.0)
}

#[test]
#[ignore = "needs helper and delay fixtures"]
fn unprepared_render_reports_not_prepared_for_both_precisions() {
    for format in [PluginFormat::Vst3, PluginFormat::Clap] {
        let mut chain = support::spawn(&[support::delay(format)]);
        for channels in [0, 2] {
            let input32 = [0.0f32; 32];
            let input64 = [0.0f64; 32];
            assert!(matches!(
                render(
                    &mut chain,
                    &vec![&input32[..]; channels],
                    32,
                    &[],
                    &options()
                ),
                Err(Error::NotPrepared)
            ));
            assert!(matches!(
                render(
                    &mut chain,
                    &vec![&input64[..]; channels],
                    32,
                    &[],
                    &options()
                ),
                Err(Error::NotPrepared)
            ));
        }
        assert!(matches!(
            RenderSession::<f32, _>::new(&mut chain),
            Err(Error::NotPrepared)
        ));
        // Direct preflight calls check preparation and precision as render does.
        let automation = [AutomationEvent {
            slot: 0,
            change: ParameterChange {
                id: 0,
                offset: 0,
                value: 0.5,
            },
        }];
        assert!(matches!(
            Process::<f32>::validate_automation(&mut chain, &automation),
            Err(Error::NotPrepared)
        ));
        let config = chain
            .main_bus_config(48_000.0, 16, Layout::Stereo, &[Layout::Stereo])
            .unwrap();
        chain.prepare_audio(&config).unwrap();
        assert!(matches!(
            Process::<f64>::validate_automation(&mut chain, &automation),
            Err(Error::Input {
                slot: None,
                error: InputError::AudioSampleFormat
            })
        ));
    }
}

#[test]
#[ignore = "needs helper and delay fixtures"]
fn late_invalid_event_port_leaves_dsp_untouched_in_render_and_session() {
    for format in [PluginFormat::Vst3, PluginFormat::Clap] {
        let mut chain = support::spawn(&[support::delay(format)]);
        let config = chain
            .main_bus_config(48_000.0, 16, Layout::Stereo, &[Layout::Stereo])
            .unwrap();
        chain.prepare_audio(&config).unwrap();
        let input = [1.0f32; 32];
        let audio = [&input[..], &input[..]];
        let events = [Event::note_on(24, 0, 60, 100).on_port(config.event_inputs)];
        let invalid_port = |result: Result<(), Error>| {
            assert!(matches!(
                result,
                Err(Error::Input { slot: None, error: InputError::EventPort { port } })
                    if port == config.event_inputs
            ));
        };
        invalid_port(render(&mut chain, &audio, 32, &events, &options()).map(|_| ()));
        {
            let mut session = RenderSession::new(&mut chain).unwrap();
            invalid_port(
                session
                    .push(
                        RenderInput {
                            audio: &audio,
                            frames: 32,
                            events: &events,
                            schedule: RenderSchedule::default(),
                        },
                        |_| panic!("invalid input must not deliver audio"),
                        || false,
                    )
                    .map(|_| ()),
            );
        }
        let clock = if format == PluginFormat::Vst3 { 5 } else { 8 };
        assert_eq!(
            chain
                .parameters(0)
                .unwrap()
                .iter()
                .find(|(p, _)| p.id == clock)
                .unwrap()
                .1,
            0.0,
            "invalid render must not advance the fixture's processing clock"
        );
        // A rejected render must leave the chain ready for a valid block.
        assert_eq!(support::block(&mut chain, 16, 1.0), [0.0; 16]);
    }
}
