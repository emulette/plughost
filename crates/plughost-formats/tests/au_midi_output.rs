//! Audio Unit MIDI output, from an AUv3 MIDI processor this test registers in its own process.
//! The unit sends back the MIDI it receives on its first output, and on request several messages
//! in one call, a message on its second output, or more events than a block allows.

#![cfg(target_os = "macos")]

use std::cell::OnceCell;
use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::{Arc, Mutex, Once};

use block2::{DynBlock, RcBlock};
use objc2::rc::{Allocated, Retained};
use objc2::runtime::Bool;
use objc2::{AnyThread, ClassType, DefinedClass, define_class, msg_send};
use objc2_audio_toolbox::{
    AUAudioUnit, AUAudioUnitBus, AUAudioUnitBusArray, AUAudioUnitBusType, AUEventSampleTime,
    AudioComponentDescription, AudioComponentInstantiationOptions, kAudioUnitType_MIDIProcessor,
};
use objc2_avf_audio::AVAudioFormat;
use objc2_core_audio_types::AudioBufferList;
use objc2_foundation::{NSArray, NSError, NSInteger, NSString};
use plughost_core::{
    AudioBusConfig, AudioConfig, AudioDirection, EventConfig, Layout, MAX_BLOCK_EVENTS, MidiEvent,
    ProcessMode, SampleFormat,
};
use plughost_formats::au::{self, Plugin};
use plughost_formats::{Error, HostedPlugin};

type OutputEvent = dyn Fn(AUEventSampleTime, u8, NSInteger, NonNull<u8>) -> i32;
type InternalRender = dyn Fn(
    *mut c_void,
    *const c_void,
    u32,
    NSInteger,
    *mut AudioBufferList,
    *const c_void,
    *mut c_void,
) -> i32;

/// AUMIDIEvent, which begins like every AURenderEvent. System exclusive bytes continue past
/// `data`.
#[repr(C)]
struct RenderEvent {
    next: *const RenderEvent,
    time: AUEventSampleTime,
    kind: u8,
    reserved: u8,
    length: u16,
    cable: u8,
    data: [u8; 3],
}
const MIDI: u8 = 8;
const SYSEX: u8 = 9;

#[derive(Default)]
struct Ivars {
    inputs: OnceCell<Retained<AUAudioUnitBusArray>>,
    outputs: OnceCell<Retained<AUAudioUnitBusArray>>,
    send: Arc<Mutex<Option<RcBlock<OutputEvent>>>>,
    render: OnceCell<RcBlock<InternalRender>>,
}

/// Answers one received message.
fn respond(send: &RcBlock<OutputEvent>, time: AUEventSampleTime, bytes: &[u8]) {
    let out = |cable: u8, bytes: &[u8]| {
        send.call((
            time,
            cable,
            bytes.len() as NSInteger,
            NonNull::from(bytes).cast(),
        ));
    };
    match bytes {
        [0x90, 127, _] => {
            for _ in 0..=MAX_BLOCK_EVENTS {
                out(0, &[0x90, 1, 1]);
            }
        }
        // Two notes under running status, a clock and a system exclusive message.
        [0xB0, 127, _] => out(0, &[0x90, 1, 2, 3, 4, 0xF8, 0xF0, 0x7D, 0x01, 0xF7]),
        [0xB0, 126, value] => out(1, &[0xB0, 126, *value]),
        _ => out(0, bytes),
    }
}

