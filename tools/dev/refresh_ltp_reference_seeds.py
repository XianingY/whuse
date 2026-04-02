#!/usr/bin/env python3
from __future__ import annotations

import os
import re
import sys
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[2]
LTP_DIR = REPO_ROOT / "tools" / "oscomp" / "ltp"

UNDEFINED_DEFAULT = REPO_ROOT.parent / "os" / "undefined"
NIGHTHAWK_DEFAULT = REPO_ROOT.parent / "os" / "NighthawkOS"
STARRY_DEFAULT = REPO_ROOT.parent / "os" / "starry-mix"

ROUND1_BATCHES = (
    "sync",
    "path",
    "open_io",
    "file_syscalls",
)

BLACKLIST_REVIEW_ROUND1 = (
    "dup03",
    "dup06",
    "dup205",
    "fcntl05",
    "fstat03",
    "statvfs02",
)

FROZEN_PREFIXES = (
    "statx",
    "nfs",
    "tpm",
    "vxlan",
    "geneve",
    "gre",
    "futex_waitv",
)
FROZEN_EXACT = {
    "mremap01",
    "open_tree01",
    "open_tree02",
    "open_by_handle_at01",
    "open_by_handle_at02",
}


def repo_path_from_env(var_name: str, default: Path) -> Path:
    value = os.environ.get(var_name)
    return Path(value).expanduser().resolve() if value else default.resolve()


def read_text(path: Path) -> str:
    return path.read_text(encoding="utf-8")


def parse_undefined_seed(root: Path) -> list[str]:
    text = read_text(root / "apps" / "oscomp" / "testcase_list")
    match = re.search(r'ltp_testlist="(.*?)"', text, re.S)
    if not match:
        raise RuntimeError("failed to parse undefined testcase_list ltp_testlist")
    return sorted({item for item in match.group(1).split() if item})


def parse_nighthawk_seed(root: Path) -> list[str]:
    text = read_text(root / "user" / "src" / "ltpauto.rs")
    match = re.search(
        r"pub fn runltp_rvml\(\) \{\n\s*let RVTESTCASES = Vec::from\(\[(.*?)\]\);",
        text,
        re.S,
    )
    if not match:
        raise RuntimeError("failed to parse NighthawkOS runltp_rvml list")
    return sorted(set(re.findall(r'"([^"]+)"', match.group(1))))


def parse_starry_seed(root: Path) -> list[str]:
    lines = read_text(root / "src" / "test" / "pre.sh").splitlines()
    inside = False
    cases: list[str] = []
    for raw in lines:
        line = raw.strip()
        if line == 'all_testcases="':
            inside = True
            continue
        if inside and line == '"':
            break
        if inside and line:
            cases.append(line)
    if not cases:
        raise RuntimeError("failed to parse starry-mix all_testcases block")
    return sorted(set(cases))


def read_case_set(path: Path) -> set[str]:
    return {line.strip() for line in read_text(path).splitlines() if line.strip()}


def is_frozen_case(case_name: str) -> bool:
    if case_name in FROZEN_EXACT:
        return True
    if case_name.startswith("mmap") and case_name not in {"mmap01"}:
        return case_name not in {"mmap09", "mmap10", "mmap11"}
    return any(case_name.startswith(prefix) for prefix in FROZEN_PREFIXES)


def ordered_round_cases(
    batch_cases: set[str],
    curated: set[str],
    blacklist: set[str],
    undefined: set[str],
    nighthawk: set[str],
    starry: set[str],
) -> list[str]:
    frontier = batch_cases & (undefined | nighthawk | starry) - curated - blacklist
    both = sorted(case for case in frontier if case in undefined and case in nighthawk)
    nighthawk_only = sorted(
        case for case in frontier if case in nighthawk and case not in undefined
    )
    undefined_only = sorted(
        case for case in frontier if case in undefined and case not in nighthawk
    )
    starry_only = sorted(
        case
        for case in frontier
        if case in starry and case not in undefined and case not in nighthawk
    )
    return both + nighthawk_only + undefined_only + starry_only


def write_lines(path: Path, lines: list[str] | tuple[str, ...]) -> None:
    path.write_text("".join(f"{line}\n" for line in lines), encoding="utf-8")


def main() -> int:
    undefined_root = repo_path_from_env("WHUSE_REF_UNDEFINED_REPO", UNDEFINED_DEFAULT)
    nighthawk_root = repo_path_from_env("WHUSE_REF_NIGHTHAWK_REPO", NIGHTHAWK_DEFAULT)
    starry_root = repo_path_from_env("WHUSE_REF_STARRY_REPO", STARRY_DEFAULT)

    undefined_seed = parse_undefined_seed(undefined_root)
    nighthawk_seed = parse_nighthawk_seed(nighthawk_root)
    starry_seed = parse_starry_seed(starry_root)

    write_lines(LTP_DIR / "musl_rv_seed_ref_undefined.txt", undefined_seed)
    write_lines(LTP_DIR / "musl_rv_seed_ref_nighthawk.txt", nighthawk_seed)
    write_lines(LTP_DIR / "musl_rv_seed_ref_starry_mix.txt", starry_seed)

    curated = read_case_set(LTP_DIR / "musl_rv_curated_whitelist.txt")
    blacklist = read_case_set(LTP_DIR / "musl_rv_curated_blacklist.txt")
    undefined_set = set(undefined_seed)
    nighthawk_set = set(nighthawk_seed)
    starry_set = set(starry_seed)

    for batch_name in ROUND1_BATCHES:
        batch_cases = read_case_set(LTP_DIR / f"musl_rv_batch_{batch_name}.txt")
        ordered = ordered_round_cases(
            batch_cases, curated, blacklist, undefined_set, nighthawk_set, starry_set
        )
        write_lines(LTP_DIR / f"musl_rv_round1_{batch_name}.txt", ordered)

    review_cases = [
        case_name
        for case_name in BLACKLIST_REVIEW_ROUND1
        if case_name in blacklist and not is_frozen_case(case_name)
    ]
    write_lines(LTP_DIR / "musl_rv_blacklist_review_round1.txt", review_cases)

    print(
        "refreshed:",
        "musl_rv_seed_ref_undefined.txt",
        f"({len(undefined_seed)})",
        "musl_rv_seed_ref_nighthawk.txt",
        f"({len(nighthawk_seed)})",
        "musl_rv_seed_ref_starry_mix.txt",
        f"({len(starry_seed)})",
    )
    for batch_name in ROUND1_BATCHES:
        round_path = LTP_DIR / f"musl_rv_round1_{batch_name}.txt"
        print(f"{round_path.name}: {sum(1 for _ in round_path.open(encoding='utf-8'))}")
    print(
        "musl_rv_blacklist_review_round1.txt:",
        len(review_cases),
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
