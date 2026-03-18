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
export WHUSE_STAGE1_CLEAN_LOONGARCH="${WHUSE_STAGE1_CLEAN_LOONGARCH:-1}"
export WHUSE_OSCOMP_SKIP_BUILD="${WHUSE_OSCOMP_SKIP_BUILD:-0}"
export WHUSE_STAGE2_TIMEOUT_PROFILE="${WHUSE_STAGE2_TIMEOUT_PROFILE:-real}"
export WHUSE_STAGE2_REAL_PHASE="${WHUSE_STAGE2_REAL_PHASE:-full}"
export WHUSE_STAGE2_GATE_LIBCTEST_SCOPE="${WHUSE_STAGE2_GATE_LIBCTEST_SCOPE:-full}"
export WHUSE_STAGE2_CHAIN_REAL_STEPS="${WHUSE_STAGE2_CHAIN_REAL_STEPS:-busybox_testcode.sh,iozone_testcode.sh,libctest_testcode.sh}"

RV_IMG="${REPO_ROOT}/target/oscomp/sdcard-rv.img"
LA_IMG="${REPO_ROOT}/target/oscomp/sdcard-la.img"

if [[ "${WHUSE_OSCOMP_COMPAT}" != "0" ]]; then
	echo "WHUSE_OSCOMP_COMPAT must be 0 for stage1 real-execution runs" >&2
	exit 1
fi
case "${WHUSE_STAGE2_TIMEOUT_PROFILE}" in
real | chain-fast) ;;
*)
	echo "WHUSE_STAGE2_TIMEOUT_PROFILE must be real or chain-fast" >&2
	exit 1
	;;
esac
case "${WHUSE_STAGE2_REAL_PHASE}" in
gate | full) ;;
*)
	echo "WHUSE_STAGE2_REAL_PHASE must be gate or full" >&2
	exit 1
	;;
esac
case "${WHUSE_STAGE2_GATE_LIBCTEST_SCOPE}" in
smoke | full) ;;
*)
	echo "WHUSE_STAGE2_GATE_LIBCTEST_SCOPE must be smoke or full" >&2
	exit 1
	;;
esac

required_core_markers=(
	"whuse-oscomp-shell-entered"
	"whuse-oscomp-script-start"
	"whuse-oscomp-suite-done"
)

required_step_groups=(
	"time-test:time-test"
	"busybox:busybox_testcode.sh"
	"iozone:iozone_testcode.sh"
	"libctest:libctest_testcode.sh"
	"libc-bench:libc-bench"
	"lmbench:lmbench_testcode.sh"
	"lua:lua_testcode.sh"
	"unixbench:unixbench_testcode.sh"
	"netperf:netperf_testcode.sh"
	"iperf:iperf_testcode.sh"
	"cyclic:cyclic_testcode.sh,cyclictest_testcode.sh"
)
required_step_groups_gate=(
	"time-test:time-test"
	"busybox:busybox_testcode.sh"
	"iozone:iozone_testcode.sh"
	"libctest:libctest_testcode.sh"
)

runtime_images=()
active_run_pid=""
active_run_pgid=""
active_run_label=""
cleanup_done=0

cleanup_runtime_images() {
	for img in "${runtime_images[@]}"; do
		rm -f "${img}"
	done
}

cleanup_loongarch_stale_qemu() {
	if [[ "${WHUSE_STAGE1_CLEAN_LOONGARCH}" != "1" ]]; then
		return
	fi
	local cleanup_script="${REPO_ROOT}/tools/dev/cleanup_stale_qemu.sh"
	if [[ ! -f "${cleanup_script}" ]]; then
		return
	fi
	echo "[cleanup] scanning stale loongarch qemu instances"
	bash "${cleanup_script}" --dry-run || true
	bash "${cleanup_script}" || true
}

terminate_process_group() {
	local pgid="$1"
	local sig="$2"
	if [[ -z "${pgid}" ]]; then
		return
	fi
	if pgrep -g "${pgid}" >/dev/null 2>&1 || ps -p "${pgid}" >/dev/null 2>&1; then
		kill "-${sig}" -- "-${pgid}" 2>/dev/null || true
	fi
}

cleanup_active_run() {
	if [[ -z "${active_run_pgid}" ]]; then
		return
	fi
	echo "[cleanup] stopping process group ${active_run_pgid} (${active_run_label})"
	terminate_process_group "${active_run_pgid}" "TERM"
	sleep 2
	if pgrep -g "${active_run_pgid}" >/dev/null 2>&1; then
		echo "[cleanup] process group ${active_run_pgid} still alive, forcing KILL"
		terminate_process_group "${active_run_pgid}" "KILL"
	fi
	active_run_pid=""
	active_run_pgid=""
	active_run_label=""
}

