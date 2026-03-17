#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"

MODE="${1:-both}"
TIMEOUT_SECS="${TIMEOUT_SECS:-3600}"
TS="$(date +%Y%m%d-%H%M%S)"

export WHUSE_OSCOMP_DOCKER_IMAGE="${WHUSE_OSCOMP_DOCKER_IMAGE:-docker.educg.net/cg/os-contest:20260104}"
export WHUSE_OSCOMP_COMPAT="${WHUSE_OSCOMP_COMPAT:-0}"
export WHUSE_STAGE1_USE_IMAGE_COPY="${WHUSE_STAGE1_USE_IMAGE_COPY:-0}"

RV_IMG="${REPO_ROOT}/target/oscomp/sdcard-rv.img"
LA_IMG="${REPO_ROOT}/target/oscomp/sdcard-la.img"

if [[ "${WHUSE_OSCOMP_COMPAT}" != "0" ]]; then
	echo "WHUSE_OSCOMP_COMPAT must be 0 for stage1 real-execution runs" >&2
	exit 1
fi

required_steps=(
	"time-test"
	"busybox_testcode.sh"
	"iozone_testcode.sh"
)

runtime_images=()

cleanup_runtime_images() {
	for img in "${runtime_images[@]}"; do
		rm -f "${img}"
	done
}

trap cleanup_runtime_images EXIT

prepare_runtime_image() {
	local arch="$1"
	local src="$2"
	if [[ "${WHUSE_STAGE1_USE_IMAGE_COPY}" != "1" ]]; then
		echo "${src}"
		return
	fi
	local dst="/tmp/whuse-${arch}-stage1-${TS}.img"
	cp --reflink=auto "${src}" "${dst}"
	runtime_images+=("${dst}")
	echo "${dst}"
}

run_arch() {
	local arch="$1"
	local image="$2"
	local xtask_cmd="$3"
	local log="/tmp/${arch}-stage1-${TS}.log"
	local text_log="/tmp/${arch}-stage1-${TS}.strings.log"
	local runtime_image
	runtime_image="$(prepare_runtime_image "${arch}" "${image}")"

	echo "[${arch}] running ${xtask_cmd}, timeout=${TIMEOUT_SECS}s, image=${runtime_image}"
	timeout "${TIMEOUT_SECS}s" env WHUSE_DISK_IMAGE="${runtime_image}" cargo xtask "${xtask_cmd}" >"${log}" 2>&1 || true
	strings "${log}" >"${text_log}" || true
	echo "[${arch}] log: ${log}"

	if rg -q "KERNEL PANIC|panic|pid 1 \(init\).*trap" "${text_log}"; then
		echo "[${arch}] detected kernel panic or init crash" >&2
		rg "KERNEL PANIC|panic|pid 1 \(init\).*trap" "${text_log}" >&2 || true
		return 1
	fi

	local ok=0
	for step in "${required_steps[@]}"; do
		if rg -q "whuse-oscomp-step-begin:${step}" "${text_log}"; then
			echo "[${arch}] step-begin ok: ${step}"
		else
			echo "[${arch}] missing step-begin: ${step}" >&2
			ok=1
		fi
		if rg -q "whuse-oscomp-step-end:${step}|whuse-oscomp-step-skip:${step}" "${text_log}"; then
			echo "[${arch}] step-done ok: ${step}"
		else
			echo "[${arch}] missing step-done: ${step}" >&2
			ok=1
		fi
	done

	echo "[${arch}] marker summary:"
	rg "whuse-oscomp-step-(begin|end|timeout|skip)|whuse-oscomp-suite-done" "${text_log}" || true
	local timeout_count
	local fail_count
	local error_count
	local bench_watchdog_count
	timeout_count="$(rg -c "whuse-oscomp-step-timeout:" "${text_log}" || true)"
	fail_count="$(rg -c "testcase .* fail" "${text_log}" || true)"
	error_count="$(rg -c "testcase .* error" "${text_log}" || true)"
	bench_watchdog_count="$(rg -c "whuse-oscomp-(lmbench|bench)-marker:watchdog-timeout:" "${text_log}" || true)"
	echo "[${arch}] quality summary: step-timeout=${timeout_count} testcase-fail=${fail_count} testcase-error=${error_count} bench-watchdog-timeout=${bench_watchdog_count}"
	return "${ok}"
}

cd "${REPO_ROOT}"

echo "building/checking workspace..."
make check
echo "preparing oscomp images..."
cargo xtask oscomp-images

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
