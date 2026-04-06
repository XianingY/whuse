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

assert_has_case_entry() {
    local path="$1"
    if ! grep -Eq '^[[:space:]]*[^#[:space:]]' "${path}"; then
        fail "expected ${path} to contain at least one case entry"
    fi
}

assert_function_contains() {
    local path="$1"
    local function_name="$2"
    local pattern="$3"
    local function_text
    function_text="$(
        awk -v function_name="${function_name}" '
            $0 ~ "^fn " function_name "\\(" { in_fn = 1 }
            in_fn { print }
            in_fn && $0 ~ "^}$" { exit }
        ' "${path}"
    )"
    [[ -n "${function_text}" ]] || fail "expected to find function ${function_name} in ${path}"
    grep -Fq "${pattern}" <<<"${function_text}" || fail "expected function ${function_name} in ${path} to contain: ${pattern}"
}

assert_function_not_contains() {
    local path="$1"
    local function_name="$2"
    local pattern="$3"
    local function_text
    function_text="$(
        awk -v function_name="${function_name}" '
            $0 ~ "^fn " function_name "\\(" { in_fn = 1 }
            in_fn { print }
            in_fn && $0 ~ "^}$" { exit }
        ' "${path}"
    )"
    [[ -n "${function_text}" ]] || fail "expected to find function ${function_name} in ${path}"
    if grep -Fq "${pattern}" <<<"${function_text}"; then
        fail "did not expect function ${function_name} in ${path} to contain: ${pattern}"
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
assert_file_exists "${LTP_DIR}/score_whitelist_glibc_rv.txt"
assert_file_exists "${LTP_DIR}/score_blacklist_glibc_rv.txt"
assert_file_exists "${LTP_DIR}/score_whitelist_musl_la.txt"
assert_file_exists "${LTP_DIR}/score_blacklist_musl_la.txt"
assert_file_exists "${LTP_DIR}/score_whitelist_glibc_la.txt"
assert_file_exists "${LTP_DIR}/score_blacklist_glibc_la.txt"
assert_file_exists "${LTP_DIR}/musl_rv_curated_whitelist.txt"
assert_file_exists "${LTP_DIR}/musl_rv_curated_blacklist.txt"
assert_file_exists "${LTP_DIR}/curated_whitelist_glibc_rv.txt"
assert_file_exists "${LTP_DIR}/curated_blacklist_glibc_rv.txt"
assert_file_exists "${LTP_DIR}/curated_whitelist_musl_la.txt"
assert_file_exists "${LTP_DIR}/curated_blacklist_musl_la.txt"
assert_file_exists "${LTP_DIR}/curated_whitelist_glibc_la.txt"
assert_file_exists "${LTP_DIR}/curated_blacklist_glibc_la.txt"
assert_file_exists "${LTP_DIR}/pending_whitelist_rv_musl.txt"
assert_file_exists "${LTP_DIR}/pending_blacklist_rv_musl.txt"
assert_file_exists "${LTP_DIR}/pending_whitelist_glibc_rv.txt"
assert_file_exists "${LTP_DIR}/pending_blacklist_glibc_rv.txt"
assert_file_exists "${LTP_DIR}/musl_rv_batch_file_syscalls.txt"
assert_file_exists "${LTP_DIR}/musl_rv_batch_open_io.txt"
assert_file_exists "${LTP_DIR}/musl_rv_batch_stat.txt"
assert_file_exists "${LTP_DIR}/musl_rv_batch_memory.txt"
assert_file_exists "${LTP_DIR}/musl_rv_batch_sync.txt"
assert_file_exists "${LTP_DIR}/musl_rv_batch_path.txt"

for blacklist in \
    "${LTP_DIR}/score_blacklist.txt" \
    "${LTP_DIR}/score_blacklist_glibc_rv.txt" \
    "${LTP_DIR}/score_blacklist_musl_la.txt" \
    "${LTP_DIR}/score_blacklist_glibc_la.txt" \
    "${LTP_DIR}/musl_rv_curated_blacklist.txt" \
    "${LTP_DIR}/curated_blacklist_glibc_rv.txt" \
    "${LTP_DIR}/pending_blacklist_glibc_rv.txt" \
    "${LTP_DIR}/curated_blacklist_musl_la.txt" \
    "${LTP_DIR}/curated_blacklist_glibc_la.txt"; do
    assert_has_case_entry "${blacklist}"
