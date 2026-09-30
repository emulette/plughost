use super::*;
use tempfile::TempDir;

fn file(root: &Path, name: &str) -> PathBuf {
    let path = root.join(name);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, b"candidate, contents are not parsed by discovery").unwrap();
    path
}

fn root(format: PluginFormat, path: &Path) -> PresetDirectory {
    PresetDirectory {
        format,
        path: path.to_path_buf(),
    }
}

fn vst3(path: PathBuf) -> PresetFile {
    PresetFile {
        format: PluginFormat::Vst3,
        path,
    }
}

#[test]
fn discovers_nested_candidates_in_path_order_without_reading_contents() {
    let directory = TempDir::new().unwrap();
    let later = file(directory.path(), "Vendor/Zebra/z.vstpreset");
    let earlier = file(directory.path(), "Vendor/Alpha/a.VSTPRESET");
    let au = file(directory.path(), "Vendor/Alpha/a.aupreset");
    file(directory.path(), "Vendor/Alpha/a.vstpreset.backup");
    file(directory.path(), "README.md");
    assert_eq!(
        discover_preset_files(&root(PluginFormat::Vst3, directory.path())).unwrap(),
        [vst3(earlier), vst3(later)]
    );
    assert_eq!(
        discover_preset_files(&root(PluginFormat::AudioUnit, directory.path())).unwrap(),
        [PresetFile {
            format: PluginFormat::AudioUnit,
            path: au
        }]
    );
}

#[test]
fn missing_and_non_directory_roots_remain_explicit_io_errors() {
    let directory = TempDir::new().unwrap();
    let missing = root(PluginFormat::Vst3, &directory.path().join("absent"));
    assert!(matches!(
        discover_preset_files(&missing),
        Err(PresetSearchError::Io { path, source })
            if path == missing.path && source.kind() == io::ErrorKind::NotFound
    ));
    let not_directory = root(
        PluginFormat::Vst3,
        &file(directory.path(), "not-a-directory"),
    );
    assert!(matches!(
        discover_preset_files(&not_directory),
        Err(PresetSearchError::Io { path, .. }) if path == not_directory.path
    ));
    assert_eq!(
        discover_preset_files(&root(PluginFormat::Vst3, directory.path())).unwrap(),
        []
    );
}

#[cfg(unix)]
#[test]
fn follows_links_once_and_skips_unreadable_entries() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let directory = TempDir::new().unwrap();
    let real = directory.path().join("real");
    let own = file(&real, "Vendor/own.vstpreset");
    let external = directory.path().join("external");
    file(&external, "installed.vstpreset");
    symlink(&external, real.join("Linked")).unwrap();
    symlink(&real, real.join("Vendor/Loop")).unwrap();
    symlink(directory.path().join("absent"), real.join("Dangling")).unwrap();
    let locked = real.join("Locked");
    file(&locked, "hidden.vstpreset");
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
    let linked_root = directory.path().join("root");
    symlink(&real, &linked_root).unwrap();

    let found = discover_preset_files(&root(PluginFormat::Vst3, &linked_root));
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
    let relative = |path: &Path| linked_root.join(path.strip_prefix(&real).unwrap());
    assert_eq!(
        found.unwrap(),
        [
            vst3(linked_root.join("Linked/installed.vstpreset")),
            vst3(relative(&own)),
        ]
    );
}

#[cfg(target_os = "windows")]
#[test]
fn follows_a_junction_root_without_revisiting_a_junction_loop() {
    use std::os::windows::process::CommandExt;
    use windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;

    let directory = TempDir::new().unwrap();
    file(directory.path(), "one.vstpreset");
    let junction = directory.path().join("loop");
    let result = std::process::Command::new("pwsh")
        .args(["-NoProfile", "-NonInteractive", "-Command",
            "$ErrorActionPreference = 'Stop'; New-Item -ItemType Junction -Path $env:PLUGHOST_TEST_JUNCTION_PATH -Value $env:PLUGHOST_TEST_JUNCTION_TARGET | Out-Null"])
        .env("PLUGHOST_TEST_JUNCTION_PATH", &junction)
        .env("PLUGHOST_TEST_JUNCTION_TARGET", directory.path())
        .creation_flags(CREATE_NO_WINDOW)
        .output().unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let found = discover_preset_files(&root(PluginFormat::Vst3, directory.path()));
    let through_junction = discover_preset_files(&root(PluginFormat::Vst3, &junction));
    // Remove just the junction, without following its target, before recursive cleanup.
    fs::remove_dir(&junction).unwrap();
    assert_eq!(
        found.unwrap(),
        [vst3(directory.path().join("one.vstpreset"))]
    );
    assert_eq!(
        through_junction.unwrap(),
        [vst3(junction.join("one.vstpreset"))]
    );
}
