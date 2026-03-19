#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
XTASK_CMD=(cargo run --manifest-path "${REPO_ROOT}/tools/xtask/Cargo.toml" --)

MODE="${1:-riscv}"
TIMEOUT_SECS="${TIMEOUT_SECS:-3600}"
TS="$(date +%Y%m%d-%H%M%S)"

export WHUSE_OSCOMP_DOCKER_IMAGE="${WHUSE_OSCOMP_DOCKER_IMAGE:-docker.educg.net/cg/os-contest:20260104}"
export WHUSE_OSCOMP_COMPAT="${WHUSE_OSCOMP_COMPAT:-0}"
export WHUSE_STAGE2_USE_IMAGE_COPY="${WHUSE_STAGE2_USE_IMAGE_COPY:-0}"
export WHUSE_STAGE2_STOP_ON_SUITE_DONE="${WHUSE_STAGE2_STOP_ON_SUITE_DONE:-1}"
WHUSE_STAGE2_IMAGE_POLICY="${WHUSE_STAGE2_IMAGE_POLICY:-auto}"

RV_IMG="${REPO_ROOT}/target/oscomp/sdcard-rv.img"
LA_IMG="${REPO_ROOT}/target/oscomp/sdcard-la.img"

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

required_musl_entries=(
    "busybox"
    "busybox_testcode.sh"
    "busybox_cmd.txt"
    "iozone_testcode.sh"
    "libctest_testcode.sh"
    "libc-bench"
    "lmbench_testcode.sh"
    "lua_testcode.sh"
    "unixbench_testcode.sh"
    "netperf_testcode.sh"
    "iperf_testcode.sh"
    "cyclictest_testcode.sh"
)

runtime_images=()

cleanup_runtime_images() {
    for img in "${runtime_images[@]}"; do
        rm -f "${img}"
    done
}

trap cleanup_runtime_images EXIT

check_image_complete() {
    local arch="$1"
    local image="$2"
    if [[ ! -f "${image}" ]]; then
        echo "[${arch}] image missing: ${image}"
        return 1
    fi
    local musl_listing
    local basic_listing
    if ! musl_listing="$(debugfs -R "ls -l /musl" "${image}" 2>/dev/null)"; then
        echo "[${arch}] debugfs /musl failed: ${image}"
        return 1
    fi
    if ! basic_listing="$(debugfs -R "ls -l /musl/basic" "${image}" 2>/dev/null)"; then
        echo "[${arch}] debugfs /musl/basic failed: ${image}"
        return 1
    fi
    local missing=()
    local entry
    for entry in "${required_musl_entries[@]}"; do
        if ! grep -Fq "${entry}" <<<"${musl_listing}"; then
            missing+=("/musl/${entry}")
        fi
    done
    if ! grep -Fq "test_all.sh" <<<"${musl_listing}" && ! grep -Fq "run-all.sh" <<<"${basic_listing}"; then
        missing+=("/musl/test_all.sh(or /musl/basic/run-all.sh)")
    fi
    if (( ${#missing[@]} != 0 )); then
        echo "[${arch}] image incomplete: missing ${missing[*]}"
        return 1
    fi
    echo "[${arch}] image completeness ok: ${image}"
    return 0
}

ensure_oscomp_images() {
    local requested_arches=()
    case "${MODE}" in
    riscv) requested_arches=("rv") ;;
    loongarch) requested_arches=("la") ;;
    both) requested_arches=("rv" "la") ;;
    *)
        echo "usage: $0 [riscv|loongarch|both]" >&2
        exit 2
        ;;
    esac

    local needs_rebuild=0
    local arch
    for arch in "${requested_arches[@]}"; do
        if [[ "${arch}" == "rv" ]]; then
            check_image_complete "rv" "${RV_IMG}" || needs_rebuild=1
        else
            check_image_complete "la" "${LA_IMG}" || needs_rebuild=1
        fi
    done

    case "${WHUSE_STAGE2_IMAGE_POLICY}" in
    always)
        echo "image policy=always, rebuilding oscomp images"
        "${XTASK_CMD[@]}" oscomp-images
        ;;
    never)
        if (( needs_rebuild != 0 )); then
            echo "image policy=never, but required images are missing/incomplete" >&2
            return 1
        fi
        ;;
    auto)
        if (( needs_rebuild != 0 )); then
            echo "image policy=auto, rebuilding oscomp images due to missing/incomplete image"
            "${XTASK_CMD[@]}" oscomp-images
        else
            echo "image policy=auto, using existing validated image(s)"
        fi
        ;;
    *)
        echo "invalid WHUSE_STAGE2_IMAGE_POLICY=${WHUSE_STAGE2_IMAGE_POLICY} (expected: auto|always|never)" >&2
        return 1
        ;;
    esac

    for arch in "${requested_arches[@]}"; do
        if [[ "${arch}" == "rv" ]]; then
            check_image_complete "rv" "${RV_IMG}" || return 1
        else
            check_image_complete "la" "${LA_IMG}" || return 1
        fi
    done
}