done

assert_contains "${RUNNER}" "WHUSE_LTP_CURATED_WHITELIST"
assert_contains "${RUNNER}" "WHUSE_LTP_CURATED_BLACKLIST"
assert_contains "${RUNNER}" "WHUSE_LTP_PENDING_WHITELIST_RV_MUSL"
assert_contains "${RUNNER}" "WHUSE_LTP_PENDING_BLACKLIST_RV_GLIBC"
assert_contains "${RUNNER}" "WHUSE_LTP_AUTO_PROMOTE_SCORE"
assert_contains "${RUNNER}" "WHUSE_LTP_SCORE_PROMOTE_BATCH_MAX"
assert_contains "${RUNNER}" "WHUSE_STAGE2_REQUIRE_GUEST_SHUTDOWN"
assert_contains "${RUNNER}" "RUN_ID="
assert_contains "${RUNNER}" 'export WHUSE_STAGE2_BASIC_PROFILE="${WHUSE_STAGE2_BASIC_PROFILE:-full}"'
assert_contains "${RUNNER}" 'export WHUSE_STAGE2_BUSYBOX_PROFILE="${WHUSE_STAGE2_BUSYBOX_PROFILE:-full}"'
assert_contains "${RUNNER}" 'export WHUSE_STAGE2_GATE_LIBCTEST_SCOPE="${WHUSE_STAGE2_GATE_LIBCTEST_SCOPE:-full}"'
assert_contains "${RUNNER}" 'export WHUSE_STAGE2_LIBCBENCH_SCOPE="${WHUSE_STAGE2_LIBCBENCH_SCOPE:-full}"'
assert_contains "${RUNNER}" 'export WHUSE_STAGE2_LMBENCH_SCOPE="${WHUSE_STAGE2_LMBENCH_SCOPE:-full}"'
assert_contains "${RUNNER}" "ltp-riscv-curated"
assert_contains "${RUNNER}" "ltp-riscv-pending"
assert_contains "${RUNNER}" "candidate_label=\"\${arch}-ltp-\${candidate_runtime}-\${WHUSE_LTP_PROFILE}\""
assert_contains "${RUNNER}" "pass-candidates-\${RUN_ID}.txt"
assert_contains "${RUNNER}" "bad-candidates-\${RUN_ID}.txt"
assert_contains "${RUNNER}" "conf-candidates-\${RUN_ID}.txt"
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
assert_contains "${RUNNER}" "build_stage2_local_env()"
assert_contains "${RUNNER}" "inject_stage2_local_env"
assert_contains "${RUNNER}" "inject_ltp_runtime_files()"
assert_contains "${RUNNER}" "runtime_hint"
assert_contains "${RUNNER}" "inject_ltp_target_score_files()"
assert_contains "${RUNNER}" "write_runtime_image_file \"\${image}\" \"\${whitelist_image_path}\" \"\${whitelist_host}\""
assert_contains "${RUNNER}" "write_runtime_image_file \"\${image}\" \"\${blacklist_image_path}\" \"\${blacklist_host}\""
assert_contains "${RUNNER}" "write_runtime_image_config \"\${image}\" \"/musl/.whuse_ltp_whitelist_\${runtime}\" \"\${whitelist_image_path}\""
assert_contains "${RUNNER}" "write_runtime_image_config \"\${image}\" \"/musl/.whuse_ltp_blacklist_\${runtime}\" \"\${blacklist_image_path}\""
assert_contains "${RUNNER}" "write_runtime_image_config \"\${image}\" \"/musl/.whuse_ltp_whitelist_\${runtime_hint}\" \"\${ltp_whitelist_guest_path}\""
assert_contains "${RUNNER}" "write_runtime_image_config \"\${image}\" \"/musl/.whuse_ltp_blacklist_\${runtime_hint}\" \"\${ltp_blacklist_guest_path}\""
assert_contains "${RUNNER}" "write_runtime_image_config \"\${image}\" \"/musl/.whuse_ltp_whitelist\" \"/musl/ltp_score_whitelist.txt\""
assert_contains "${RUNNER}" "write_runtime_image_config \"\${image}\" \"/musl/.whuse_ltp_blacklist\" \"/musl/ltp_score_blacklist.txt\""
assert_contains "${RUNNER}" "ltp_score_whitelist_for_target()"
assert_contains "${RUNNER}" "ltp_score_blacklist_for_target()"
assert_contains "${RUNNER}" "ltp_curated_whitelist_for_target()"
assert_contains "${RUNNER}" "ltp_curated_blacklist_for_target()"
assert_contains "${RUNNER}" "ltp_pending_whitelist_for_target()"
assert_contains "${RUNNER}" "ltp_pending_blacklist_for_target()"
assert_contains "${RUNNER}" "select_ltp_score_promotion_candidates()"
assert_contains "${RUNNER}" "run_ltp_riscv_score_gate()"
assert_contains "${RUNNER}" "apply_ltp_curated_to_score_promotions()"
suite_done_fallback_count="$(grep -Fc 'if [[ "${suite_done_seen}" == "0" ]] && rg -q "whuse-oscomp-suite-done" "${text_log}"; then' "${RUNNER}")"
if (( suite_done_fallback_count < 2 )); then
    fail "expected suite-done fallback check in both ltp runner and score gate paths"
