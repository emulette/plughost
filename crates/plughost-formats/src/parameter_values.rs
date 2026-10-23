//! The rule every format applies to the parameter ranges and values a plugin reports: one that
//! is not finite has no normalized form, so the call that needs it fails, and a value outside its
//! range is clamped to it.

/// `min..=max` when it is a range: finite, in order, and with a finite span.
pub(crate) fn range(min: f64, max: f64) -> Option<(f64, f64)> {
    (min.is_finite() && max.is_finite() && min <= max && (max - min).is_finite())
        .then_some((min, max))
}

/// `plain` normalized over `min..=max` and clamped to it, or `None` when either is not finite. A
/// range of one value normalizes to 0.
pub(crate) fn normalized(plain: f64, min: f64, max: f64) -> Option<f64> {
    let (min, max) = range(min, max)?;
    if !plain.is_finite() {
        return None;
    }
    Some(if max > min {
        ((plain - min) / (max - min)).clamp(0.0, 1.0)
    } else {
        0.0
    })
}
