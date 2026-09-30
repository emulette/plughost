# plughost-helper

The helper process body for [plughost](https://crates.io/crates/plughost). An application ships a
separate executable whose `main` is:

```rust
fn main() -> std::process::ExitCode {
    plughost_helper::run()
}
```

and passes that executable's path to `plughost::Scanner` and `plughost::Chain`, which start and
manage the process. Build the application and helper with matching plughost versions. See the
[plughost documentation](https://crates.io/crates/plughost) for packaging requirements.

Licensed under either the Apache License, Version 2.0 or the MIT license, at your option.
