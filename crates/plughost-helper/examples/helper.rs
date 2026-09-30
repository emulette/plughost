//! The helper executable an application ships: a `main` that calls `plughost_helper::run`.
//! On macOS, `scripts/build-helper.sh` packages and signs this example as a background `.app`.

fn main() -> std::process::ExitCode {
    plughost_helper::run()
}
