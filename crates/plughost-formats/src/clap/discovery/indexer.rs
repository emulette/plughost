use super::*;
use clack_host::prelude::HostError;
pub(super) struct Indexer {
    pub error: Option<ClapError>,
    file_types: Vec<PresetFileType>,
    locations: Vec<PresetLocationInfo>,
    soundpacks: Vec<PresetSoundpack>,
}
impl Indexer {
    pub fn new() -> Self {
        Self {
            error: None,
            file_types: Vec::new(),
            locations: Vec::new(),
            soundpacks: Vec::new(),
        }
    }
    pub fn take(&mut self) -> Result<Declarations, ClapError> {
        if let Some(error) = self.error.take() {
            return Err(error);
        }
        Ok(Declarations {
            file_types: std::mem::take(&mut self.file_types),
            locations: std::mem::take(&mut self.locations),
            soundpacks: std::mem::take(&mut self.soundpacks),
        })
    }
    fn receive(
        &mut self,
        action: impl FnOnce(&mut Self) -> Result<(), ClapError>,
    ) -> Result<(), HostError> {
        if self.error.is_some() {
            return Err(HostError::Message(
                super::super::errors::PRESET_DISCOVERY_FAILED,
            ));
        }
        action(self).map_err(|error| {
            self.error = Some(error);
            HostError::Message(super::super::errors::PRESET_DISCOVERY_FAILED)
        })
    }
}
impl IndexerImpl for Indexer {
    fn declare_filetype(&mut self, value: FileType) -> Result<(), HostError> {
        self.receive(|s| {
            let v = PresetFileType {
                name: text(value.name)?,
                description: optional(value.description)?,
                extension: optional(value.file_extension)?,
            };
            s.file_types.push(v);
            Ok(())
        })
    }
    fn declare_location(&mut self, value: LocationInfo) -> Result<(), HostError> {
        self.receive(|s| {
            let location = match value.location {
                Location::Plugin => PresetLocation::Plugin,
                Location::File { path } => PresetLocation::File {
                    path: text(path)?.into(),
                },
            };
            location_path(&location).map_err(|_| ClapError::PresetMetadata)?;
            let v = PresetLocationInfo {
                name: text(value.name)?,
                flags: value.flags.bits(),
                location,
            };
            s.locations.push(v);
            Ok(())
        })
    }
    fn declare_soundpack(&mut self, value: Soundpack) -> Result<(), HostError> {
        self.receive(|s| {
            let v = PresetSoundpack {
                id: text(value.id)?,
                name: text(value.name)?,
                flags: value.flags.bits(),
                description: optional(value.description)?,
                homepage_url: optional(value.homepage_url)?,
                vendor: optional(value.vendor)?,
                image_path: optional(value.image_path)?,
                release_timestamp: value.release_timestamp.map(|v| v.seconds_since_epoch()),
            };
            s.soundpacks.push(v);
            Ok(())
        })
    }
}

pub(super) struct Declarations {
    pub file_types: Vec<PresetFileType>,
    pub locations: Vec<PresetLocationInfo>,
    pub soundpacks: Vec<PresetSoundpack>,
}
