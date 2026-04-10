#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
SHARED_ROOT="${REPO_ROOT}"
if [[ "${REPO_ROOT}" == *"/.worktrees/"* ]]; then
    SHARED_ROOT="${REPO_ROOT%%/.worktrees/*}"
fi

fail() {
    echo "FAIL: $*" >&2
    exit 1
}

pick_la_image() {
    local candidate
    for candidate in \
        "$(dirname "${SHARED_ROOT}")/testsuits-for-oskernel/sdcard-la.img" \
        "${SHARED_ROOT}/target/oscomp/sdcard-la.img" \
        "${REPO_ROOT}/target/oscomp/sdcard-la.img"
    do
        if [[ "${candidate}" == "${REPO_ROOT}/target/oscomp/sdcard-la.img" ]]; then
            continue
        fi
        if [[ -f "${candidate}" ]] && debugfs -R "stats" "${candidate}" >/dev/null 2>&1; then
            printf '%s\n' "${candidate}"
            return 0
        fi
    done
    return 1
}

before_latest="$(ls -1t /tmp/la-stage2-*.log 2>/dev/null | head -n 1 || true)"
run_output="$(mktemp)"
la_image="$(pick_la_image || true)"
if [[ -z "${la_image}" ]]; then
    fail "failed to locate a readable loongarch oscomp image"
fi

mkdir -p "${REPO_ROOT}/target/oscomp"
ln -snf "${la_image}" "${REPO_ROOT}/target/oscomp/sdcard-la.img"

(
    cd "${REPO_ROOT}" &&
    cargo run --manifest-path tools/xtask/Cargo.toml -- build-loongarch
) >"${run_output}" 2>&1 || {
    cat "${run_output}" >&2
    fail "failed to build loongarch kernel artifact"
}

if ! (
    cd "${REPO_ROOT}" &&
    TIMEOUT_SECS=120 \
    WHUSE_STAGE2_IMAGE_POLICY=never \
    WHUSE_STAGE2_USE_IMAGE_COPY=1 \
    WHUSE_OSCOMP_PROFILE=basic \
    tools/dev/run_oscomp_stage2.sh loongarch
) >"${run_output}" 2>&1; then
    cat "${run_output}" >&2
fi

after_latest="$(ls -1t /tmp/la-stage2-*.log 2>/dev/null | head -n 1 || true)"
if [[ -z "${after_latest}" ]]; then
    fail "expected loongarch stage2 log to be created"
fi
if [[ -n "${before_latest}" && "${after_latest}" == "${before_latest}" ]]; then
    fail "expected a fresh loongarch stage2 log"
fi

log_text="$(mktemp)"
strings "${after_latest}" >"${log_text}"

grep -Fq "whuse-oscomp-step-begin:basic_testcode.sh" "${log_text}" \
    || fail "missing loongarch basic step begin marker in ${after_latest}"
grep -Fq "Testing brk :" "${log_text}" \
    || fail "missing basic brk probe output in ${after_latest}"
if grep -Eq "pid 1 \(init\).*trap|FATAL KERNEL TRAP|panic" "${log_text}"; then
    fail "unexpected kernel/init crash marker in ${after_latest}"
fi

echo "PASS: loongarch basic boot reached stage2 basic markers"
