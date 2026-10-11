use serde::{Deserialize, Serialize};

/// Native parameter properties. All flags default to false.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ParameterFlags {
    /// The parameter has discrete values; `step_count` describes the intervals.
    pub discrete: bool,
    /// The plugin permits sample-timed automation for this parameter.
    pub automatable: bool,
    /// The value is observable but host edits are prohibited.
    pub read_only: bool,
    /// The native parameter is marked hidden from ordinary host interfaces.
    pub hidden: bool,
    /// This parameter controls native bypass.
    pub bypass: bool,
    /// The native parameter represents a labeled list of choices.
    pub list: bool,
    /// This parameter selects a native program.
    pub program_change: bool,
}

/// Native parameter identity, labels, normalized default, and edit capabilities.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ParameterInfo {
    /// VST3 or CLAP parameter ID, or Audio Unit parameter address.
    pub id: u64,
    /// Full native display name.
    pub title: String,
    /// Abbreviated native display name, or empty when unavailable.
    pub short_title: String,
    /// Native unit label, or empty when unavailable.
    pub units: String,
    /// Number of intervals; flags.discrete distinguishes a constant discrete value from continuous.
    pub step_count: u32,
    /// Native default, normalized to 0..=1. None when the format does not expose it.
    pub default_value: Option<f64>,
    /// Native capabilities and presentation flags.
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

    /// Validates a writable, automatable normalized value and rounds discrete steps.
    /// Rejects non-finite values and values outside `0..=1` before quantization.
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
    /// Native parameter ID or Audio Unit address.
    pub id: u64,
    /// Zero-based sample offset relative to the supplied processing block.
    pub offset: usize,
    /// Finite normalized value in `0..=1`.
    pub value: f64,
}

/// A point on one slot's automation lane. Offsets are relative to the supplied block/render.
/// Points hold their value until the next point; simultaneous points preserve input order.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct AutomationEvent {
    /// Zero-based index of the plugin slot receiving the change.
    pub slot: usize,
    /// Parameter ID, sample offset, and normalized value.
    pub change: ParameterChange,
}

/// Checks nondecreasing offsets strictly below `frames` and finite normalized values.
/// Does not query parameter metadata or enforce the per-block event budget.
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

/// Rejects text containing NUL or exceeding `PARAMETER_TEXT_BYTES` UTF-8 bytes.
pub fn validate_parameter_text(text: &str) -> Result<(), crate::InputError> {
    if text.len() > PARAMETER_TEXT_BYTES || text.contains('\0') {
        Err(crate::InputError::ParameterText)
    } else {
        Ok(())
    }
}

/// Rejects non-finite values and values outside the inclusive normalized range `0..=1`.
pub fn validate_normalized(value: f64) -> Result<(), crate::InputError> {
    if value.is_finite() && (0.0..=1.0).contains(&value) {
        Ok(())
    } else {
        Err(crate::InputError::ParameterValue)
    }
}

/// One native unit or group in a parameter’s root-to-leaf hierarchy.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParameterGroup {
    /// VST3 unit ID, CLAP module path, or AU group key path.
    pub id: String,
    /// Native group display name.
    pub name: String,
    /// Native VST3 program-list association, when supplied by the unit.
    pub program_list_id: Option<i32>,
}

/// A fresh query. Endpoints describe the normalized mapping, not a linear scale.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ParameterDetails {
    /// Current parameter metadata, including flags.
    pub info: ParameterInfo,
    /// AU AudioUnitParameterUnit code. Other formats do not expose this unit enumeration.
    pub native_unit: Option<u32>,
    /// Native value corresponding to normalized zero.
    pub plain_at_zero: f64,
    /// Native value corresponding to normalized one.
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

/// Maximum number of discrete choices returned by a single page request.
pub const PARAMETER_CHOICE_PAGE_SIZE: u32 = 256;

/// One discrete parameter choice and its native display conversion.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ParameterChoice {
    /// Zero-based discrete choice index.
    pub index: u64,
    /// Choice value in the inclusive normalized range `0..=1`.
    pub normalized: f64,
    /// Value expressed in the plugin’s native units.
    pub plain: f64,
    /// Native display text for this choice.
    pub text: String,
}

/// A bounded slice of a discrete parameter’s native choices.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ParameterChoicePage {
    /// Total number of discrete choices, including both endpoints.
    pub total: u64,
    /// Index of the first returned choice.
    pub start: u64,
    /// Choices in ascending index order; the final page may be shorter than requested.
    pub choices: Vec<ParameterChoice>,
}

/// Checks the page-size limit and the representable discrete-choice index range.
pub fn validate_choice_page(start: u64, count: u32) -> Result<(), crate::InputError> {
    if count == 0 || count > PARAMETER_CHOICE_PAGE_SIZE || start > u64::from(u32::MAX) + 1 {
        Err(crate::InputError::ParameterChoicePage)
    } else {
        Ok(())
    }
}
