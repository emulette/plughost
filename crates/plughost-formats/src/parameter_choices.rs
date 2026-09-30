use crate::{Error, HostedPlugin};
use plughost_core::{InputError, ParameterChoice, ParameterChoicePage, validate_choice_page};

pub(crate) fn read<P: HostedPlugin + ?Sized>(
    plugin: &mut P,
    id: u64,
    start: u64,
    count: u32,
) -> Result<ParameterChoicePage, Error> {
    validate_choice_page(start, count).map_err(Error::Input)?;
    let details = plugin.parameter_details(id)?;
    let steps = details.info.step_count;
    let (total, indices) = indices(&details, start, count)?;
    let mut choices = Vec::new();
    for index in indices {
        let normalized = if steps == 0 {
            0.0
        } else {
            index as f64 / f64::from(steps)
        };
        choices.push(ParameterChoice {
            index,
            normalized,
            plain: plugin.parameter_to_plain(id, normalized)?,
            text: plugin.parameter_text(id, normalized)?,
        });
    }
    Ok(ParameterChoicePage {
        total,
        start,
        choices,
    })
}

pub(crate) fn indices(
    details: &plughost_core::ParameterDetails,
    start: u64,
    count: u32,
) -> Result<(u64, std::ops::Range<u64>), Error> {
    if !details.info.flags.discrete {
        return Err(Error::ContinuousParameter);
    }
    let total = u64::from(details.info.step_count) + 1;
    if start > total {
        return Err(Error::Input(InputError::ParameterChoicePage));
    }
    Ok((total, start..(start + u64::from(count)).min(total)))
}