cleanup_all() {
	local rc=$?
	if [[ "${cleanup_done}" -eq 1 ]]; then
		return "${rc}"
	fi
	cleanup_done=1
	cleanup_active_run
	cleanup_loongarch_stale_qemu
	cleanup_runtime_images
	return "${rc}"
}

on_interrupt() {
	cleanup_all
	exit 130
}

on_term() {
	cleanup_all
	exit 143
}

trap cleanup_all EXIT
trap on_interrupt INT
trap on_term TERM

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

inject_stage2_profile_files() {
	local arch="$1"
	if [[ "${arch}" != "la" ]]; then
		return
	fi
	echo "[${arch}] stage2 timeout profile: ${WHUSE_STAGE2_TIMEOUT_PROFILE}, real-phase: ${WHUSE_STAGE2_REAL_PHASE}, gate-libctest-scope: ${WHUSE_STAGE2_GATE_LIBCTEST_SCOPE} (chain-real-steps=${WHUSE_STAGE2_CHAIN_REAL_STEPS})"
}

run_xtask_with_timeout() {
	local runtime_image="$1"
	local xtask_cmd="$2"
	local log="$3"
	local label="$4"
	local start_ts
	local deadline
	local now_ts
	local timed_out=0
	local suite_done_detected=0

	start_ts="$(date +%s)"
	deadline=$((start_ts + TIMEOUT_SECS))
	setsid env WHUSE_DISK_IMAGE="${runtime_image}" cargo xtask "${xtask_cmd}" >"${log}" 2>&1 &
	local runner_pid=$!
	local runner_pgid="${runner_pid}"
	active_run_pid="${runner_pid}"
	active_run_pgid="${runner_pgid}"
	active_run_label="${label}"

	while kill -0 "${runner_pid}" >/dev/null 2>&1; do
		if [[ -f "${log}" ]] && rg -a -q "whuse-oscomp-suite-done" "${log}"; then
			suite_done_detected=1
			break
		fi
		now_ts="$(date +%s)"
		if ((now_ts >= deadline)); then
			timed_out=1
			break
		fi
		sleep 1
	done

	if [[ "${suite_done_detected}" -eq 1 ]]; then
		echo "[${label}] suite-done detected, terminating process group ${runner_pgid}"
		terminate_process_group "${runner_pgid}" "TERM"
		local suite_grace_deadline=$((SECONDS + 5))
		while pgrep -g "${runner_pgid}" >/dev/null 2>&1; do
			if ((SECONDS >= suite_grace_deadline)); then
				break
			fi
			sleep 1
		done
		if pgrep -g "${runner_pgid}" >/dev/null 2>&1; then
			echo "[${label}] suite-done cleanup forcing KILL on process group ${runner_pgid}"
			terminate_process_group "${runner_pgid}" "KILL"
		fi
	fi

	if [[ "${timed_out}" -eq 1 ]]; then
		echo "[${label}] timeout reached (${TIMEOUT_SECS}s), terminating process group ${runner_pgid}"
		terminate_process_group "${runner_pgid}" "TERM"
		local grace_deadline=$((SECONDS + 5))
		while pgrep -g "${runner_pgid}" >/dev/null 2>&1; do
			if ((SECONDS >= grace_deadline)); then
				break
			fi
			sleep 1
		done
		if pgrep -g "${runner_pgid}" >/dev/null 2>&1; then
			echo "[${label}] process group ${runner_pgid} still alive, forcing KILL"
			terminate_process_group "${runner_pgid}" "KILL"
		fi
	fi

	local run_rc=0
	wait "${runner_pid}" || run_rc=$?
	if pgrep -g "${runner_pgid}" >/dev/null 2>&1; then
		echo "[${label}] detected residual processes in group ${runner_pgid}, forcing cleanup"
		terminate_process_group "${runner_pgid}" "TERM"
		sleep 2
		if pgrep -g "${runner_pgid}" >/dev/null 2>&1; then
			terminate_process_group "${runner_pgid}" "KILL"
		fi
		timed_out=1
	fi
	active_run_pid=""
	active_run_pgid=""
	active_run_label=""

	if [[ "${suite_done_detected}" -eq 1 ]]; then
		return 0
	fi
	if [[ "${timed_out}" -eq 1 ]]; then
		return 124
	fi
	return "${run_rc}"
}