define_class!(
    #[unsafe(super(AUAudioUnit))]
    #[ivars = Ivars]
    struct EchoUnit;

    impl EchoUnit {
        #[unsafe(method_id(initWithComponentDescription:options:error:))]
        fn init(
            this: Allocated<Self>,
            description: AudioComponentDescription,
            options: AudioComponentInstantiationOptions,
            error: *mut *mut NSError,
        ) -> Option<Retained<Self>> {
            Self::build(this, description, options, error)
        }

        #[unsafe(method_id(inputBusses))]
        fn input_busses(&self) -> Retained<AUAudioUnitBusArray> {
            self.ivars().inputs.get().unwrap().clone()
        }

        #[unsafe(method_id(outputBusses))]
        fn output_busses(&self) -> Retained<AUAudioUnitBusArray> {
            self.ivars().outputs.get().unwrap().clone()
        }

        #[unsafe(method_id(MIDIOutputNames))]
        fn midi_output_names(&self) -> Retained<NSArray<NSString>> {
            NSArray::from_retained_slice(&[NSString::from_str("Echo"), NSString::from_str("Second")])
        }

        #[unsafe(method(allocateRenderResourcesAndReturnError:))]
        fn allocate(&self, error: *mut *mut NSError) -> Bool {
            let allocated: Bool =
                unsafe { msg_send![super(self), allocateRenderResourcesAndReturnError: error] };
            *self.ivars().send.lock().unwrap() =
                unsafe { RcBlock::copy(self.MIDIOutputEventBlock()) };
            allocated
        }

        #[unsafe(method(internalRenderBlock))]
        fn internal_render_block(&self) -> *mut DynBlock<InternalRender> {
            let send = Arc::clone(&self.ivars().send);
            let render = self.ivars().render.get_or_init(|| {
                RcBlock::new(
                    move |_: *mut c_void,
                          _: *const c_void,
                          _: u32,
                          _: NSInteger,
                          output: *mut AudioBufferList,
                          events: *const c_void,
                          _: *mut c_void| {
                        // SAFETY: the host passes its buffer list, with every buffer it declares.
                        let buffers = unsafe {
                            std::slice::from_raw_parts_mut(
                                (*output).mBuffers.as_mut_ptr(),
                                (*output).mNumberBuffers as usize,
                            )
                        };
                        for buffer in buffers.iter().filter(|buffer| !buffer.mData.is_null()) {
                            unsafe {
                                std::ptr::write_bytes(
                                    buffer.mData.cast::<u8>(),
                                    0,
                                    buffer.mDataByteSize as usize,
                                )
                            };
                        }
                        let send = send.lock().unwrap();
                        let Some(send) = send.as_ref() else {
                            return 0;
                        };
                        let mut event = events.cast::<RenderEvent>();
                        // SAFETY: the list and its MIDI bytes are valid during this call.
                        while let Some(current) = unsafe { event.as_ref() } {
                            if matches!(current.kind, MIDI | SYSEX) {
                                let bytes = unsafe {
                                    std::slice::from_raw_parts(
                                        current.data.as_ptr(),
                                        usize::from(current.length),
                                    )
                                };
                                respond(send, current.time, bytes);
                            }
                            event = current.next;
                        }
                        0
                    },
                )
            });
            RcBlock::as_ptr(render)
        }
    }
);

impl EchoUnit {
    fn build(
        this: Allocated<Self>,
        description: AudioComponentDescription,
        options: AudioComponentInstantiationOptions,
        error: *mut *mut NSError,
    ) -> Option<Retained<Self>> {
        let this = this.set_ivars(Ivars::default());
        let this: Option<Retained<Self>> = unsafe {
            msg_send![super(this), initWithComponentDescription: description, options: options, error: error]
        };
        let this = this?;
        let format = unsafe {
            AVAudioFormat::initStandardFormatWithSampleRate_channels(
                AVAudioFormat::alloc(),
                48_000.0,
                2,
            )
        }?;
        let bus: Result<Retained<AUAudioUnitBus>, Retained<NSError>> =
            unsafe { msg_send![AUAudioUnitBus::alloc(), initWithFormat: &*format, error: _] };
        let array = |kind, buses: &[Retained<AUAudioUnitBus>]| unsafe {
            AUAudioUnitBusArray::initWithAudioUnit_busType_busses(
                AUAudioUnitBusArray::alloc(),
                &this,
                kind,
                &NSArray::from_retained_slice(buses),
            )
        };
        let _ = this
            .ivars()
            .inputs
            .set(array(AUAudioUnitBusType::Input, &[]));
        let _ = this
            .ivars()
            .outputs
            .set(array(AUAudioUnitBusType::Output, &[bus.ok()?]));
        Some(this)
    }
}

/// Registers the unit once and returns its class ID.
fn class_id() -> String {
    static REGISTER: Once = Once::new();
    REGISTER.call_once(|| unsafe {
        AUAudioUnit::registerSubclass_asComponentDescription_name_version(
            EchoUnit::class(),
            AudioComponentDescription {
                componentType: kAudioUnitType_MIDIProcessor,
                componentSubType: u32::from_be_bytes(*b"echo"),
                componentManufacturer: u32::from_be_bytes(*b"Plgh"),
                componentFlags: 0,
                componentFlagsMask: 0,
            },
            &NSString::from_str("plughost: MIDI echo"),
            1,
        );
    });
    au::components()
        .into_iter()
        .find(|component| component.name == "MIDI echo")
        .unwrap()
        .class_id
}

/// The unit's stereo output with MIDI input and the given MIDI outputs.
fn config(plugin: &mut Plugin, outputs: Vec<u64>) -> AudioConfig {
    let output = plugin
        .audio_buses()
        .unwrap()
        .into_iter()
        .find(|bus| bus.direction == AudioDirection::Output)
        .unwrap();
    AudioConfig {
        sample_rate: 48_000.0,
        max_block_size: 512,
        sample_format: SampleFormat::F32,
        mode: ProcessMode::Offline,
        configuration: None,
        inputs: Vec::new(),
        outputs: vec![AudioBusConfig {
            id: output.id,
            layout: Layout::Stereo,
            active: true,
        }],
        events: EventConfig {
            inputs: vec![0],
            outputs,
        },
    }
}

