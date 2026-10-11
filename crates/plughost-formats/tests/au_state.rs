//! Native AU state restoration across allocation, including edits made after restore.
#![cfg(target_os = "macos")]

use std::cell::{Cell, OnceCell};
use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Once};

use block2::{DynBlock, RcBlock};
use objc2::rc::{Allocated, Retained};
use objc2::runtime::{AnyObject, Bool};
use objc2::{AnyThread, ClassType, DefinedClass, define_class, msg_send};
use objc2_audio_toolbox::{
    AUAudioUnit, AUAudioUnitBus, AUAudioUnitBusArray, AUAudioUnitBusType, AUAudioUnitPreset,
    AUParameter, AUParameterTree, AudioComponentDescription, AudioComponentInstantiationOptions,
    AudioUnitParameterOptions, AudioUnitParameterUnit, kAudioUnitType_Effect,
};
use objc2_avf_audio::AVAudioFormat;
use objc2_core_audio_types::AudioBufferList;
use objc2_foundation::{NSArray, NSDictionary, NSError, NSInteger, NSNumber, NSString};
use plughost_core::render::Tail;
use plughost_core::{
    FactoryPresetId, Layout, ProcessConfig, ProcessMode, SampleFormat, StatePurpose,
};
use plughost_formats::HostedPlugin;
use plughost_formats::au::{self, Plugin};

type State = NSDictionary<NSString, AnyObject>;
type Values = Arc<Mutex<[f32; 2]>>;
type InternalRender = dyn Fn(
    *mut c_void,
    *const c_void,
    u32,
    NSInteger,
    *mut AudioBufferList,
    *const c_void,
    *mut c_void,
) -> i32;
static EDITOR: Mutex<Option<Values>> = Mutex::new(None);
static SERIAL: Mutex<()> = Mutex::new(());
/// Instances created while set ignore a state restored before render resources are allocated.
static IGNORE_UNALLOCATED: AtomicBool = AtomicBool::new(false);

#[derive(Default)]
struct Ivars {
    inputs: OnceCell<Retained<AUAudioUnitBusArray>>,
    outputs: OnceCell<Retained<AUAudioUnitBusArray>>,
    tree: OnceCell<Retained<AUParameterTree>>,
    render: OnceCell<RcBlock<InternalRender>>,
    values: Values,
    bypass: Cell<bool>,
    ignore_unallocated: Cell<bool>,
}

define_class!(
    #[unsafe(super(AUAudioUnit))]
    #[ivars = Ivars]
    struct RestoredUnit;

    impl RestoredUnit {
        #[unsafe(method_id(initWithComponentDescription:options:error:))]
        fn init(this: Allocated<Self>, description: AudioComponentDescription, options: AudioComponentInstantiationOptions, error: *mut *mut NSError) -> Option<Retained<Self>> {
            Self::build(this, description, options, error)
        }
        #[unsafe(method_id(inputBusses))]
        fn inputs(&self) -> Retained<AUAudioUnitBusArray> { self.ivars().inputs.get().unwrap().clone() }
        #[unsafe(method_id(outputBusses))]
        fn outputs(&self) -> Retained<AUAudioUnitBusArray> { self.ivars().outputs.get().unwrap().clone() }
        #[unsafe(method_id(parameterTree))]
        fn tree(&self) -> Retained<AUParameterTree> { self.ivars().tree.get().unwrap().clone() }
        #[unsafe(method(shouldBypassEffect))]
        fn bypass(&self) -> Bool { Bool::new(self.ivars().bypass.get()) }
        #[unsafe(method(setShouldBypassEffect:))]
        fn set_bypass(&self, enabled: Bool) { self.ivars().bypass.set(enabled.as_bool()); }
        #[unsafe(method(latency))]
        fn latency(&self) -> f64 { f64::from(self.ivars().values.lock().unwrap()[0]) * 32.0 / 48_000.0 }
        #[unsafe(method(tailTime))]
        fn tail(&self) -> f64 { f64::from(self.ivars().values.lock().unwrap()[1]) * 64.0 / 48_000.0 }
        #[unsafe(method_id(fullState))]
        fn full_state(&self) -> Retained<State> { self.state() }
        #[unsafe(method_id(fullStateForDocument))]
        fn document_state(&self) -> Retained<State> { self.state() }
        #[unsafe(method(setFullState:))]
        fn set_full_state(&self, state: Option<&State>) { self.restore(state.unwrap()); }
        #[unsafe(method(setFullStateForDocument:))]
        fn set_document_state(&self, state: Option<&State>) { self.restore(state.unwrap()); }
        #[unsafe(method_id(factoryPresets))]
        fn presets(&self) -> Retained<NSArray<AUAudioUnitPreset>> { NSArray::from_retained_slice(&[Self::preset()]) }
        #[unsafe(method_id(currentPreset))]
        fn current_preset(&self) -> Retained<AUAudioUnitPreset> { Self::preset() }
        #[unsafe(method(setCurrentPreset:))]
        fn set_preset(&self, _: Option<&AUAudioUnitPreset>) { *self.ivars().values.lock().unwrap() = [0.5; 2]; }
        #[unsafe(method(allocateRenderResourcesAndReturnError:))]
        fn allocate(&self, error: *mut *mut NSError) -> Bool {
            // This unit requires state to be replayed after allocation, like affected native AUs.
            *self.ivars().values.lock().unwrap() = [0.0; 2];
            self.ivars().bypass.set(false);
            self.ivars().ignore_unallocated.set(false);
            unsafe { msg_send![super(self), allocateRenderResourcesAndReturnError: error] }
        }
        #[unsafe(method(internalRenderBlock))]
        fn render(&self) -> *mut DynBlock<InternalRender> {
            RcBlock::as_ptr(self.ivars().render.get_or_init(|| RcBlock::new(
                |_: *mut c_void, _: *const c_void, _: u32, _: NSInteger, _: *mut AudioBufferList, _: *const c_void, _: *mut c_void| 0
            )))
        }
    }
);

