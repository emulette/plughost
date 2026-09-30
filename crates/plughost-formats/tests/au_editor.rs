//! Audio Unit editors that change their size, from an AUv3 this test registers in its own
//! process. Editors are opened on the main thread, so this test has no harness.

#[cfg(target_os = "macos")]
mod unit {
    use std::cell::{OnceCell, RefCell};
    use std::ffi::c_void;
    use std::rc::Rc;
    use std::sync::Once;

    use block2::{DynBlock, RcBlock};
    use objc2::rc::{Allocated, Retained};
    use objc2::{AnyThread, ClassType, DefinedClass, MainThreadMarker, define_class, msg_send};
    use objc2_app_kit::{NSView, NSViewController};
    use objc2_audio_toolbox::{
        AUAudioUnit, AUAudioUnitBus, AUAudioUnitBusArray, AUAudioUnitBusType,
        AudioComponentDescription, AudioComponentInstantiationOptions, kAudioUnitType_Effect,
    };
    use objc2_avf_audio::AVAudioFormat;
    use objc2_core_audio_types::AudioBufferList;
    use objc2_foundation::{NSArray, NSError, NSInteger, NSSize, NSString};
    use plughost_formats::au::{self, Plugin};
    use plughost_formats::{EditorView, HostedPlugin};

    type InternalRender = dyn Fn(
        *mut c_void,
        *const c_void,
        u32,
        NSInteger,
        *mut AudioBufferList,
        *const c_void,
        *mut c_void,
    ) -> i32;

    thread_local! {
        /// The view controller the unit gave out last.
        static CONTROLLER: RefCell<Option<Retained<NSViewController>>> = const { RefCell::new(None) };
    }

    #[derive(Default)]
    struct Ivars {
        inputs: OnceCell<Retained<AUAudioUnitBusArray>>,
        outputs: OnceCell<Retained<AUAudioUnitBusArray>>,
        render: OnceCell<RcBlock<InternalRender>>,
    }

    define_class!(
        /// An effect whose editor is a 300 × 200 view.
        #[unsafe(super(AUAudioUnit))]
        #[ivars = Ivars]
        struct ViewUnit;

        impl ViewUnit {
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

            #[unsafe(method(requestViewControllerWithCompletionHandler:))]
            fn request_view_controller(&self, handler: &DynBlock<dyn Fn(*mut NSViewController)>) {
                let mtm = MainThreadMarker::new().unwrap();
                let controller = NSViewController::new(mtm);
                let view = NSView::new(mtm);
                view.setFrameSize(NSSize::new(300.0, 200.0));
                controller.setView(&view);
                controller.setPreferredContentSize(NSSize::new(300.0, 200.0));
                handler.call((Retained::as_ptr(&controller).cast_mut(),));
                CONTROLLER.set(Some(controller));
            }
        }
    );

    impl ViewUnit {
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
            let bus = || -> Option<Retained<AUAudioUnitBus>> {
                let bus: Result<Retained<AUAudioUnitBus>, Retained<NSError>> = unsafe {
                    msg_send![AUAudioUnitBus::alloc(), initWithFormat: &*format, error: _]
                };
                bus.ok()
            };
            let array = |kind, bus: Retained<AUAudioUnitBus>| unsafe {
                AUAudioUnitBusArray::initWithAudioUnit_busType_busses(
                    AUAudioUnitBusArray::alloc(),
                    &this,
                    kind,
                    &NSArray::from_retained_slice(&[bus]),
                )
            };
            let _ = this
                .ivars()
                .inputs
                .set(array(AUAudioUnitBusType::Input, bus()?));
            let _ = this
                .ivars()
                .outputs
                .set(array(AUAudioUnitBusType::Output, bus()?));
            Some(this)
        }
    }

    /// Registers the unit once and returns its class ID.
    fn class_id() -> String {
        static REGISTER: Once = Once::new();
        REGISTER.call_once(|| unsafe {
            AUAudioUnit::registerSubclass_asComponentDescription_name_version(
                ViewUnit::class(),
                AudioComponentDescription {
                    componentType: kAudioUnitType_Effect,
                    componentSubType: u32::from_be_bytes(*b"view"),
                    componentManufacturer: u32::from_be_bytes(*b"Plgh"),
                    componentFlags: 0,
                    componentFlagsMask: 0,
                },
                &NSString::from_str("plughost: view"),
                1,
            );
        });
        au::components()
            .into_iter()
            .find(|component| component.name == "view")
            .unwrap()
            .class_id
    }

    fn controller() -> Retained<NSViewController> {
        CONTROLLER.with_borrow(|controller| controller.clone().unwrap())
    }

    pub fn the_window_follows_the_size_the_view_takes() {
        let mtm = MainThreadMarker::new().unwrap();
        let mut plugin = Plugin::new(&class_id()).unwrap();
        let parent = NSView::new(mtm);
        let requests = Rc::new(RefCell::new(Vec::new()));
        let record = Rc::clone(&requests);
        let view = unsafe {
            plugin.open_editor(
                Retained::as_ptr(&parent).cast_mut().cast(),
                Box::new(move |width, height| {
                    record.borrow_mut().push((width, height));
                    true
                }),
            )
        }
        .unwrap();
        assert_eq!(
            view,
            EditorView::Embedded {
                width: 300,
                height: 200
            }
        );
        plugin.idle();
        assert_eq!(*requests.borrow(), []);

        // A v3 view controller asks for a new size.
        controller().setPreferredContentSize(NSSize::new(500.0, 250.0));
        plugin.idle();
        assert_eq!(*requests.borrow(), [(500, 250)]);
        assert_eq!(controller().view().frame().size, NSSize::new(500.0, 250.0));

        // A view sizes itself; its controller's unchanged preference does not undo that.
        controller().view().setFrameSize(NSSize::new(320.0, 240.0));
        plugin.idle();
        plugin.idle();
        assert_eq!(*requests.borrow(), [(500, 250), (320, 240)]);

        plugin.close_editor();
        controller().setPreferredContentSize(NSSize::new(600.0, 300.0));
        plugin.idle();
        assert_eq!(requests.borrow().len(), 2);
    }
}

fn main() {
    #[cfg(target_os = "macos")]
    unit::the_window_follows_the_size_the_view_takes();
}
