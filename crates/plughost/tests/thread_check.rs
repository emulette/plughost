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