fn prepared(outputs: Vec<u64>) -> Plugin {
    let mut plugin = Plugin::new(&class_id()).unwrap();
    let config = config(&mut plugin, outputs);
    plugin.prepare_audio(&config).unwrap();
    plugin
}

fn block(plugin: &mut Plugin, events: &[MidiEvent]) -> Result<Vec<MidiEvent>, Error> {
    let mut output = vec![vec![0.0f32; 512]; 2];
    let mut outputs: Vec<&mut [f32]> = output.iter_mut().map(Vec::as_mut_slice).collect();
    let mut produced = Vec::new();
    plugin.process(
        &plughost_core::BlockContext::new(512),
        &[],
        &mut outputs,
        &[],
        events,
        &mut produced,
    )?;
    Ok(produced)
}

#[test]
fn midi_outputs_are_listed_as_event_ports_by_cable() {
    let mut plugin = Plugin::new(&class_id()).unwrap();
    let outputs: Vec<_> = plugin
        .event_ports()
        .unwrap()
        .into_iter()
        .filter(|port| port.direction == AudioDirection::Output)
        .map(|port| (port.id, port.index, port.name))
        .collect();
    assert_eq!(
        outputs,
        [(0, 0, "Echo".to_owned()), (1, 1, "Second".to_owned())]
    );
}

#[test]
fn sent_midi_arrives_at_its_block_offset_as_separate_messages() {
    let mut plugin = prepared(vec![0]);
    let sysex = vec![0xF0, 0x7D, 0x01, 0x02, 0xF7];
    let produced = block(
        &mut plugin,
        &[
            MidiEvent::note_on(100, 0, 60, 100),
            MidiEvent::sysex(200, sysex.clone()),
            MidiEvent::control_change(300, 0, 127, 0),
        ],
    )
    .unwrap();
    assert_eq!(
        produced,
        [
            MidiEvent::note_on(100, 0, 60, 100),
            MidiEvent::sysex(200, sysex),
            MidiEvent::channel(300, [0x90, 1, 2]),
            MidiEvent::channel(300, [0x90, 3, 4]),
            MidiEvent::sysex(300, vec![0xF0, 0x7D, 0x01, 0xF7]),
        ]
    );
    // The clock has no event form; it is reported, not delivered.
    let diagnostics = plugin.take_diagnostics().records;
    assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
    assert!(diagnostics[0].message.ends_with(" 1"), "{diagnostics:?}");

    // Offsets are relative to each block.
    let produced = block(&mut plugin, &[MidiEvent::note_off(5, 0, 60, 0)]).unwrap();
    assert_eq!(produced, [MidiEvent::note_off(5, 0, 60, 0)]);
}

#[test]
fn only_prepared_outputs_deliver_their_messages() {
    let second = [MidiEvent::control_change(10, 0, 126, 42)];
    let mut first_only = prepared(vec![0]);
    assert_eq!(block(&mut first_only, &second).unwrap(), []);

    let mut both = prepared(vec![0, 1]);
    assert_eq!(
        block(&mut both, &second).unwrap(),
        [MidiEvent::control_change(10, 0, 126, 42).on_port(1)]
    );

    let mut plugin = Plugin::new(&class_id()).unwrap();
    let config = config(&mut plugin, vec![2]);
    let error = plugin.prepare_audio(&config).unwrap_err();
    assert_eq!(error.kind(), plughost_core::FailureKind::InvalidInput);
}

#[test]
fn a_block_with_too_many_events_fails_and_the_next_one_delivers() {
    let mut plugin = prepared(vec![0]);
    let error = block(&mut plugin, &[MidiEvent::note_on(0, 0, 127, 1)]).unwrap_err();
    assert!(error.is_output_event_overflow(), "{error}");
    assert_eq!(
        block(&mut plugin, &[MidiEvent::note_on(7, 0, 60, 1)]).unwrap(),
        [MidiEvent::note_on(7, 0, 60, 1)]
    );
}

#[test]
fn the_main_configuration_prepares_the_first_output() {
    let mut plugin = Plugin::new(&class_id()).unwrap();
    plugin
        .prepare(&plughost_core::ProcessConfig {
            sample_rate: 48_000.0,
            max_block_size: 512,
            sample_format: SampleFormat::F32,
            input: Layout::None,
            output: Layout::Stereo,
            mode: ProcessMode::Offline,
        })
        .unwrap();
    let produced = block(&mut plugin, &[MidiEvent::note_on(3, 0, 64, 90)]).unwrap();
    assert_eq!(produced, [MidiEvent::note_on(3, 0, 64, 90)]);
}
