#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
RUNNER="${REPO_ROOT}/tools/dev/run_oscomp_stage2.sh"
LTP_DIR="${REPO_ROOT}/tools/oscomp/ltp"

fail() {
    echo "FAIL: $*" >&2
    exit 1
}

assert_file_exists() {
    local path="$1"
    [[ -f "${path}" ]] || fail "expected file to exist: ${path}"
}

assert_contains() {
    local path="$1"
    local pattern="$2"
    grep -Fq "${pattern}" "${path}" || fail "expected ${path} to contain: ${pattern}"
}

assert_not_contains() {
    local path="$1"
    local pattern="$2"
    if grep -Fq "${pattern}" "${path}"; then
        fail "did not expect ${path} to contain: ${pattern}"
    fi
}

assert_order_in_array_block() {
    local path="$1"
    local block_name="$2"
    local first="$3"
    local second="$4"
    local block_text first_line second_line
    block_text="$(
        awk -v block_name="${block_name}" '
            $0 ~ "^" block_name "=\\(" { in_block = 1 }
            in_block { print }
            in_block && $0 ~ "^\\)" { exit }
        ' "${path}"
    )"
    [[ -n "${block_text}" ]] || fail "expected to find array block ${block_name} in ${path}"
    first_line="$(printf '%s\n' "${block_text}" | nl -ba | grep -F "\"${first}\"" | awk 'NR==1 {print $1}')"
    second_line="$(printf '%s\n' "${block_text}" | nl -ba | grep -F "\"${second}\"" | awk 'NR==1 {print $1}')"
    [[ -n "${first_line}" ]] || fail "expected ${block_name} in ${path} to contain ${first}"
    [[ -n "${second_line}" ]] || fail "expected ${block_name} in ${path} to contain ${second}"
    if (( first_line >= second_line )); then
        fail "expected ${first} to appear before ${second} in ${block_name} from ${path}"
    fi
}

assert_file_exists "${RUNNER}"
assert_file_exists "${LTP_DIR}/score_whitelist.txt"
assert_file_exists "${LTP_DIR}/score_blacklist.txt"
assert_file_exists "${LTP_DIR}/musl_rv_curated_whitelist.txt"
assert_file_exists "${LTP_DIR}/musl_rv_curated_blacklist.txt"
assert_file_exists "${LTP_DIR}/musl_rv_batch_file_syscalls.txt"
assert_file_exists "${LTP_DIR}/musl_rv_batch_open_io.txt"
assert_file_exists "${LTP_DIR}/musl_rv_batch_stat.txt"
assert_file_exists "${LTP_DIR}/musl_rv_batch_memory.txt"
assert_file_exists "${LTP_DIR}/musl_rv_batch_sync.txt"
assert_file_exists "${LTP_DIR}/musl_rv_batch_path.txt"

assert_contains "${RUNNER}" "WHUSE_LTP_CURATED_WHITELIST"
assert_contains "${RUNNER}" "WHUSE_LTP_CURATED_BLACKLIST"
assert_contains "${RUNNER}" "WHUSE_STAGE2_REQUIRE_GUEST_SHUTDOWN"
assert_contains "${RUNNER}" "RUN_ID="
assert_contains "${RUNNER}" "ltp-riscv-curated"
assert_contains "${RUNNER}" "riscv-raw-exit"
assert_contains "${RUNNER}" "loongarch-raw-exit"
assert_contains "${RUNNER}" "both-raw-exit"
assert_contains "${RUNNER}" "riscv | riscv-raw-exit"
assert_contains "${RUNNER}" "loongarch | loongarch-raw-exit"
assert_contains "${RUNNER}" "both | both-raw-exit"
assert_contains "${RUNNER}" "missing guest shutdown marker"
assert_contains "${RUNNER}" "raw-exit runner did not exit cleanly"
assert_contains "${RUNNER}" "runtime config verification failed"
assert_contains "${RUNNER}" "runtime marker verification failed"
assert_contains "${RUNNER}" "basic profile semantic check fallback ok"
assert_contains "${RUNNER}" "count_case_filter_entries"
assert_contains "${RUNNER}" "refusing to overwrite protected score whitelist"
assert_contains "${REPO_ROOT}/crates/kernel-core/src/lib_loongarch.inc.rs" "if [ \\\"\$line\\\" = \\\"hwclock\\\" ]; then"
assert_contains "${REPO_ROOT}/crates/syscall/src/lib.rs" "whuse-busybox:utimensat-shortcut"
assert_not_contains "${RUNNER}" "cp \"\${pass_candidates}\" \"\${REPO_ROOT}/tools/oscomp/ltp/score_whitelist.txt\""
assert_not_contains "${RUNNER}" "cp \"\${bad_candidates}\" \"\${REPO_ROOT}/tools/oscomp/ltp/score_blacklist.txt\""
assert_order_in_array_block "${RUNNER}" "full_root_steps" "ltp_testcode.sh" "lmbench_testcode.sh"

echo "ok"
