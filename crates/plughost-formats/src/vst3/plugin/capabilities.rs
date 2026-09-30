use plughost_core::{Capabilities, Support};
use vst3::Steinberg::Vst::{
    BusDirections_, IAudioProcessorTrait, IComponentTrait, IUnitInfo, IUnitInfoTrait, MediaTypes_,
    SymbolicSampleSizes_,
};
use vst3::Steinberg::{kResultFalse, kResultOk};

use super::{Plugin, Vst3Error, lock};

fn count_support(count: i32) -> Support {
    if count < 0 {
        Support::Unknown
    } else {
        (count > 0).into()
    }
}

impl Plugin {
    /// Queries native interfaces without creating an editor or serializing state.
    pub(crate) fn capabilities(&self) -> Result<Capabilities, Vst3Error> {
        let automation = self
            .parameter_cache()
            .infos()
            .iter()
            .any(|info| info.flags.automatable)
            .into();
        let engine = lock(&self.engine);
        let instance = engine.instance()?;
        let precision = |size| match unsafe { instance.processor.canProcessSampleSize(size) } {
            result if result == kResultOk => Support::Supported,
            result if result == kResultFalse => Support::Unsupported,
            _ => Support::Unknown,
        };
        let notes = |direction| {
            count_support(unsafe {
                instance
                    .component
                    .getBusCount(MediaTypes_::kEvent as i32, direction)
            })
        };
        let factory_presets = self
            .controller
            .cast::<IUnitInfo>()
            .map_or(Support::Unsupported, |units| {
                count_support(unsafe { units.getProgramListCount() })
            });
        Ok(Capabilities {
            embedded_editor: if self.editor.is_some() {
                Support::Supported
            } else {
                Support::Unknown
            },
            bus_discovery: Support::Supported,
            note_input: notes(BusDirections_::kInput as i32),
            note_output: notes(BusDirections_::kOutput as i32),
            sample_accurate_automation: automation,
            // IComponent has state methods but no query establishing that the plugin implements them.
            state: Support::Unknown,
            factory_presets,
            f32: precision(SymbolicSampleSizes_::kSample32 as i32),
            f64: precision(SymbolicSampleSizes_::kSample64 as i32),
        })
    }
}
