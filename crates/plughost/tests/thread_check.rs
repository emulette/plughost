//! The helper calls each CLAP function on the thread CLAP assigns it, through a real helper
//! process: the thread-check fixture asks the host's thread check in every main-thread and
//! audio-thread function and logs a call on the wrong thread. Build the helper and the test
//! plugins first (see `support`).

use plughost::{
    AutomationEvent, BlockContext, Chain, DiagnosticSeverity, ParameterChange, PluginFormat,
    StatePurpose,
};

mod support;

use support::{delay_variant, prepare, spawn};

const GAIN: u64 = 0;
const WRONG_THREAD: &str = "called on the wrong thread: ";

fn block(chain: &mut Chain, automation: &[AutomationEvent]) {
    let input = [0.25f32; 512];
    let (mut left, mut right) = ([0.0f32; 512], [0.0f32; 512]);
    chain
        .process_audio_f32(
            &BlockContext::new(512),
            &[&input, &input],
            &mut [&mut left, &mut right],
            automation,
            &[],
            &mut Vec::new(),
        )
        .unwrap();
}

fn wrong_threads(chain: &mut Chain) -> Vec<String> {
    let parameters = chain.parameters(0).unwrap();
    let value = |id| parameters.iter().find(|(info, _)| info.id == id).unwrap().1;
    assert_eq!(value(16), 1.0, "the host must provide thread-check");
    assert_eq!(value(17), 1.0, "the host must provide logging");
    assert!(value(18) > 0.0, "the fixture must have performed checks");
    assert_eq!(
        value(19),
        0.0,
        "the fixture found a call on the wrong thread"
    );
    chain
        .take_diagnostics()
        .unwrap()
        .records
        .into_iter()
        .filter(|record| {
            record.severity == DiagnosticSeverity::HostMisbehaving
                && record.message.starts_with(WRONG_THREAD)
        })
        .map(|record| record.message)
        .collect()
}

#[test]
#[ignore = "needs helper and the thread-check fixture (.ps1 or .sh build scripts)"]
fn the_helper_calls_each_clap_function_on_its_thread() {
    let mut chain = spawn(&[delay_variant(PluginFormat::Clap, "thread-check", 0xF)]);
    // Edits reach an inactive plugin with a flush on the main thread.
    chain.set_parameter(0, GAIN, 0.5).unwrap();
    chain.parameters(0).unwrap();
    prepare(&mut chain);
    let automation = [AutomationEvent {
        slot: 0,
        change: ParameterChange {
            id: GAIN,
            offset: 10,
            value: 0.75,
        },
    }];
    block(&mut chain, &automation);
    assert_eq!(wrong_threads(&mut chain), Vec::<String>::new());
    // Edits between blocks reach the active plugin with a flush on its audio thread, before
    // its parameters are read and its state is saved.
    chain.set_parameter(0, GAIN, 0.25).unwrap();
    chain.parameters(0).unwrap();
    chain.set_parameter(0, GAIN, 0.5).unwrap();
    let state = chain.save_state(0, StatePurpose::Project).unwrap();
    chain.reset().unwrap();
    block(&mut chain, &[]);
    assert_eq!(wrong_threads(&mut chain), Vec::<String>::new());
    // Replacing the instance and preparing the chain again stop the old instance's
    // processing and deactivate it.
    chain
        .restore_state(0, &state, StatePurpose::Project)
        .unwrap();
    block(&mut chain, &[]);
    chain.reprepare().unwrap();
    block(&mut chain, &[]);
    chain.take_changes().unwrap();
    assert_eq!(wrong_threads(&mut chain), Vec::<String>::new());
}

#[test]
#[ignore = "needs helper and the thread-check fixture (.ps1 or .sh build scripts)"]
fn the_helper_scans_clap_ports_only_while_the_plugin_is_inactive() {
    // The fixture counts audio and note port scans between its activation and deactivation.
    const ACTIVE_PORT_SCANS: u64 = 20;
    let mut chain = spawn(&[delay_variant(PluginFormat::Clap, "thread-check", 0xF)]);
    let inactive = (
        chain.audio_buses(0).unwrap(),
        chain.event_ports(0).unwrap(),
        chain.capabilities(0).unwrap(),
    );
    prepare(&mut chain);
    block(&mut chain, &[]);
    // An active plugin answers with the ports it was activated with.
    assert_eq!(chain.event_ports(0).unwrap(), inactive.1);
    assert_eq!(chain.capabilities(0).unwrap(), inactive.2);
    let active = chain.audio_buses(0).unwrap();
    assert_eq!(active.len(), inactive.0.len());
    let scans = chain
        .parameters(0)
        .unwrap()
        .into_iter()
        .find(|(info, _)| info.id == ACTIVE_PORT_SCANS)
        .unwrap()
        .1;
    assert_eq!(
        chain
            .parameter_to_plain(0, ACTIVE_PORT_SCANS, scans)
            .unwrap(),
        0.0
    );
}
