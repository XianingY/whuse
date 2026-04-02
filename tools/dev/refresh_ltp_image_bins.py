#!/usr/bin/env python3
from __future__ import annotations

import argparse
import subprocess
import sys
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[2]
DEFAULT_IMAGE = REPO_ROOT / "target" / "oscomp" / "sdcard-rv.img"
DEFAULT_OUTPUT = REPO_ROOT / "tools" / "oscomp" / "ltp" / "musl_rv_seed_image_bins.txt"
DEBUGFS_TARGET = "/musl/ltp/testcases/bin"


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Refresh musl-rv LTP seed list from real bins inside the image."
    )
    parser.add_argument(
        "--image",
        type=Path,
        default=DEFAULT_IMAGE,
        help=f"path to sdcard image (default: {DEFAULT_IMAGE})",
    )
    parser.add_argument(
        "--output",
        type=Path,
        default=DEFAULT_OUTPUT,
        help=f"output seed file (default: {DEFAULT_OUTPUT})",
    )
    return parser.parse_args()


def list_debugfs_entries(image: Path) -> list[str]:
    result = subprocess.run(
        ["debugfs", "-R", f"ls -p {DEBUGFS_TARGET}", str(image)],
        check=True,
        capture_output=True,
        text=True,
    )
    return result.stdout.splitlines()


def parse_case_names(lines: list[str]) -> list[str]:
    names: set[str] = set()
    for raw in lines:
        line = raw.strip()
        if not line.startswith("/"):
            continue
        parts = line.split("/")
        if len(parts) < 7:
            continue
        name = parts[5].strip()
        if not name or name in {".", ".."}:
            continue
        names.add(name)
    return sorted(names)


def write_lines(path: Path, lines: list[str]) -> None:
    path.write_text("".join(f"{line}\n" for line in lines), encoding="utf-8")


def main() -> int:
    args = parse_args()
    image = args.image.expanduser().resolve()
    output = args.output.expanduser().resolve()
    if not image.is_file():
        raise FileNotFoundError(f"missing image: {image}")
    entries = list_debugfs_entries(image)
    case_names = parse_case_names(entries)
    if not case_names:
        raise RuntimeError(f"no LTP bins found under {DEBUGFS_TARGET} in {image}")
    output.parent.mkdir(parents=True, exist_ok=True)
    write_lines(output, case_names)
    print(f"refreshed {output} ({len(case_names)}) from {image}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