run_arch() {
	local arch="$1"
	local image="$2"
	local xtask_cmd="$3"
	local log="/tmp/${arch}-stage1-${TS}.log"
	local text_log="/tmp/${arch}-stage1-${TS}.strings.log"
	local runtime_image
	runtime_image="$(prepare_runtime_image "${arch}" "${image}")"
	inject_stage2_profile_files "${arch}"
	if [[ "${arch}" == "la" ]]; then
		cleanup_loongarch_stale_qemu
	fi

	echo "[${arch}] running ${xtask_cmd}, timeout=${TIMEOUT_SECS}s, image=${runtime_image}"
	set +e
	run_xtask_with_timeout "${runtime_image}" "${xtask_cmd}" "${log}" "${arch}"
	local run_rc=$?
	set -e
	echo "[${arch}] xtask exit code: ${run_rc}"
	strings "${log}" >"${text_log}" || true
	echo "[${arch}] log: ${log}"

	if rg -q "KERNEL PANIC|panic|pid 1 \(init\).*trap" "${text_log}"; then
		echo "[${arch}] detected kernel panic or init crash" >&2
		rg "KERNEL PANIC|panic|pid 1 \(init\).*trap" "${text_log}" >&2 || true
		return 1
	fi

	local ok=0
	for marker in "${required_core_markers[@]}"; do
		if rg -q -F "${marker}" "${text_log}"; then
			echo "[${arch}] core-marker ok: ${marker}"
		else
			echo "[${arch}] missing core-marker: ${marker}" >&2
			ok=1
		fi
	done

	local -a step_groups_to_check=("${required_step_groups[@]}")
	if [[ "${arch}" == "la" && "${WHUSE_STAGE2_TIMEOUT_PROFILE}" == "real" && "${WHUSE_STAGE2_REAL_PHASE}" == "gate" ]]; then
		step_groups_to_check=("${required_step_groups_gate[@]}")
	fi
	for step_group in "${step_groups_to_check[@]}"; do
		local label="${step_group%%:*}"
		local names_csv="${step_group#*:}"
		local found_begin=0
		local found_done=0
		IFS=',' read -r -a names <<<"${names_csv}"
		for name in "${names[@]}"; do
			if rg -q -F "whuse-oscomp-step-begin:${name}" "${text_log}"; then
				found_begin=1
			fi
			if rg -q -F "whuse-oscomp-step-end:${name}" "${text_log}" || \
				rg -q -F "whuse-oscomp-step-skip:${name}" "${text_log}" || \
				rg -q -F "whuse-oscomp-step-timeout:${name}" "${text_log}"; then
				found_done=1
			fi
		done
		if [[ "${found_begin}" -eq 1 ]]; then
			echo "[${arch}] step-begin ok: ${label}"
		else
			echo "[${arch}] missing step-begin: ${label} (${names_csv})" >&2
			ok=1
		fi
		if [[ "${found_done}" -eq 1 ]]; then
			echo "[${arch}] step-done ok: ${label}"
		else
			echo "[${arch}] missing step-done: ${label} (${names_csv})" >&2
			ok=1
		fi
	done
	if [[ "${arch}" == "la" && "${WHUSE_STAGE2_TIMEOUT_PROFILE}" == "real" && "${WHUSE_STAGE2_REAL_PHASE}" == "gate" ]]; then
		if rg -q -F "whuse-oscomp-step-end:libctest_testcode.sh:0" "${text_log}"; then
			echo "[${arch}] gate-check ok: libctest step-end:0"
		else
			echo "[${arch}] gate-check failed: missing whuse-oscomp-step-end:libctest_testcode.sh:0" >&2
			ok=1
		fi
		if rg -q -F "whuse-oscomp-real-phase:gate" "${text_log}"; then
			echo "[${arch}] gate-check ok: real-phase marker"
		else
			echo "[${arch}] gate-check failed: missing whuse-oscomp-real-phase:gate marker" >&2
			ok=1
		fi
	fi

	echo "[${arch}] marker summary:"
	rg "whuse-oscomp-step-(begin|end|timeout|skip)|whuse-oscomp-suite-done" "${text_log}" || true
	local timeout_count
	local fail_count
	local error_count
	local bench_watchdog_count
	timeout_count="$(rg -c "whuse-oscomp-step-timeout:" "${text_log}" || echo 0)"
	fail_count="$(rg -c "testcase .* fail" "${text_log}" || echo 0)"
	error_count="$(rg -c "testcase .* error" "${text_log}" || echo 0)"
	bench_watchdog_count="$(rg -c "whuse-oscomp-(lmbench|bench)-marker:watchdog-timeout:" "${text_log}" || echo 0)"
	echo "[${arch}] quality summary: step-timeout=${timeout_count} testcase-fail=${fail_count} testcase-error=${error_count} bench-watchdog-timeout=${bench_watchdog_count}"
	if [[ "${arch}" == "la" ]]; then
		cleanup_loongarch_stale_qemu
	fi
	return "${ok}"
}

cd "${REPO_ROOT}"

if [[ "${WHUSE_OSCOMP_SKIP_BUILD}" == "1" ]]; then
	echo "skipping workspace check/image preparation (WHUSE_OSCOMP_SKIP_BUILD=1)"
else
	echo "building/checking workspace..."
	make check
	echo "preparing oscomp images..."
	cargo xtask oscomp-images
fi

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
