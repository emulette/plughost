//! Audio Unit bus negotiation against an AUv3 this test registers in its own process, once as an
//! instrument and once as an effect. Like JUCE's units, it has a stereo input and output and lists
//! only the channel pairs with its input on: 1 in 1 out, and 2 in 2 out.

#![cfg(target_os = "macos")]

use std::cell::OnceCell;
use std::ffi::c_void;
use std::sync::Once;

use block2::{DynBlock, RcBlock};
use objc2::rc::{Allocated, Retained};
use objc2::{AnyThread, ClassType, DefinedClass, define_class, msg_send};
use objc2_audio_toolbox::{
    AUAudioUnit, AUAudioUnitBus, AUAudioUnitBusArray, AUAudioUnitBusType,
    AudioComponentDescription, AudioComponentInstantiationOptions, kAudioUnitType_Effect,
    kAudioUnitType_MusicDevice,
};
use objc2_avf_audio::AVAudioFormat;
use objc2_core_audio_types::AudioBufferList;
use objc2_foundation::{NSArray, NSError, NSInteger, NSNumber, NSString};
use plughost_core::{Layout, ProcessConfig, ProcessMode, SampleFormat};
use plughost_formats::HostedPlugin;
use plughost_formats::au::{self, AuError, Plugin};

type InternalRender = dyn Fn(
    *mut c_void,
    *const c_void,
    u32,
    NSInteger,
    *mut AudioBufferList,
    *const c_void,
    *mut c_void,
) -> i32;

#[derive(Default)]
struct Ivars {
    inputs: OnceCell<Retained<AUAudioUnitBusArray>>,
    outputs: OnceCell<Retained<AUAudioUnitBusArray>>,
    render: OnceCell<RcBlock<InternalRender>>,
}

define_class!(
    #[unsafe(super(AUAudioUnit))]
    #[ivars = Ivars]
    struct PairedUnit;

    impl PairedUnit {
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

        #[unsafe(method_id(channelCapabilities))]
        fn channel_capabilities(&self) -> Retained<NSArray<NSNumber>> {
            NSArray::from_retained_slice(&[1, 1, 2, 2].map(NSNumber::new_i32))
        }

        #[unsafe(method(internalRenderBlock))]
        fn internal_render_block(&self) -> *mut DynBlock<InternalRender> {
            let render = self.ivars().render.get_or_init(|| {
                RcBlock::new(
                    |_: *mut c_void,
                     _: *const c_void,
                     _: u32,
                     _: NSInteger,
                     _: *mut AudioBufferList,
                     _: *const c_void,
                     _: *mut c_void| 0,
                )
            });
            RcBlock::as_ptr(render)
        }
    }
);

impl PairedUnit {
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
        let array = |kind| -> Option<Retained<AUAudioUnitBusArray>> {
            let bus: Result<Retained<AUAudioUnitBus>, Retained<NSError>> =
                unsafe { msg_send![AUAudioUnitBus::alloc(), initWithFormat: &*format, error: _] };
            Some(unsafe {
                AUAudioUnitBusArray::initWithAudioUnit_busType_busses(
                    AUAudioUnitBusArray::alloc(),
                    &this,
                    kind,
                    &NSArray::from_retained_slice(&[bus.ok()?]),
                )
            })
        };
        let _ = this.ivars().inputs.set(array(AUAudioUnitBusType::Input)?);
        let _ = this.ivars().outputs.set(array(AUAudioUnitBusType::Output)?);
        Some(this)
    }
}

/// Registers the class once under both types and returns the class ID of `kind`.
fn class_id(kind: u32) -> String {
    static REGISTER: Once = Once::new();
    let description = |kind| AudioComponentDescription {
        componentType: kind,
        componentSubType: u32::from_be_bytes(*b"pair"),
        componentManufacturer: u32::from_be_bytes(*b"Plgh"),
        componentFlags: 0,
        componentFlagsMask: 0,
    };
    REGISTER.call_once(|| {
        for kind in [kAudioUnitType_MusicDevice, kAudioUnitType_Effect] {
            unsafe {
                AUAudioUnit::registerSubclass_asComponentDescription_name_version(
                    PairedUnit::class(),
                    description(kind),
                    &NSString::from_str("plughost: paired"),
                    1,
                )
            };
        }
    });
    let kind = u32::to_be_bytes(kind);
    au::components()
        .into_iter()
        .find(|component| {
            component.name == "paired"
                && component.class_id.starts_with(
                    &kind
                        .iter()
                        .map(|byte| format!("{byte:02X}"))
                        .collect::<String>(),
                )
        })
        .unwrap()
        .class_id
}

fn prepare(kind: u32, input: Layout) -> Result<(), plughost_formats::Error> {
    Plugin::new(&class_id(kind))
        .unwrap()
        .prepare(&ProcessConfig {
            sample_rate: 48_000.0,
            max_block_size: 512,
            sample_format: SampleFormat::F32,
            input,
            output: Layout::Stereo,
            mode: ProcessMode::Offline,
        })
}

#[test]
fn an_instrument_leaves_its_input_off_whatever_channel_pairs_it_lists() {
    prepare(kAudioUnitType_MusicDevice, Layout::None).unwrap();
    prepare(kAudioUnitType_MusicDevice, Layout::Stereo).unwrap();
}

#[test]
fn an_effect_keeps_to_its_channel_pairs() {
    assert!(matches!(
        prepare(kAudioUnitType_Effect, Layout::None),
        Err(plughost_formats::Error::Au(AuError::AudioConfiguration))
    ));
    prepare(kAudioUnitType_Effect, Layout::Stereo).unwrap();
}
