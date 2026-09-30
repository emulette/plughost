use clack_extensions::preset_discovery::prelude::*;
use clack_plugin::prelude::PluginError;
use std::ffi::CStr;
pub struct Factory(ProviderDescriptor);
impl Factory {
    pub fn new() -> Self {
        Self(
            ProviderDescriptor::new("com.studio.plughost.presets", "Fixture presets")
                .with_vendor("plughost"),
        )
    }
}
impl PresetDiscoveryFactoryImpl for Factory {
    fn provider_count(&self) -> u32 {
        1
    }
    fn provider_descriptor(&self, index: u32) -> Option<&ProviderDescriptor> {
        (index == 0).then_some(&self.0)
    }
    fn create_provider<'a>(
        &'a self,
        indexer: IndexerInfo<'a>,
        id: &CStr,
    ) -> Option<ProviderInstance<'a>> {
        if Some(id) != self.0.id() {
            return None;
        }
        Some(ProviderInstance::new(indexer, &self.0, |mut indexer| {
            indexer.declare_filetype(FileType {
                name: c"Fixture preset",
                description: Some(c"Provider test input"),
                file_extension: Some(c"phcp"),
            })?;
            indexer.declare_location(LocationInfo {
                name: c"Factory",
                flags: Flags::IS_FACTORY_CONTENT,
                location: Location::Plugin,
            })?;
            Ok(Presets)
        }))
    }
}
struct Presets;
impl ProviderImpl<'_> for Presets {
    fn get_metadata(
        &mut self,
        location: Location,
        receiver: &mut MetadataReceiver,
    ) -> Result<(), PluginError> {
        if let Location::File { path } = location {
            let path = path.to_string_lossy();
            if path.ends_with("crash.phcp") {
                std::process::abort();
            }
            if path.ends_with("hang.phcp") {
                loop {
                    std::thread::sleep(std::time::Duration::from_secs(1));
                }
            }
            if path.ends_with("fail.phcp") {
                return Err(PluginError::Message(crate::errors::REFUSED));
            }
            if path.ends_with("duplicate.phcp") {
                receiver.begin_preset(Some(c"Duplicate"), Some(c"same"))?;
                receiver.begin_preset(Some(c"Duplicate"), Some(c"same"))?;
                return Ok(());
            }
        }
        receiver
            .begin_preset(Some(c"Half"), Some(c"half"))?
            .add_plugin_id(UniversalPluginId::clap(c"com.studio.plughost.test-presets"))
            .add_creator(c"plughost")
            .set_description(c"Constant half-scale output")
            .add_feature(c"Test");
        receiver
            .begin_preset(Some(c"Quarter"), Some(c"quarter"))?
            .add_plugin_id(UniversalPluginId::clap(c"com.studio.plughost.test-presets"));
        Ok(())
    }
}