impl RestoredUnit {
    fn preset() -> Retained<AUAudioUnitPreset> {
        let preset = unsafe { AUAudioUnitPreset::new() };
        unsafe {
            preset.setNumber(0);
            preset.setName(&NSString::from_str("Initial"));
        }
        preset
    }
    fn state(&self) -> Retained<State> {
        let values = *self.ivars().values.lock().unwrap();
        let values = [
            NSNumber::new_f32(values[0]),
            NSNumber::new_f32(values[1]),
            NSNumber::new_bool(self.ivars().bypass.get()),
        ];
        NSDictionary::from_slices(
            &[
                &*NSString::from_str("host"),
                &*NSString::from_str("editor"),
                &*NSString::from_str("bypass"),
            ],
            &values
                .iter()
                .map(|value| &**value as &AnyObject)
                .collect::<Vec<_>>(),
        )
    }
    fn restore(&self, state: &State) {
        if self.ivars().ignore_unallocated.get() {
            return;
        }
        let number = |key| {
            state
                .objectForKey(&NSString::from_str(key))
                .unwrap()
                .downcast::<NSNumber>()
                .unwrap()
        };
        *self.ivars().values.lock().unwrap() = [number("host").as_f32(), number("editor").as_f32()];
        self.ivars().bypass.set(number("bypass").as_bool());
    }
    fn build(
        this: Allocated<Self>,
        description: AudioComponentDescription,
        options: AudioComponentInstantiationOptions,
        error: *mut *mut NSError,
    ) -> Option<Retained<Self>> {
        let this = this.set_ivars(Ivars {
            ignore_unallocated: Cell::new(IGNORE_UNALLOCATED.load(Ordering::Relaxed)),
            ..Ivars::default()
        });
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
        for (kind, target) in [
            (AUAudioUnitBusType::Input, &this.ivars().inputs),
            (AUAudioUnitBusType::Output, &this.ivars().outputs),
        ] {
            let bus: Result<Retained<AUAudioUnitBus>, Retained<NSError>> =
                unsafe { msg_send![AUAudioUnitBus::alloc(), initWithFormat: &*format, error: _] };
            target
                .set(unsafe {
                    AUAudioUnitBusArray::initWithAudioUnit_busType_busses(
                        AUAudioUnitBusArray::alloc(),
                        &this,
                        kind,
                        &NSArray::from_retained_slice(&[bus.ok()?]),
                    )
                })
                .unwrap();
        }
        let parameters = ["Host", "Editor"].into_iter().enumerate().map(|(id, name)| unsafe {
            AUParameterTree::createParameterWithIdentifier_name_address_min_max_unit_unitName_flags_valueStrings_dependentParameters(
                &NSString::from_str(name), &NSString::from_str(name), id as u64, 0.0, 1.0,
                AudioUnitParameterUnit::Generic, None,
                AudioUnitParameterOptions::Flag_IsReadable | AudioUnitParameterOptions::Flag_IsWritable, None, None,
            ).into_super()
        }).collect::<Vec<_>>();
        let tree = unsafe {
            AUParameterTree::createTreeWithChildren(&NSArray::from_retained_slice(&parameters))
        };
        let values = Arc::clone(&this.ivars().values);
        let observer = RcBlock::new(move |parameter: NonNull<AUParameter>, value: f32| {
            values.lock().unwrap()[unsafe { parameter.as_ref().address() } as usize] = value;
        });
        let values = Arc::clone(&this.ivars().values);
        let provider = RcBlock::new(move |parameter: NonNull<AUParameter>| {
            values.lock().unwrap()[unsafe { parameter.as_ref().address() } as usize]
        });
        unsafe {
            tree.setImplementorValueObserver(RcBlock::as_ptr(&observer));
            tree.setImplementorValueProvider(RcBlock::as_ptr(&provider));
        }
        this.ivars().tree.set(tree).unwrap();
        *EDITOR.lock().unwrap() = Some(Arc::clone(&this.ivars().values));
        Some(this)
    }
}