fi
assert_contains "${RUNNER}" "ltp_whitelist_for_target_profile()"
assert_contains "${RUNNER}" "ltp_blacklist_for_target_profile()"
assert_contains "${RUNNER}" "score_whitelist_glibc_rv.txt"
assert_contains "${RUNNER}" "score_whitelist_musl_la.txt"
assert_contains "${RUNNER}" "score_whitelist_glibc_la.txt"
assert_contains "${RUNNER}" "curated_whitelist_glibc_rv.txt"
assert_contains "${RUNNER}" "pending_whitelist_glibc_rv.txt"
assert_contains "${RUNNER}" "curated_whitelist_musl_la.txt"
assert_contains "${RUNNER}" "curated_whitelist_glibc_la.txt"
assert_contains "${RUNNER}" "WHUSE_LTP_SCORE_WHITELIST_RV_GLIBC"
assert_contains "${RUNNER}" "WHUSE_LTP_CURATED_WHITELIST_LA_GLIBC"
assert_contains "${RUNNER}" "STAGE2_USE_IMAGE_COPY_WAS_SET=1"
assert_contains "${RUNNER}" "prepare_ltp_runtime_image()"
assert_contains "${RUNNER}" "if [[ \"\${STAGE2_USE_IMAGE_COPY_WAS_SET}\" != \"1\" ]]; then"
assert_contains "${RUNNER}" "WHUSE_STAGE2_USE_IMAGE_COPY=1"
assert_contains "${RUNNER}" "if [[ \"\${effective_profile}\" == \"full\" || \"\${effective_profile}\" == \"ltp\" ]]; then"
assert_contains "${RUNNER}" "inject_ltp_target_score_files \"\${arch}\" \"\${runtime_image}\""
assert_contains "${RUNNER}" "prepare_ltp_runtime_image \"\${RV_IMG}\""
assert_contains "${RUNNER}" "inject_ltp_runtime_config \"\${runtime_image}\" \"\${ltp_profile}\" \"\${ltp_whitelist}\" \"\${ltp_blacklist}\" \"\${runtime_filter}\""
assert_contains "${RUNNER}" "export WHUSE_STAGE2_FULL_MAX_GROUP=\"\${WHUSE_STAGE2_FULL_MAX_GROUP:-all}\""
assert_contains "${RUNNER}" "WHUSE_STAGE2_FULL_MAX_GROUP=\${WHUSE_STAGE2_FULL_MAX_GROUP}"
assert_contains "${RUNNER}" "WHUSE_OSCOMP_RUNTIME_FILTER=\${WHUSE_OSCOMP_RUNTIME_FILTER}"
assert_contains "${RUNNER}" "basic profile semantic check fallback ok"
assert_contains "${RUNNER}" "count_case_filter_entries"
assert_contains "${RUNNER}" "has_kernel_panic_or_init_crash"
assert_contains "${RUNNER}" "panicked at|kernel panic|FATAL KERNEL TRAP|pid 1 \\(init\\).*(trap|crash)|^panic(:| |$)"
assert_contains "${RUNNER}" "refusing to overwrite protected score whitelist"
assert_contains "${RUNNER}" "skip candidate apply: WHUSE_OSCOMP_RUNTIME_FILTER=both cannot map pass/bad to a single target curated file"
assert_contains "${RUNNER}" "skip pending promotion: WHUSE_OSCOMP_RUNTIME_FILTER=both cannot map pass-candidates to a single runtime pending list"
assert_contains "${RUNNER}" "skip pending promotion apply: no pass-candidates"
assert_contains "${RUNNER}" "pending->curated promoted"
assert_contains "${RUNNER}" "pending apply requires pending whitelist, curated whitelist, and curated blacklist targets"
assert_contains "${RUNNER}" "remove_cases_from_list_preserve_order \"\${curated_blacklist}\" \"\${pass_candidates}\" \"\${curated_blacklist_tmp}\""
assert_contains "${RUNNER}" "ltp_curated_blacklist_for_target \"rv\" \"\${runtime_filter}\""
assert_contains "${RUNNER}" "curated stability regression detected"
assert_contains "${RUNNER}" "curated->score promoted"
assert_contains "${RUNNER}" "score gate failed for candidate batch"
assert_contains "${RUNNER}" "score alarm:"
assert_contains "${REPO_ROOT}/crates/kernel-core/src/lib_loongarch.inc.rs" "if [ \\\"\$line\\\" = \\\"hwclock\\\" ]; then"
assert_contains "${REPO_ROOT}/crates/kernel-core/src/lib_loongarch.inc.rs" "glibc-busybox-not-priority"
assert_contains "${REPO_ROOT}/crates/kernel-core/src/lib_loongarch.inc.rs" "glibc-libctest-not-scored"
assert_not_contains "${REPO_ROOT}/crates/kernel-core/src/lib_loongarch.inc.rs" "glibc-ltp-not-scored"
assert_contains "${REPO_ROOT}/crates/kernel-core/src/lib_loongarch.inc.rs" "loongarch-iozone-not-scored"
assert_contains "${REPO_ROOT}/crates/kernel-core/src/lib_loongarch.inc.rs" "loongarch-lmbench-not-scored"
assert_contains "${REPO_ROOT}/crates/kernel-core/src/lib_loongarch.inc.rs" "loongarch-unixbench-not-priority"
assert_contains "${REPO_ROOT}/crates/kernel-core/src/lib_loongarch.inc.rs" "loongarch-netperf-not-priority"
assert_contains "${REPO_ROOT}/crates/kernel-core/src/lib_loongarch.inc.rs" "loongarch-iperf-not-priority"
assert_contains "${REPO_ROOT}/crates/kernel-core/src/lib_loongarch.inc.rs" "loongarch-cyclic-not-priority"
assert_contains "${REPO_ROOT}/crates/kernel-core/src/lib_loongarch.inc.rs" "run_basic_testsuite_runtime_entry()"
assert_contains "${REPO_ROOT}/crates/kernel-core/src/lib_loongarch.inc.rs" "run_basic_testsuite_runtime_entry \\\"\$runtime\\\" \\\"\$timeout_s\\\""
assert_contains "${REPO_ROOT}/crates/kernel-core/src/lib_loongarch.inc.rs" "/musl/busybox sh ./run-all.sh"
assert_contains "${REPO_ROOT}/crates/kernel-core/src/lib_loongarch.inc.rs" "run_ltp_step_runtime"
assert_contains "${REPO_ROOT}/crates/kernel-core/src/lib_loongarch.inc.rs" "whuse_ltp_count_lines()"
assert_contains "${REPO_ROOT}/crates/kernel-core/src/lib_loongarch.inc.rs" "whuse_ltp_list_contains()"
assert_contains "${REPO_ROOT}/crates/kernel-core/src/lib_loongarch.inc.rs" "whuse_ltp_cleanup_case_tree()"
assert_contains "${REPO_ROOT}/crates/kernel-core/src/lib_loongarch.inc.rs" 'case_status=\"$case_log.status\"'
assert_contains "${REPO_ROOT}/crates/kernel-core/src/lib_loongarch.inc.rs" 'while [ ! -f \"$case_status\" ]'
assert_not_contains "${REPO_ROOT}/crates/kernel-core/src/lib_loongarch.inc.rs" "/musl/busybox wc -l < \"\$whitelist\""
assert_not_contains "${REPO_ROOT}/crates/kernel-core/src/lib_loongarch.inc.rs" "/musl/busybox grep -Fqx \"\$case_name\" \"\$WHUSE_LTP_WHITELIST\""
assert_not_contains "${REPO_ROOT}/crates/kernel-core/src/lib_loongarch.inc.rs" "/musl/busybox grep -Fqx \"\$case_name\" \"\$WHUSE_LTP_BLACKLIST\""
assert_not_contains "${REPO_ROOT}/crates/kernel-core/src/lib_loongarch.inc.rs" '/musl/busybox timeout \"$WHUSE_LTP_CASE_TIMEOUT\" \"$case_path\" >\"$case_log\" 2>&1'
assert_contains "${REPO_ROOT}/crates/kernel-core/src/lib_loongarch.inc.rs" "run_loongarch_full_ltp_step"
assert_contains "${REPO_ROOT}/crates/kernel-core/src/lib_loongarch.inc.rs" "render_oscomp_ltp_step_helper_script("
assert_contains "${REPO_ROOT}/crates/kernel-core/src/lib_loongarch.inc.rs" "OSCOMP_LTP_STEP_HELPER_PATH"
assert_function_contains "${REPO_ROOT}/crates/kernel-core/src/lib_loongarch.inc.rs" "select_oscomp_suite_script" "render_selected_oscomp_suite_script("
assert_function_not_contains "${REPO_ROOT}/crates/kernel-core/src/lib_loongarch.inc.rs" "select_oscomp_suite_script" "render_oscomp_official_suite_script("
assert_function_contains "${REPO_ROOT}/crates/kernel-core/src/lib_loongarch.inc.rs" "render_selected_oscomp_suite_script" "render_oscomp_internal_full_suite_script("
assert_contains "${REPO_ROOT}/crates/kernel-core/src/lib_loongarch.inc.rs" "/musl/busybox sh /tmp/whuse-oscomp-ltp-step.sh"
assert_function_not_contains "${REPO_ROOT}/crates/kernel-core/src/lib_loongarch.inc.rs" "render_oscomp_internal_full_suite_script" "script.push_str(ltp_helpers);"
assert_contains "${REPO_ROOT}/crates/kernel-core/src/lib_loongarch.inc.rs" "WHUSE_STAGE2_BASIC_PROFILE=\${WHUSE_STAGE2_BASIC_PROFILE:-__WHUSE_STAGE2_BASIC_PROFILE__}"
assert_contains "${REPO_ROOT}/crates/kernel-core/src/lib_loongarch.inc.rs" "WHUSE_STAGE2_BUSYBOX_PROFILE=\${WHUSE_STAGE2_BUSYBOX_PROFILE:-__WHUSE_STAGE2_BUSYBOX_PROFILE__}"
assert_contains "${REPO_ROOT}/crates/kernel-core/src/lib_loongarch.inc.rs" 'case "$WHUSE_OSCOMP_PROFILE:$WHUSE_STAGE2_BASIC_PROFILE" in'
assert_contains "${REPO_ROOT}/crates/kernel-core/src/lib_loongarch.inc.rs" "full:smoke)"
assert_contains "${REPO_ROOT}/crates/kernel-core/src/lib_loongarch.inc.rs" "run_busybox_smoke_step()"
assert_contains "${REPO_ROOT}/crates/kernel-core/src/lib_loongarch.inc.rs" "WHUSE_STAGE2_GATE_LIBCTEST_SCOPE"
assert_contains "${REPO_ROOT}/crates/kernel-core/src/lib_loongarch.inc.rs" "run_libctest_smoke_step"
assert_contains "${REPO_ROOT}/crates/kernel-core/src/lib_loongarch.inc.rs" "WHUSE_STAGE2_LIBCBENCH_SCOPE"
assert_contains "${REPO_ROOT}/crates/kernel-core/src/lib_loongarch.inc.rs" "run_libcbench_smoke_step"
assert_contains "${REPO_ROOT}/crates/kernel-core/src/lib_loongarch.inc.rs" "WHUSE_STAGE2_LMBENCH_SCOPE"
assert_contains "${REPO_ROOT}/crates/kernel-core/src/lib_loongarch.inc.rs" "run_lmbench_smoke_step"
assert_contains "${REPO_ROOT}/crates/kernel-core/src/lib_loongarch.inc.rs" "whuse-oscomp-shell-suite-begin"
assert_contains "${REPO_ROOT}/crates/kernel-core/src/lib_loongarch.inc.rs" ". /tmp/whuse-oscomp-suite.sh"
assert_contains "${REPO_ROOT}/crates/kernel-core/src/lib_loongarch.inc.rs" "whuse-oscomp-shell-suite-end:\$rc"
assert_contains "${REPO_ROOT}/crates/syscall/src/lib.rs" "whuse-busybox:utimensat-shortcut"
assert_not_contains "${RUNNER}" "cp \"\${pass_candidates}\" \"\${REPO_ROOT}/tools/oscomp/ltp/score_whitelist.txt\""
assert_not_contains "${RUNNER}" "cp \"\${bad_candidates}\" \"\${REPO_ROOT}/tools/oscomp/ltp/score_blacklist.txt\""
assert_contains "${REPO_ROOT}/crates/kernel-core/src/lib_riscv.inc.rs" "whuse-oscomp-step-skip:iozone_testcode.sh:riscv-known-panic"
assert_contains "${REPO_ROOT}/crates/kernel-core/src/lib_riscv.inc.rs" "glibc-libctest-known-oom"
assert_not_contains "${REPO_ROOT}/crates/kernel-core/src/lib_riscv.inc.rs" "glibc-ltp-not-scored"
assert_contains "${REPO_ROOT}/crates/kernel-core/src/lib_riscv.inc.rs" "run_ltp_step_runtime"
assert_contains "${REPO_ROOT}/crates/kernel-core/src/lib_riscv.inc.rs" "run_basic_testsuite_runtime_entry()"
assert_contains "${REPO_ROOT}/crates/kernel-core/src/lib_riscv.inc.rs" "run_basic_testsuite_runtime_entry \\\"\$runtime\\\" \\\"\$timeout_s\\\""
assert_contains "${REPO_ROOT}/crates/kernel-core/src/lib_riscv.inc.rs" "for case_name in \$tests; do"
assert_contains "${REPO_ROOT}/crates/kernel-core/src/lib_riscv.inc.rs" "case_path="
assert_contains "${REPO_ROOT}/crates/kernel-core/src/lib_riscv.inc.rs" "waitpid"
assert_contains "${REPO_ROOT}/crates/kernel-core/src/lib_riscv.inc.rs" "/musl/busybox sh ./basic/run-all.sh"
assert_not_contains "${REPO_ROOT}/crates/kernel-core/src/lib_riscv.inc.rs" 'if [ "$WHUSE_OSCOMP_PROFILE" = "basic" ] && [ "$marker_script" = "basic_testcode.sh" ]'
assert_not_contains "${RUNNER}" "rg -q \"KERNEL PANIC|panic|pid 1 \\(init\\).*trap\""
assert_order_in_array_block "${RUNNER}" "riscv_full_root_steps" "iozone_testcode.sh" "ltp_testcode.sh"
assert_order_in_array_block "${RUNNER}" "riscv_full_root_steps" "ltp_testcode.sh" "libctest_testcode.sh"
assert_order_in_array_block "${RUNNER}" "riscv_full_root_steps" "ltp_testcode.sh" "lua_testcode.sh"
assert_order_in_array_block "${RUNNER}" "riscv_full_root_steps" "ltp_testcode.sh" "libc-bench"
assert_order_in_array_block "${RUNNER}" "riscv_full_root_steps" "ltp_testcode.sh" "lmbench_testcode.sh"
assert_order_in_array_block "${RUNNER}" "loongarch_full_root_steps" "basic_testcode.sh" "busybox_testcode.sh"
assert_order_in_array_block "${RUNNER}" "loongarch_full_root_steps" "busybox_testcode.sh" "ltp_testcode.sh"
assert_order_in_array_block "${RUNNER}" "loongarch_full_root_steps" "ltp_testcode.sh" "libctest_testcode.sh"
assert_order_in_array_block "${RUNNER}" "loongarch_full_root_steps" "libctest_testcode.sh" "lua_testcode.sh"
assert_order_in_array_block "${RUNNER}" "loongarch_full_root_steps" "lua_testcode.sh" "libc-bench"
assert_order_in_array_block "${RUNNER}" "loongarch_full_root_steps" "ltp_testcode.sh" "iozone_testcode.sh"
assert_order_in_array_block "${RUNNER}" "loongarch_full_root_steps" "iozone_testcode.sh" "lmbench_testcode.sh"

echo "ok"
