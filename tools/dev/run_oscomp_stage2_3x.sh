#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"

RUNS="${RUNS:-3}"
TIMEOUT_SECS="${TIMEOUT_SECS:-3600}"
WHUSE_STAGE2_IMAGE_POLICY="${WHUSE_STAGE2_IMAGE_POLICY:-auto}"
WHUSE_OSCOMP_COMPAT="${WHUSE_OSCOMP_COMPAT:-0}"
WHUSE_STAGE2_STOP_ON_SUITE_DONE="${WHUSE_STAGE2_STOP_ON_SUITE_DONE:-1}"

if [[ "${WHUSE_OSCOMP_COMPAT}" != "0" ]]; then
    echo "WHUSE_OSCOMP_COMPAT must be 0 for stage2 real-execution runs" >&2
    exit 1
fi

required_steps=(
    "time-test"
    "busybox_testcode.sh"
    "iozone_testcode.sh"
    "libctest_testcode.sh"
    "libc-bench"
    "lmbench_testcode.sh"
    "lua_testcode.sh"
    "unixbench_testcode.sh"
    "netperf_testcode.sh"
    "iperf_testcode.sh"
    "cyclic_testcode.sh"
)

count_matches() {
    local pattern="$1"
    local log_file="$2"
    local out
    out="$(rg -c "${pattern}" "${log_file}" || true)"
    if [[ -z "${out}" ]]; then
        echo "0"
    else
        echo "${out}"
    fi
}

TS="$(date +%Y%m%d-%H%M%S)"

suite_done_runs=0
panic_or_init_trap_runs=0
step_close_incomplete_runs=0
script_nonzero_runs=0
total_fail=0
total_error=0
total_step_timeout=0
total_bench_watchdog_timeout=0

cd "${REPO_ROOT}"

for i in $(seq 1 "${RUNS}"); do
    run_out="/tmp/rv-stage2-3x-${TS}-run${i}.out"
    echo "[3x] run ${i}/${RUNS} ..."
    set +e
    TIMEOUT_SECS="${TIMEOUT_SECS}" \
    WHUSE_STAGE2_IMAGE_POLICY="${WHUSE_STAGE2_IMAGE_POLICY}" \
    WHUSE_OSCOMP_COMPAT="${WHUSE_OSCOMP_COMPAT}" \
    WHUSE_STAGE2_STOP_ON_SUITE_DONE="${WHUSE_STAGE2_STOP_ON_SUITE_DONE}" \
    tools/dev/run_oscomp_stage2.sh riscv | tee "${run_out}"
    run_rc=${PIPESTATUS[0]}
    set -e

    if (( run_rc != 0 )); then
        script_nonzero_runs=$((script_nonzero_runs + 1))
    fi

    log_path="$(awk '/^\[rv\] log: / { print $3 }' "${run_out}" | tail -n1)"
    if [[ -z "${log_path}" || ! -f "${log_path}" ]]; then
        echo "[3x] run ${i}: unable to locate rv log path from ${run_out}" >&2
        panic_or_init_trap_runs=$((panic_or_init_trap_runs + 1))
        continue
    fi

    text_log="${log_path%.log}.strings.log"
    if [[ ! -f "${text_log}" ]]; then
        strings "${log_path}" >"${text_log}" || true
    fi

    suite_done=0
    if rg -q "whuse-oscomp-suite-done" "${text_log}"; then
        suite_done=1
        suite_done_runs=$((suite_done_runs + 1))
    fi

    panic_or_init=0
    if rg -q "KERNEL PANIC|panic|pid 1 \(init\).*trap" "${text_log}"; then
        panic_or_init=1
        panic_or_init_trap_runs=$((panic_or_init_trap_runs + 1))
    fi

    step_timeout_count="$(count_matches "whuse-oscomp-step-timeout:" "${text_log}")"
    fail_count="$(count_matches "testcase .* fail" "${text_log}")"
    error_count="$(count_matches "testcase .* error" "${text_log}")"
    bench_watchdog_count="$(count_matches "whuse-oscomp-(lmbench|bench)-marker:watchdog-timeout:" "${text_log}")"

    total_step_timeout=$((total_step_timeout + step_timeout_count))
    total_fail=$((total_fail + fail_count))
    total_error=$((total_error + error_count))
    total_bench_watchdog_timeout=$((total_bench_watchdog_timeout + bench_watchdog_count))

    step_close_ok=1
    for step in "${required_steps[@]}"; do
        if ! rg -q "whuse-oscomp-step-begin:${step}" "${text_log}"; then
            step_close_ok=0
            break
        fi
        if ! rg -q "whuse-oscomp-step-end:${step}|whuse-oscomp-step-skip:${step}|whuse-oscomp-step-timeout:${step}" "${text_log}"; then
            step_close_ok=0
            break
        fi
    done
    if (( step_close_ok == 0 )); then
        step_close_incomplete_runs=$((step_close_incomplete_runs + 1))
    fi

    echo "[3x] run ${i} summary: suite_done=${suite_done} panic_or_init_trap=${panic_or_init} step_close_ok=${step_close_ok} fail=${fail_count} error=${error_count} step-timeout=${step_timeout_count} bench-watchdog-timeout=${bench_watchdog_count} rc=${run_rc} log=${log_path}"
done

echo "[3x] aggregate summary: runs=${RUNS} suite_done_runs=${suite_done_runs} panic_or_init_trap_runs=${panic_or_init_trap_runs} step_close_incomplete_runs=${step_close_incomplete_runs} total_fail=${total_fail} total_error=${total_error} total_step-timeout=${total_step_timeout} total_bench-watchdog-timeout=${total_bench_watchdog_timeout} script_nonzero_runs=${script_nonzero_runs}"

if (( suite_done_runs != 3 )); then
    echo "[3x] FAIL: suite_done_runs != 3" >&2
    exit 1
fi
if (( panic_or_init_trap_runs != 0 )); then
    echo "[3x] FAIL: panic_or_init_trap_runs != 0" >&2
    exit 1
fi
if (( step_close_incomplete_runs != 0 )); then
    echo "[3x] FAIL: step_close_incomplete_runs != 0" >&2
    exit 1
fi
if (( total_fail >= 3 )); then
    echo "[3x] FAIL: total_fail >= 3" >&2
    exit 1
fi
if (( total_error != 0 )); then
    echo "[3x] FAIL: total_error != 0" >&2
    exit 1
fi

echo "[3x] PASS: suite_done=3/3, no panic/init-trap, total_fail<3, total_error=0"
