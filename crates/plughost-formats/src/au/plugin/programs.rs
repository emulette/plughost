use super::{AuError, Plugin, lock};
use plughost_core::{FactoryPreset, FactoryPresetId, MAX_FACTORY_PRESETS};

impl Plugin {
    pub(crate) fn factory_presets(&self) -> Result<Vec<FactoryPreset>, AuError> {
        let engine = lock(&self.engine);
        let unit = engine.unit()?;
        let Some(presets) = (unsafe { unit.factoryPresets() }) else {
            return Ok(Vec::new());
        };
        if presets.count() > MAX_FACTORY_PRESETS {
            return Err(AuError::FactoryPresetMetadata);
        }
        let mut seen = std::collections::HashSet::new();
        (0..presets.count())
            .map(|index| {
                let preset = presets.objectAtIndex(index);
                let number = unsafe { preset.number() } as i64;
                if number < 0 || !seen.insert(number) {
                    return Err(AuError::FactoryPresetMetadata);
                }
                Ok(FactoryPreset {
                    id: FactoryPresetId::AudioUnit { number },
                    name: unsafe { preset.name() }.to_string(),
                    group: None,
                })
            })
            .collect()
    }

    /// Selects an advertised native factory preset. AU provides no asynchronous completion
    /// signal for its setter; returning confirms the selected identity, not render readiness.
    pub(crate) fn select_factory_preset(&mut self, id: &FactoryPresetId) -> Result<(), AuError> {
        let FactoryPresetId::AudioUnit { number } = id else {
            return Err(AuError::FactoryPreset);
        };
        if !self
            .factory_presets()?
            .iter()
            .any(|preset| preset.id == *id)
        {
            return Err(AuError::FactoryPreset);
        }
        let engine = lock(&self.engine);
        let unit = engine.unit()?;
        let presets = unsafe { unit.factoryPresets() }.ok_or(AuError::FactoryPreset)?;
        if presets.count() > MAX_FACTORY_PRESETS {
            return Err(AuError::FactoryPresetMetadata);
        }
        let preset = (0..presets.count())
            .map(|index| presets.objectAtIndex(index))
            .find(|preset| unsafe { preset.number() } as i64 == *number)
            .ok_or(AuError::FactoryPreset)?;
        unsafe { unit.setCurrentPreset(Some(&preset)) };
        let selected = unsafe { unit.currentPreset() }.ok_or(AuError::FactoryPresetSelection)?;
        if unsafe { selected.number() } as i64 != *number {
            return Err(AuError::FactoryPresetSelection);
        }
        self.restored_unprepared = if engine.prepared.is_none() {
            Some((
                unsafe { unit.fullState() }.ok_or(AuError::State)?,
                plughost_core::StatePurpose::Preset,
            ))
        } else {
            None
        };
        Ok(())
    }
}
