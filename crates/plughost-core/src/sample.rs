use crate::config::SampleFormat;

/// An audio sample type the plugin formats can process.
pub trait Sample: Copy + Default + Send + Sync + 'static {
    const FORMAT: SampleFormat;

    fn to_f64(self) -> f64;
}

impl Sample for f32 {
    const FORMAT: SampleFormat = SampleFormat::F32;

    fn to_f64(self) -> f64 {
        f64::from(self)
    }
}

impl Sample for f64 {
    const FORMAT: SampleFormat = SampleFormat::F64;

    fn to_f64(self) -> f64 {
        self
    }
}
