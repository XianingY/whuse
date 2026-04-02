#!/usr/bin/env python3
from __future__ import annotations

import re
import sys
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[2]
LTP_DIR = REPO_ROOT / "tools" / "oscomp" / "ltp"

IMAGE_SEED = LTP_DIR / "musl_rv_seed_image_bins.txt"
CURATED = LTP_DIR / "musl_rv_curated_whitelist.txt"
BLACKLIST = LTP_DIR / "musl_rv_curated_blacklist.txt"
FLAKY = LTP_DIR / "musl_rv_flaky_score_cases.txt"
REF_UNDEFINED = LTP_DIR / "musl_rv_seed_ref_undefined.txt"
REF_NIGHTHAWK = LTP_DIR / "musl_rv_seed_ref_nighthawk.txt"
REF_STARRY = LTP_DIR / "musl_rv_seed_ref_starry_mix.txt"

FROZEN_PREFIXES = (
    "bpf_",
    "statx",
    "nfs",
    "tpm",
    "vxlan",
    "geneve",
    "gre",
    "futex_waitv",
    "keyctl",
    "request_key",
)
FROZEN_EXACT = {
    "add_key01",
    "mremap01",
    "open_tree01",
    "open_tree02",
    "open_by_handle_at01",
    "open_by_handle_at02",
}

ROUND_PATTERNS: dict[str, tuple[re.Pattern[str], ...]] = {
    "fs_path": (
        re.compile(
            r"^(access|ch(dir|mod|own)|fch(mod|own)|lchown|link|unlink|rename|mkdir|rmdir|"
            r"readlink|symlink|stat|lstat|fstat|statfs|statvfs|truncate|ftruncate|creat)"
        ),
    ),
    "open_io": (
        re.compile(
            r"^(open|openat|close|read|write|pread|pwrite|readv|writev|preadv|pwritev|"
            r"lseek|sendfile|copy_file_range|splice|tee|vmsplice)"
        ),
    ),
    "process_signal": (
        re.compile(
            r"^(fork|vfork|clone|exec|wait|waitpid|waitid|kill|tgkill|sig|rt_sig|setsid|"
            r"getsid|getpid|getppid|gettid)"
        ),
    ),
    "socket_basic": (
        re.compile(
            r"^(socket|bind|listen|accept|connect|shutdown|send|recv|getsock|setsock|"
            r"socketpair|poll|epoll|select)"
        ),
    ),
    "time": (
        re.compile(
            r"^(alarm|clock_|clock|getitimer|setitimer|nanosleep|timer|timerfd|adjtimex|time)"
        ),
    ),
}

PHASE_LIMITS: dict[str, int] = {
    "fs_path": 16,
    "open_io": 16,
    "process_signal": 16,
}


def read_case_set(path: Path) -> set[str]:
    return {line.strip() for line in path.read_text(encoding="utf-8").splitlines() if line.strip()}


def write_lines(path: Path, lines: list[str]) -> None:
    path.write_text("".join(f"{line}\n" for line in lines), encoding="utf-8")


def write_phase_slices(round_name: str, selected: list[str], chunk_size: int) -> None:
    phase_idx = 1
    start = 0
    while start < len(selected):
        phase_output = LTP_DIR / f"musl_rv_image_round_{round_name}_phase{phase_idx}.txt"
        chunk = selected[start : start + chunk_size]
        write_lines(phase_output, chunk)
        print(f"{phase_output.name}: {len(chunk)}")
        start += chunk_size
        phase_idx += 1


def is_frozen_case(case_name: str) -> bool:
    if case_name in FROZEN_EXACT:
        return True
    if case_name.endswith(".sh"):
        return True
    if case_name.startswith("mmap") and case_name not in {"mmap01", "mmap09", "mmap10", "mmap11"}:
        return True
    return any(case_name.startswith(prefix) for prefix in FROZEN_PREFIXES)


def support_rank(
    case_name: str,
    undefined: set[str],
    nighthawk: set[str],
    starry: set[str],
) -> tuple[int, int, int, int, str]:
    in_undefined = int(case_name in undefined)
    in_nighthawk = int(case_name in nighthawk)
    in_starry = int(case_name in starry)
    support = in_undefined + in_nighthawk + in_starry
    return (-support, -in_nighthawk, -in_undefined, -in_starry, case_name)


def select_cases(
    seed: set[str],
    curated: set[str],
    blacklist: set[str],
    flaky: set[str],
    patterns: tuple[re.Pattern[str], ...],
    undefined: set[str],
    nighthawk: set[str],
    starry: set[str],
) -> list[str]:
    candidates = []
    for case_name in seed:
        if case_name in curated or case_name in blacklist or case_name in flaky or is_frozen_case(case_name):
            continue
        if any(pattern.match(case_name) for pattern in patterns):
            candidates.append(case_name)
    return sorted(
        candidates,
        key=lambda case_name: support_rank(case_name, undefined, nighthawk, starry),
    )


def main() -> int:
    seed = read_case_set(IMAGE_SEED)
    curated = read_case_set(CURATED)
    blacklist = read_case_set(BLACKLIST)
    flaky = read_case_set(FLAKY)
    undefined = read_case_set(REF_UNDEFINED)
    nighthawk = read_case_set(REF_NIGHTHAWK)
    starry = read_case_set(REF_STARRY)

    print(f"image seed count: {len(seed)}")
    for round_name, patterns in ROUND_PATTERNS.items():
        selected = select_cases(seed, curated, blacklist, flaky, patterns, undefined, nighthawk, starry)
        output = LTP_DIR / f"musl_rv_image_round_{round_name}.txt"
        write_lines(output, selected)
        print(f"{output.name}: {len(selected)}")
        phase_limit = PHASE_LIMITS.get(round_name)
        if phase_limit is not None:
            write_phase_slices(round_name, selected, phase_limit)
    return 0


if __name__ == "__main__":
    sys.exit(main())