fn plugin() -> Plugin {
    static REGISTER: Once = Once::new();
    REGISTER.call_once(|| unsafe {
        AUAudioUnit::registerSubclass_asComponentDescription_name_version(
            RestoredUnit::class(),
            AudioComponentDescription {
                componentType: kAudioUnitType_Effect,
                componentSubType: u32::from_be_bytes(*b"rstr"),
                componentManufacturer: u32::from_be_bytes(*b"Plgh"),
                componentFlags: 0,
                componentFlagsMask: 0,
            },
            &NSString::from_str("plughost: restore edits"),
            1,
        );
    });
    let id = au::components()
        .into_iter()
        .find(|info| info.name == "restore edits")
        .unwrap()
        .class_id;
    Plugin::new(&id).unwrap()
}

fn config() -> ProcessConfig {
    ProcessConfig {
        sample_rate: 48_000.0,
        max_block_size: 512,
        sample_format: SampleFormat::F32,
        input: Layout::Stereo,
        output: Layout::Stereo,
        mode: ProcessMode::Offline,
    }
}

fn values(plugin: &mut Plugin) -> Vec<f64> {
    plugin
        .parameters()
        .into_iter()
        .map(|(_, value)| value)
        .collect()
}

#[test]
fn prepare_preserves_current_state_after_restore_or_factory_preset() {
    let _serial = SERIAL.lock().unwrap();
    for purpose in [
        Some(StatePurpose::Project),
        Some(StatePurpose::Preset),
        None,
    ] {
        let state = purpose.map(|purpose| {
            let mut source = plugin();
            source.set_parameter(0, 0.5).unwrap();
            source.save_state(purpose).unwrap()
        });
        // Created last, so the simulated editor below edits this instance.
        let mut plugin = plugin();
        if let (Some(purpose), Some(state)) = (purpose, &state) {
            plugin.restore_state(state, purpose).unwrap();
        } else {
            plugin
                .select_factory_preset(&FactoryPresetId::AudioUnit { number: 0 })
                .unwrap();
        }
        plugin.set_parameter(0, 0.25).unwrap();
        // A native editor updates the unit's model without calling the host's setter.
        EDITOR.lock().unwrap().as_ref().unwrap().lock().unwrap()[1] = 0.75;
        plugin.set_bypass(true).unwrap();
        plugin.prepare(&config()).unwrap();
        let values = values(&mut plugin);
        assert_eq!(values.len(), 2);
        assert_eq!(values[0], 0.25, "{purpose:?}");
        assert_eq!(plugin.bypass().unwrap().enabled, Some(true));
        let timing = plugin.timing().unwrap();
        assert_eq!(timing.latency, 8);
        assert_eq!(timing.tail, Tail::Samples(48));
        assert!(!timing.latency_changed);
    }
}

#[test]
fn prepare_applies_a_restore_the_unit_did_not_take_before_allocation() {
    let _serial = SERIAL.lock().unwrap();
    let mut source = plugin();
    source.set_parameter(0, 0.5).unwrap();
    source.set_parameter(1, 0.25).unwrap();
    let state = source.save_state(StatePurpose::Project).unwrap();
    IGNORE_UNALLOCATED.store(true, Ordering::Relaxed);
    let mut plugin = plugin();
    IGNORE_UNALLOCATED.store(false, Ordering::Relaxed);
    plugin.restore_state(&state, StatePurpose::Project).unwrap();
    plugin.prepare(&config()).unwrap();
    // Latency and tail follow the unit's model: 0.5 * 32 and 0.25 * 64 samples.
    let timing = plugin.timing().unwrap();
    assert_eq!(timing.latency, 16);
    assert_eq!(timing.tail, Tail::Samples(16));
}

#[test]
#[ignore = "AU instantiation benchmark; run in release mode with --nocapture"]
fn au_instantiation_benchmark() {
    use std::io::Write;
    let _serial = SERIAL.lock().unwrap();
    let first = plugin();
    let expected = first.info().clone();
    drop(first);
    let started = std::time::Instant::now();
    for _ in 0..300 {
        let mut unit = Plugin::new(&expected.class_id).unwrap();
        assert_eq!(unit.info().name, expected.name);
        assert_eq!(unit.info().vendor, expected.vendor);
        assert_eq!(unit.info().sdk_version, expected.sdk_version);
        assert_eq!(unit.parameters().len(), 2);
    }
    writeln!(
        std::io::stdout(),
        "instances=300 ms={:.3}",
        started.elapsed().as_secs_f64() * 1000.0
    )
    .unwrap();
}
