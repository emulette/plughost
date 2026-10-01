use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ParameterFlags {
    pub discrete: bool,
    pub automatable: bool,
    pub read_only: bool,
    pub hidden: bool,
    pub bypass: bool,
    pub list: bool,
    pub program_change: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ParameterInfo {
    /// VST3 or CLAP parameter ID, or Audio Unit parameter address.
    pub id: u64,
    pub title: String,
    pub short_title: String,
    pub units: String,
    /// Number of intervals; flags.discrete distinguishes a constant discrete value from continuous.
    pub step_count: u32,
    /// Native default, normalized to 0..=1. None when the format does not expose it.
    pub default_value: Option<f64>,
    pub flags: ParameterFlags,
}

impl ParameterInfo {
    /// A continuous parameter without a default, short title, units or flags.
    pub fn new(id: u64, title: impl Into<String>) -> ParameterInfo {
        ParameterInfo {
            id,
            title: title.into(),
            short_title: String::new(),
            units: String::new(),
            step_count: 0,
            default_value: None,
            flags: ParameterFlags::default(),
        }
    }

    pub fn automation_value(&self, value: f64) -> Result<f64, crate::InputError> {
        self.validate_edit(value)?;
        if !self.flags.automatable {
            return Err(crate::InputError::NotAutomatable { id: self.id });
        }
        Ok(if self.step_count == 0 {
            value
        } else {
            (value * f64::from(self.step_count)).round() / f64::from(self.step_count)
        })
    }

    /// Rejects a host edit before a controller or processing queue is changed.
    pub fn validate_edit(&self, value: f64) -> Result<(), crate::InputError> {
        if !value.is_finite() || !(0.0..=1.0).contains(&value) {
            return Err(crate::InputError::ParameterValue);
        }
        if self.flags.read_only {
            return Err(crate::InputError::ReadOnlyParameter { id: self.id });
        }
        Ok(())
    }
}

/// A normalized parameter value that takes effect at a sample offset within a block.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ParameterChange {
    pub id: u64,
    pub offset: usize,
    pub value: f64,
}

/// A point on one slot's automation lane. Offsets are relative to the supplied block/render.
/// Points hold their value until the next point; simultaneous points preserve input order.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct AutomationEvent {
    pub slot: usize,
    pub change: ParameterChange,
}

pub fn changes_fit(changes: &[ParameterChange], frames: usize) -> bool {
    changes
        .iter()
        .all(|c| c.offset < frames && c.value.is_finite() && (0.0..=1.0).contains(&c.value))
        && changes
            .windows(2)
            .all(|pair| pair[0].offset <= pair[1].offset)
}

/// Bound for caller text and native UTF-8 displays (VST3 has its own String128 output bound).
pub const PARAMETER_TEXT_BYTES: usize = 4096;

pub fn validate_parameter_text(text: &str) -> Result<(), crate::InputError> {
    if text.len() > PARAMETER_TEXT_BYTES || text.contains('\0') {
        Err(crate::InputError::ParameterText)
    } else {
        Ok(())
    }
}

pub fn validate_normalized(value: f64) -> Result<(), crate::InputError> {
    if value.is_finite() && (0.0..=1.0).contains(&value) {
        Ok(())
    } else {
        Err(crate::InputError::ParameterValue)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParameterGroup {
    /// VST3 unit ID, CLAP module path, or AU group key path.
    pub id: String,
    pub name: String,
    /// Native VST3 program-list association, when supplied by the unit.
    pub program_list_id: Option<i32>,
}

/// A fresh query. Endpoints describe the normalized mapping, not a linear scale.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ParameterDetails {
    pub info: ParameterInfo,
    /// AU AudioUnitParameterUnit code. Other formats do not expose this unit enumeration.
    pub native_unit: Option<u32>,
    pub plain_at_zero: f64,
    pub plain_at_one: f64,
    /// Root to leaf. None means unavailable; Some(empty) is ungrouped.
    pub groups: Option<Vec<ParameterGroup>>,
}

impl ParameterDetails {
    /// Details without a native unit or group information.
    pub fn new(info: ParameterInfo, plain_at_zero: f64, plain_at_one: f64) -> ParameterDetails {
        ParameterDetails {
            info,
            native_unit: None,
            plain_at_zero,
            plain_at_one,
            groups: None,
        }
    }
}

pub const PARAMETER_CHOICE_PAGE_SIZE: u32 = 256;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ParameterChoice {
    pub index: u64,
    pub normalized: f64,
    pub plain: f64,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ParameterChoicePage {
    pub total: u64,
    pub start: u64,
    pub choices: Vec<ParameterChoice>,
}

pub fn validate_choice_page(start: u64, count: u32) -> Result<(), crate::InputError> {
    if count == 0 || count > PARAMETER_CHOICE_PAGE_SIZE || start > u64::from(u32::MAX) + 1 {
        Err(crate::InputError::ParameterChoicePage)
    } else {
        Ok(())
    }
}
