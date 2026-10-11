#!/usr/bin/env python3
"""Run bounded AddressSanitizer fuzz sessions with versioned seeds and disposable discoveries."""
import argparse
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parent.parent
TARGETS = {
    "moduleinfo": "crates/plughost/src/scan/corpus/moduleinfo",
    "binary_header": "crates/plughost/src/scan/corpus/binary_header",
    "cache": "crates/plughost/src/scan/corpus/cache",
    "framing": "crates/plughost-core/src/ipc/corpus/framing",
    "shared_output": "crates/plughost-core/src/ipc/corpus/shared_output",
    "vstpreset": "crates/plughost-formats/src/corpus/vstpreset",
    "au_plist": "crates/plughost-formats/src/corpus/au_plist",
}


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--seconds", type=int, default=60)
    parser.add_argument("--toolchain", default="nightly")
    parser.add_argument("targets", nargs="*", choices=list(TARGETS))
    args = parser.parse_args()
    if args.seconds < 1:
        parser.error("--seconds must be positive")
    selected = args.targets or list(TARGETS)
    for name in selected:
        if name == "au_plist" and sys.platform != "darwin":
            continue
        discovered = ROOT / "target" / "fuzz-corpus" / name
        artifacts = ROOT / "target" / "fuzz-artifacts" / name
        discovered.mkdir(parents=True, exist_ok=True)
        artifacts.mkdir(parents=True, exist_ok=True)
        print(f"Fuzzing {name} for {args.seconds} seconds", flush=True)
        subprocess.run(
            [
                "cargo", f"+{args.toolchain}", "fuzz", "run", name,
                str(discovered), str(ROOT / TARGETS[name]), "--",
                f"-max_total_time={args.seconds}", "-max_len=131072", "-timeout=10",
                "-rss_limit_mb=2048", f"-artifact_prefix={artifacts}/",
            ],
            cwd=ROOT,
            check=True,
        )


if __name__ == "__main__":
    main()
