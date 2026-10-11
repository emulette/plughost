#!/usr/bin/env python3
"""Build and run a clean application and helper against cargo package artifacts."""

import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tarfile


def run(arguments: list[str], directory: Path) -> None:
    subprocess.run(arguments, cwd=directory, check=True)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--allow-dirty", action="store_true")
    args = parser.parse_args()
    root = Path(__file__).resolve().parent.parent
    output = root / "target" / "package-consumer"
    if output.exists():
        shutil.rmtree(output)
    metadata = json.loads(subprocess.check_output(
        ["cargo", "metadata", "--no-deps", "--format-version", "1", "--locked"], cwd=root
    ))
    names = ["plughost-core", "plughost-formats", "plughost-helper", "plughost"]
    version = next(p["version"] for p in metadata["packages"] if p["name"] == "plughost")
    package = ["cargo", "package", "--locked", "--target-dir", str(output / "packages")]
    if args.allow_dirty:
        package.append("--allow-dirty")
    for name in names:
        package += ["-p", name]
    run(package, root)

    consumer = output / "consumer"
    consumer.mkdir(parents=True)
    for name in names:
        archive = output / "packages" / "package" / f"{name}-{version}.crate"
        with tarfile.open(archive) as contents:
            # Archives were created above by cargo from this checkout.
            contents.extractall(consumer / "crates")
    patches = "\n".join(
        f'{name} = {{ path = "crates/{name}-{version}" }}' for name in names
    )
    (consumer / "Cargo.toml").write_text(
        '[workspace]\nmembers = ["app", "helper"]\nresolver = "3"\n'
        f"[patch.crates-io]\n{patches}\n", encoding="utf-8"
    )
    for name, dependency in [("app", "plughost"), ("helper", "plughost-helper")]:
        member = consumer / name
        (member / "src").mkdir(parents=True)
        (member / "Cargo.toml").write_text(
            f'[package]\nname = "package-{name}"\nversion = "0.0.0"\nedition = "2024"\n'
            f'[dependencies]\n{dependency} = "={version}"\n', encoding="utf-8"
        )
    shutil.copyfile(root / "release-tests/package-consumer/app.rs", consumer / "app/src/main.rs")
    (consumer / "helper/src/main.rs").write_text(
        "fn main() -> std::process::ExitCode { plughost_helper::run() }\n", encoding="utf-8"
    )
    resolved = json.loads(subprocess.check_output(
        ["cargo", "metadata", "--format-version", "1"], cwd=consumer
    ))
    for name in names:
        package = next(p for p in resolved["packages"] if p["name"] == name)
        assert package["source"] is None
        assert Path(package["manifest_path"]).resolve().is_relative_to(consumer / "crates")
    build = output / "build"
    run(["cargo", "build", "--workspace", "--release", "--locked", "--target-dir", str(build)], consumer)
    suffix = ".exe" if os.name == "nt" else ""
    helper = build / "release" / f"package-helper{suffix}"
    if sys.platform == "darwin":
        app = output / "PackageHelper.app"
        run([str(root / "scripts/macos/package-app.sh"), str(helper), str(app),
             "com.studio.plughost.package-consumer"], root)
        helper = app / "Contents/MacOS/package-helper"
    run([str(build / "release" / f"package-app{suffix}"), str(helper),
         str(root / "target/test-plugins"), str(consumer / "scan.json")], consumer)


if __name__ == "__main__":
    main()
