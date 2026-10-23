//! Errors cross threads, as the processing thread's do to the thread that owns the plugin.

#[test]
fn errors_are_send_and_sync() {
    fn require<T: Send + Sync>() {}
    require::<plughost_formats::Error>();
}