prepare_runtime_image() {
    local arch="$1"
    local src="$2"
    if [[ "${WHUSE_STAGE2_USE_IMAGE_COPY}" != "1" ]]; then
        echo "${src}"
        return
    fi
    local dst="/tmp/whuse-${arch}-stage2-${TS}.img"
    cp --reflink=auto "${src}" "${dst}"
    runtime_images+=("${dst}")
    echo "${dst}"
}

run_arch() {
    local arch="$1"
    local image="$2"
    local xtask_cmd="$3"
    local log="/tmp/${arch}-stage2-${TS}.log"
    local text_log="/tmp/${arch}-stage2-${TS}.strings.log"
    local runtime_image
    local suite_done_seen=0
    local terminated_by_suite_done=0
    runtime_image="$(prepare_runtime_image "${arch}" "${image}")"

    echo "[${arch}] running ${xtask_cmd}, timeout=${TIMEOUT_SECS}s, image=${runtime_image}, stop-on-suite-done=${WHUSE_STAGE2_STOP_ON_SUITE_DONE}"
    if [[ "${WHUSE_STAGE2_STOP_ON_SUITE_DONE}" == "1" ]]; then
        local runner_pid
        setsid timeout "${TIMEOUT_SECS}s" env WHUSE_DISK_IMAGE="${runtime_image}" "${XTASK_CMD[@]}" "${xtask_cmd}" >"${log}" 2>&1 &
        runner_pid=$!
        while kill -0 "${runner_pid}" 2>/dev/null; do
            if [[ -f "${log}" ]] && grep -a -q "whuse-oscomp-suite-done" "${log}"; then
                suite_done_seen=1
                terminated_by_suite_done=1
                kill -TERM -- "-${runner_pid}" 2>/dev/null || true
                for _ in $(seq 1 10); do
                    if ! kill -0 "${runner_pid}" 2>/dev/null; then
                        break
                    fi
                    sleep 1
                done
                if kill -0 "${runner_pid}" 2>/dev/null; then
                    kill -KILL -- "-${runner_pid}" 2>/dev/null || true
                fi
                break
            fi
            sleep 1
        done
        wait "${runner_pid}" || true
    else
        timeout "${TIMEOUT_SECS}s" env WHUSE_DISK_IMAGE="${runtime_image}" "${XTASK_CMD[@]}" "${xtask_cmd}" >"${log}" 2>&1 || true
    fi
    strings "${log}" >"${text_log}" || true
    echo "[${arch}] log: ${log}"

    if rg -q "KERNEL PANIC|panic|pid 1 \(init\).*trap" "${text_log}"; then
        echo "[${arch}] detected kernel panic or init crash" >&2
        rg "KERNEL PANIC|panic|pid 1 \(init\).*trap" "${text_log}" >&2 || true
        return 1
    fi

    local ok=0

    if rg -q "whuse-oscomp-suite-done" "${text_log}"; then
        suite_done_seen=1
        echo "[${arch}] suite-done ok"
    else
        echo "[${arch}] missing whuse-oscomp-suite-done" >&2
        ok=1
    fi

    for step in "${required_steps[@]}"; do
        if rg -q "whuse-oscomp-step-begin:${step}" "${text_log}"; then
            echo "[${arch}] step-begin ok: ${step}"
        else
            echo "[${arch}] missing step-begin: ${step}" >&2
            ok=1
        fi
        if rg -q "whuse-oscomp-step-end:${step}|whuse-oscomp-step-skip:${step}|whuse-oscomp-step-timeout:${step}" "${text_log}"; then
            echo "[${arch}] step-close ok: ${step}"
        else
            echo "[${arch}] missing step-close: ${step}" >&2
            ok=1
        fi
    done

    echo "[${arch}] marker summary:"
    rg "whuse-oscomp-step-(begin|end|timeout|skip)|whuse-oscomp-suite-done" "${text_log}" || true

    local timeout_count
    local fail_count
    local error_count
    local bench_watchdog_count
    timeout_count="$(count_matches "whuse-oscomp-step-timeout:" "${text_log}")"
    fail_count="$(count_matches "testcase .* fail" "${text_log}")"
    error_count="$(count_matches "testcase .* error" "${text_log}")"
    bench_watchdog_count="$(count_matches "whuse-oscomp-(lmbench|bench)-marker:watchdog-timeout:" "${text_log}")"
    echo "[${arch}] quality summary: step-timeout=${timeout_count} testcase-fail=${fail_count} testcase-error=${error_count} bench-watchdog-timeout=${bench_watchdog_count} suite_done_seen=${suite_done_seen} terminated_by_suite_done=${terminated_by_suite_done}"

    return "${ok}"
}

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

cd "${REPO_ROOT}"

echo "building/checking workspace..."
make check

echo "preparing oscomp images (policy=${WHUSE_STAGE2_IMAGE_POLICY})..."
ensure_oscomp_images

case "${MODE}" in
riscv)
    run_arch "rv" "${RV_IMG}" "qemu-riscv"
    ;;
loongarch)
    run_arch "la" "${LA_IMG}" "qemu-loongarch"
    ;;
both)
    run_arch "rv" "${RV_IMG}" "qemu-riscv"
    run_arch "la" "${LA_IMG}" "qemu-loongarch"
    ;;
*)
    echo "usage: $0 [riscv|loongarch|both]" >&2
    exit 2
    ;;
esac
