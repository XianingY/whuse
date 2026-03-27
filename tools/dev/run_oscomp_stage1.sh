#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
XTASK_CMD=(cargo run --manifest-path "${REPO_ROOT}/tools/xtask/Cargo.toml" --)

MODE="${1:-both}"
TIMEOUT_SECS="${TIMEOUT_SECS:-3600}"
TS="$(date +%Y%m%d-%H%M%S)"

export WHUSE_OSCOMP_DOCKER_IMAGE="${WHUSE_OSCOMP_DOCKER_IMAGE:-docker.educg.net/cg/os-contest:20260104}"
export WHUSE_OSCOMP_COMPAT="${WHUSE_OSCOMP_COMPAT:-0}"
export WHUSE_STAGE1_USE_IMAGE_COPY="${WHUSE_STAGE1_USE_IMAGE_COPY:-0}"
export WHUSE_STAGE1_CLEAN_LOONGARCH="${WHUSE_STAGE1_CLEAN_LOONGARCH:-1}"
export WHUSE_STAGE1_CLEAN_QEMU="${WHUSE_STAGE1_CLEAN_QEMU:-${WHUSE_STAGE1_CLEAN_LOONGARCH}}"
export WHUSE_OSCOMP_SKIP_BUILD="${WHUSE_OSCOMP_SKIP_BUILD:-0}"
export WHUSE_STAGE2_TIMEOUT_PROFILE="${WHUSE_STAGE2_TIMEOUT_PROFILE:-real}"
export WHUSE_STAGE2_REAL_PHASE="${WHUSE_STAGE2_REAL_PHASE:-full}"
export WHUSE_STAGE2_GATE_LIBCTEST_SCOPE="${WHUSE_STAGE2_GATE_LIBCTEST_SCOPE:-full}"
export WHUSE_STAGE2_FULL_MAX_GROUP="${WHUSE_STAGE2_FULL_MAX_GROUP:-all}"
export WHUSE_STAGE2_IOZONE_PROFILE="${WHUSE_STAGE2_IOZONE_PROFILE:-smoke}"
export WHUSE_STAGE2_IOZONE_FULL_SCOPE="${WHUSE_STAGE2_IOZONE_FULL_SCOPE:-full}"
export WHUSE_STAGE2_BASIC_PROFILE="${WHUSE_STAGE2_BASIC_PROFILE:-full}"
export WHUSE_STAGE2_BUSYBOX_PROFILE="${WHUSE_STAGE2_BUSYBOX_PROFILE:-full}"
export WHUSE_STAGE2_LIBCBENCH_SCOPE="${WHUSE_STAGE2_LIBCBENCH_SCOPE:-full}"
export WHUSE_STAGE2_LMBENCH_SCOPE="${WHUSE_STAGE2_LMBENCH_SCOPE:-full}"
export WHUSE_STAGE2_CHAIN_REAL_STEPS="${WHUSE_STAGE2_CHAIN_REAL_STEPS:-busybox_testcode.sh,iozone_testcode.sh,libctest_testcode.sh}"
export WHUSE_OSCOMP_PROFILE="${WHUSE_OSCOMP_PROFILE:-}"
export WHUSE_OSCOMP_CASE_FILTER="${WHUSE_OSCOMP_CASE_FILTER:-}"
export WHUSE_OSCOMP_RUNTIME_FILTER="${WHUSE_OSCOMP_RUNTIME_FILTER:-both}"

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
case "${WHUSE_STAGE2_FULL_MAX_GROUP}" in
time-test | basic | busybox | iozone | libctest | libc-bench | lmbench | lua | unixbench | netperf | iperf | ltp | cyclic | all) ;;
*)
	echo "WHUSE_STAGE2_FULL_MAX_GROUP must be one of: time-test,basic,busybox,iozone,libctest,libc-bench,lmbench,lua,unixbench,netperf,iperf,ltp,cyclic,all" >&2
	exit 1
	;;
esac
case "${WHUSE_STAGE2_IOZONE_PROFILE}" in
smoke | full) ;;
*)
	echo "WHUSE_STAGE2_IOZONE_PROFILE must be smoke or full" >&2
	exit 1
	;;
esac
case "${WHUSE_STAGE2_IOZONE_FULL_SCOPE}" in
probe | full) ;;
*)
	echo "WHUSE_STAGE2_IOZONE_FULL_SCOPE must be probe or full" >&2
	exit 1
	;;
esac
case "${WHUSE_STAGE2_BASIC_PROFILE}" in
full | smoke) ;;
*)
	echo "WHUSE_STAGE2_BASIC_PROFILE must be full or smoke" >&2
	exit 1
	;;
esac
case "${WHUSE_STAGE2_BUSYBOX_PROFILE}" in
full | smoke) ;;
*)
	echo "WHUSE_STAGE2_BUSYBOX_PROFILE must be full or smoke" >&2
	exit 1
	;;
esac
case "${WHUSE_STAGE2_LIBCBENCH_SCOPE}" in
full | smoke) ;;
*)
	echo "WHUSE_STAGE2_LIBCBENCH_SCOPE must be full or smoke" >&2
	exit 1
	;;
esac
case "${WHUSE_STAGE2_LMBENCH_SCOPE}" in
full | smoke) ;;
*)
	echo "WHUSE_STAGE2_LMBENCH_SCOPE must be full or smoke" >&2
	exit 1
	;;
esac
case "${WHUSE_OSCOMP_PROFILE}" in
"" | full | basic | busybox | iozone | libctest | libc-bench | lmbench | lua | ltp | unixbench | netperf | iperf | cyclic) ;;
*)
	echo "invalid WHUSE_OSCOMP_PROFILE=${WHUSE_OSCOMP_PROFILE}" >&2
	exit 1
	;;
esac
case "${WHUSE_OSCOMP_RUNTIME_FILTER}" in
both | musl | glibc) ;;
*)
	echo "invalid WHUSE_OSCOMP_RUNTIME_FILTER=${WHUSE_OSCOMP_RUNTIME_FILTER}" >&2
	exit 1
	;;
esac

trim_ascii_space() {
	local value="$1"
	value="${value#"${value%%[![:space:]]*}"}"
	value="${value%"${value##*[![:space:]]}"}"
	printf '%s' "${value}"
}

case_filter_group() {
	local filter="$1"
	if [[ "${filter}" != *:* ]]; then
		return 1
	fi
	printf '%s' "${filter%%:*}"
}

case_filter_payload() {
	local filter="$1"
	if [[ "${filter}" != *:* ]]; then
		return 1
	fi
	printf '%s' "${filter#*:}"
}

is_basic_case_name() {
	case "$1" in
	brk | chdir | clone | close | dup2 | dup | execve | exit | fork | fstat | getcwd | getdents | getpid | getppid | gettimeofday | mkdir_ | mmap | mount | munmap | openat | open | pipe | read | sleep | times | umount | uname | unlink | wait | waitpid | write | yield)
		return 0
		;;
	*)
		return 1
		;;
	esac
}

validate_oscomp_case_filter() {
	local filter="${1:-}"
	local group payload raw item
	if [[ -z "${filter}" ]]; then
		return 0
	fi
	group="$(case_filter_group "${filter}")" || {
		echo "invalid WHUSE_OSCOMP_CASE_FILTER=${filter} (expected group:cases)" >&2
		return 1
	}
	payload="$(case_filter_payload "${filter}")"
	if [[ -z "${payload}" ]]; then
		echo "invalid WHUSE_OSCOMP_CASE_FILTER=${filter} (missing cases)" >&2
		return 1
	fi
	case "${group}" in
	basic)
		IFS=',' read -r -a raw <<<"${payload}"
		for item in "${raw[@]}"; do
			item="$(trim_ascii_space "${item}")"
			if [[ -z "${item}" ]] || ! is_basic_case_name "${item}"; then
				echo "invalid WHUSE_OSCOMP_CASE_FILTER=${filter} (bad basic case: ${item})" >&2
				return 1
			fi
		done
		;;
	busybox)
		IFS=',' read -r -a raw <<<"${payload}"
		for item in "${raw[@]}"; do
			item="$(trim_ascii_space "${item}")"
			if [[ -z "${item}" ]]; then
				echo "invalid WHUSE_OSCOMP_CASE_FILTER=${filter} (empty busybox selector)" >&2
				return 1
			fi
		done
		;;
	*)
		echo "invalid WHUSE_OSCOMP_CASE_FILTER=${filter} (unsupported group: ${group})" >&2
		return 1
		;;
	esac
}

validate_oscomp_case_filter "${WHUSE_OSCOMP_CASE_FILTER}"

required_core_markers=(
	"whuse-oscomp-shell-entered"
	"whuse-oscomp-script-start"
	"whuse-oscomp-suite-done"
)
full_root_steps=(
	"time-test"
	"basic_testcode.sh"
	"busybox_testcode.sh"
	"iozone_testcode.sh"
	"libctest_testcode.sh"
	"libc-bench"
	"lmbench_testcode.sh"
	"lua_testcode.sh"
	"unixbench_testcode.sh"
	"netperf_testcode.sh"
	"iperf_testcode.sh"
	"ltp_testcode.sh"
	"cyclic_testcode.sh"
)
iozone_smoke_cases=(
	"smoke-write-read"
	"smoke-random-read"
	"smoke-fwrite-fread"
)
iozone_full_cases=(
	"auto-1k-4m"
	"throughput-write-read"
	"throughput-random-read"
	"throughput-read-backwards"
	"throughput-stride-read"
	"throughput-fwrite-fread"
	"throughput-pwrite-pread"
	"throughput-pwritev-preadv"
)
iozone_probe_cases=(
	"auto-1k-4m"
	"throughput-write-read"
)

profile_root_step() {
	local profile="$1"
	case "${profile}" in
	basic) echo "basic_testcode.sh" ;;
	busybox) echo "busybox_testcode.sh" ;;
	iozone) echo "iozone_testcode.sh" ;;
	libctest) echo "libctest_testcode.sh" ;;
	libc-bench) echo "libc-bench" ;;
	lmbench) echo "lmbench_testcode.sh" ;;
	lua) echo "lua_testcode.sh" ;;
	ltp) echo "ltp_testcode.sh" ;;
	unixbench) echo "unixbench_testcode.sh" ;;
	netperf) echo "netperf_testcode.sh" ;;
	iperf) echo "iperf_testcode.sh" ;;
	cyclic) echo "cyclic_testcode.sh" ;;
	*) return 1 ;;
	esac
}

resolve_expected_root_steps() {
	local profile="${1:-full}"
	if [[ -z "${profile}" || "${profile}" == "full" ]]; then
		local step
		for step in "${full_root_steps[@]}"; do
			printf '%s\n' "${step}"
			case "${step}:${WHUSE_STAGE2_FULL_MAX_GROUP}" in
			time-test:time-test | \
				basic_testcode.sh:basic | \
				busybox_testcode.sh:busybox | \
				iozone_testcode.sh:iozone | \
				libctest_testcode.sh:libctest | \
				libc-bench:libc-bench | \
				lmbench_testcode.sh:lmbench | \
				lua_testcode.sh:lua | \
				unixbench_testcode.sh:unixbench | \
				netperf_testcode.sh:netperf | \
				iperf_testcode.sh:iperf | \
				ltp_testcode.sh:ltp | \
				cyclic_testcode.sh:cyclic)
				return 0
				;;
			esac
		done
		return 0
	fi
	profile_root_step "${profile}"
}

effective_oscomp_profile() {
	if [[ -n "${WHUSE_OSCOMP_PROFILE}" ]]; then
		printf '%s\n' "${WHUSE_OSCOMP_PROFILE}"
		return 0
	fi
	local repo_profile_file="${REPO_ROOT}/tools/oscomp/profile/default.txt"
	if [[ -f "${repo_profile_file}" ]]; then
		tr -d '[:space:]' < "${repo_profile_file}"
		printf '\n'
		return 0
	fi
	printf 'full\n'
}

case_filter_matches_profile() {
	local profile="$1"
	local filter_group
	if [[ -z "${WHUSE_OSCOMP_CASE_FILTER}" ]]; then
		return 1
	fi
	filter_group="$(case_filter_group "${WHUSE_OSCOMP_CASE_FILTER}")" || return 1
	[[ "${filter_group}" == "${profile}" ]]
}

runtime_filter_selects_runtime() {
	local runtime="$1"
	local runtime_filter="${2:-${WHUSE_OSCOMP_RUNTIME_FILTER}}"
	case "${runtime_filter}" in
	both) return 0 ;;
	"${runtime}") return 0 ;;
	*) return 1 ;;
	esac
}

count_runtime_case_markers() {
	local log_file="$1"
	local marker_prefix="$2"
	local runtime="$3"
	rg -c "^${marker_prefix}:${runtime}:" "${log_file}" 2>/dev/null || echo 0
}

count_step_semantic_lines() {
	local log_file="$1"
	local step="$2"
	local pattern="$3"
	awk \
		-v begin="whuse-oscomp-step-begin:${step}" \
		-v end="whuse-oscomp-step-end:${step}:" \
		-v pat="${pattern}" '
			index($0, begin) > 0 { in_step = 1; next }
			in_step && index($0, end) > 0 { in_step = 0; next }
			in_step && $0 ~ pat { count++ }
			END { print count + 0 }
		' "${log_file}" 2>/dev/null || echo 0
}

runtime_images=()
cleanup_target_images=()
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
	if [[ "${WHUSE_STAGE1_CLEAN_QEMU}" != "1" ]]; then
		return
	fi
	local cleanup_script="${REPO_ROOT}/tools/dev/cleanup_stale_qemu.sh"
	if [[ ! -f "${cleanup_script}" ]]; then
		return
	fi
	local -a cleanup_args=()
	local image
	for image in "${cleanup_target_images[@]}"; do
		cleanup_args+=(--image "${image}")
	done
	if [[ ${#cleanup_args[@]} -eq 0 ]]; then
		cleanup_args+=(--all-oscomp-containers)
	fi
	echo "[cleanup] scanning stale loongarch qemu instances"
	bash "${cleanup_script}" --dry-run "${cleanup_args[@]}" || true
	bash "${cleanup_script}" "${cleanup_args[@]}" || true
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
	if [[ "${WHUSE_STAGE1_USE_IMAGE_COPY}" != "1" && -z "${WHUSE_OSCOMP_CASE_FILTER}" && "${WHUSE_OSCOMP_RUNTIME_FILTER}" == "both" ]]; then
		cleanup_target_images+=("${src}")
		echo "${src}"
		return
	fi
	local dst="/tmp/whuse-${arch}-stage1-${TS}.img"
	cp --reflink=auto "${src}" "${dst}"
	runtime_images+=("${dst}")
	cleanup_target_images+=("${dst}")
	echo "${dst}"
}

write_runtime_image_config() {
	local image="$1"
	local target="$2"
	local value="$3"
	local tmp
	tmp="$(mktemp)"
	printf '%s\n' "${value}" >"${tmp}"
	debugfs -w -R "rm ${target}" "${image}" >/dev/null 2>&1 || true
	if ! debugfs -w -R "write ${tmp} ${target}" "${image}" >/dev/null 2>&1; then
		rm -f "${tmp}"
		echo "failed to write runtime config ${target} into ${image}" >&2
		return 1
	fi
	rm -f "${tmp}"
}

write_runtime_image_file() {
	local image="$1"
	local target="$2"
	local src="$3"
	debugfs -w -R "rm ${target}" "${image}" >/dev/null 2>&1 || true
	if ! debugfs -w -R "write ${src} ${target}" "${image}" >/dev/null 2>&1; then
		echo "failed to write runtime file ${target} from ${src} into ${image}" >&2
		return 1
	fi
}

write_runtime_image_text_file() {
	local image="$1"
	local target="$2"
	local content="$3"
	local tmp
	tmp="$(mktemp)"
	printf '%s' "${content}" >"${tmp}"
	write_runtime_image_file "${image}" "${target}" "${tmp}"
	rm -f "${tmp}"
}

inject_oscomp_profile() {
	local image="$1"
	local profile="$2"
	write_runtime_image_config "${image}" "/whuse-oscomp-profile" "${profile}"
}

inject_oscomp_runtime_filter() {
	local image="$1"
	local runtime_filter="$2"
	write_runtime_image_config "${image}" "/whuse-oscomp-runtime-filter" "${runtime_filter}"
}

resolve_busybox_case_lines_for_image() {
	local image="$1"
	local runtime="$2"
	local filter="$3"
	local payload original token trimmed selected line match_count
	local -a raw tokens lines
	payload="$(case_filter_payload "${filter}")"
	original="$(debugfs -R "cat /${runtime}/busybox_cmd.txt" "${image}" 2>/dev/null)" || {
		echo "failed to read /${runtime}/busybox_cmd.txt from ${image}" >&2
		return 1
	}
	while IFS= read -r line; do
		[[ -n "${line}" ]] || continue
		lines+=("${line}")
	done <<<"${original}"
	IFS=',' read -r -a raw <<<"${payload}"
	for token in "${raw[@]}"; do
		trimmed="$(trim_ascii_space "${token}")"
		trimmed="${trimmed#busybox }"
		selected=""
		match_count=0
		for line in "${lines[@]}"; do
			if [[ "${line}" == "${trimmed}" ]]; then
				selected="${line}"
				match_count=1
				break
			fi
			if [[ "${line#"$trimmed"}" != "${line}" ]]; then
				selected="${line}"
				match_count=$((match_count + 1))
			fi
		done
		if [[ "${match_count}" -eq 0 ]]; then
			echo "failed to resolve busybox case selector '${trimmed}' for ${runtime}" >&2
			return 1
		fi
		if [[ "${match_count}" -gt 1 ]]; then
			echo "ambiguous busybox case selector '${trimmed}' for ${runtime}" >&2
			return 1
		fi
		tokens+=("${selected}")
	done
	printf '%s\n' "${tokens[@]}"
}

shell_single_quote() {
	local value="$1"
	value="${value//\'/\'\"\'\"\'}"
	printf "'%s'" "${value}"
}

build_filtered_basic_run_all() {
	local runtime="$1"
	local filter="$2"
	local payload case_name
	local -a raw cases
	payload="$(case_filter_payload "${filter}")"
	IFS=',' read -r -a raw <<<"${payload}"
	for case_name in "${raw[@]}"; do
		case_name="$(trim_ascii_space "${case_name}")"
		cases+=("${case_name}")
	done
	cat <<EOF
#!/bin/sh
fail=0
tests="
$(printf '%s\n' "${cases[@]}")
"
for i in \$tests
do
	echo "Testing \$i :"
	./\$i
	rc=\$?
	echo "whuse-oscomp-basic-case:${runtime}:\$i:\$rc"
	if [ "\$fail" = "0" ] && [ "\$rc" != "0" ]; then
		fail="\$rc"
	fi
done
exit "\$fail"
EOF
}

build_filtered_basic_testcode() {
	local runtime="$1"
	cat <<EOF
#!/bin/sh
cd "/${runtime}/basic" || exit 1
exec /musl/busybox sh ./run-all.sh
EOF
}

build_filtered_busybox_testcode() {
	local runtime="$1"
	local lines_text="$2"
	local line
cat <<EOF
#!/bin/sh
set +v +x
fail=0
run_case() {
	line="\$1"
	eval "./busybox \$line"
	rc=\$?
	printf '\nwhuse-oscomp-busybox-case:${runtime}:%s:%s\n' "\$line" "\$rc"
	if [ "\$rc" -ne 0 ] && [ "\$line" != "false" ]; then
		printf 'testcase busybox %s fail\n' "\$line"
		if [ "\$fail" = "0" ]; then
			fail="\$rc"
		fi
	else
		printf 'testcase busybox %s success\n' "\$line"
	fi
}
EOF
	while IFS= read -r line; do
		[[ -n "${line}" ]] || continue
		printf 'run_case %s\n' "$(shell_single_quote "${line}")"
	done <<<"${lines_text}"
	cat <<'EOF'
exit "$fail"
EOF
}

inject_case_filter_files() {
	local image="$1"
	local effective_profile="$2"
	local filter_group basic_script busybox_lines
	if [[ -z "${WHUSE_OSCOMP_CASE_FILTER}" ]]; then
		return 0
	fi
	if [[ "${WHUSE_STAGE1_USE_IMAGE_COPY}" != "1" ]]; then
		echo "WHUSE_OSCOMP_CASE_FILTER requires temp image injection; enable image copy or let the runner create a copy" >&2
	fi
	filter_group="$(case_filter_group "${WHUSE_OSCOMP_CASE_FILTER}")" || return 1
	if [[ "${filter_group}" != "${effective_profile}" ]]; then
		echo "WHUSE_OSCOMP_CASE_FILTER=${WHUSE_OSCOMP_CASE_FILTER} requires WHUSE_OSCOMP_PROFILE=${filter_group}" >&2
		return 1
	fi
	write_runtime_image_config "${image}" "/whuse-oscomp-case-filter" "${WHUSE_OSCOMP_CASE_FILTER}"
	case "${filter_group}" in
	basic)
		basic_script="$(build_filtered_basic_run_all "musl" "${WHUSE_OSCOMP_CASE_FILTER}")"
		write_runtime_image_text_file "${image}" "/musl/basic/run-all.sh" "${basic_script}"
		basic_script="$(build_filtered_basic_testcode "musl")"
		write_runtime_image_text_file "${image}" "/musl/basic_testcode.sh" "${basic_script}"
		basic_script="$(build_filtered_basic_run_all "glibc" "${WHUSE_OSCOMP_CASE_FILTER}")"
		write_runtime_image_text_file "${image}" "/glibc/basic/run-all.sh" "${basic_script}"
		basic_script="$(build_filtered_basic_testcode "glibc")"
		write_runtime_image_text_file "${image}" "/glibc/basic_testcode.sh" "${basic_script}"
		;;
	busybox)
		busybox_lines="$(resolve_busybox_case_lines_for_image "${image}" "musl" "${WHUSE_OSCOMP_CASE_FILTER}")"
		write_runtime_image_text_file "${image}" "/musl/busybox_cmd.txt" "${busybox_lines}"$'\n'
		write_runtime_image_text_file "${image}" "/musl/busybox_testcode.sh" "$(build_filtered_busybox_testcode "musl" "${busybox_lines}")"
		busybox_lines="$(resolve_busybox_case_lines_for_image "${image}" "glibc" "${WHUSE_OSCOMP_CASE_FILTER}")"
		write_runtime_image_text_file "${image}" "/glibc/busybox_cmd.txt" "${busybox_lines}"$'\n'
		write_runtime_image_text_file "${image}" "/glibc/busybox_testcode.sh" "$(build_filtered_busybox_testcode "glibc" "${busybox_lines}")"
		;;
	esac
}

inject_stage2_local_env() {
	local image="$1"
	local runtime_filter="${2:-both}"
	local local_env
	printf -v local_env '%s\n%s\n%s\n' \
		"WHUSE_STAGE2_FULL_MAX_GROUP=${WHUSE_STAGE2_FULL_MAX_GROUP}" \
		"WHUSE_STAGE2_IOZONE_PROFILE=${WHUSE_STAGE2_IOZONE_PROFILE}" \
		"WHUSE_STAGE2_BASIC_PROFILE=${WHUSE_STAGE2_BASIC_PROFILE}"
	local_env+="WHUSE_STAGE2_BUSYBOX_PROFILE=${WHUSE_STAGE2_BUSYBOX_PROFILE}"$'\n'
	local_env+="WHUSE_STAGE2_GATE_LIBCTEST_SCOPE=${WHUSE_STAGE2_GATE_LIBCTEST_SCOPE}"$'\n'
	local_env+="WHUSE_STAGE2_LIBCBENCH_SCOPE=${WHUSE_STAGE2_LIBCBENCH_SCOPE}"$'\n'
	local_env+="WHUSE_STAGE2_LMBENCH_SCOPE=${WHUSE_STAGE2_LMBENCH_SCOPE}"$'\n'
	local_env+="WHUSE_OSCOMP_RUNTIME_FILTER=${runtime_filter}"$'\n'
	write_runtime_image_config "${image}" "/musl/.whuse_stage2_local.env" "${local_env}"
}

inject_stage2_profile_files() {
	local arch="$1"
	local image="$2"
	local runtime_filter="${3:-${WHUSE_OSCOMP_RUNTIME_FILTER}}"
	local effective_profile
	effective_profile="$(effective_oscomp_profile)"
	case "${effective_profile}" in
	"" | full | basic | busybox | iozone | libctest | libc-bench | lmbench | lua | ltp | unixbench | netperf | iperf | cyclic) ;;
	*)
		echo "invalid effective oscomp profile ${effective_profile}" >&2
		return 1
		;;
	esac
	if [[ -n "${WHUSE_OSCOMP_PROFILE}" ]]; then
		inject_oscomp_profile "${image}" "${WHUSE_OSCOMP_PROFILE}"
	fi
	inject_stage2_local_env "${image}" "${runtime_filter}"
	if [[ "${runtime_filter}" != "both" ]]; then
		inject_oscomp_runtime_filter "${image}" "${runtime_filter}"
	fi
	inject_case_filter_files "${image}" "${effective_profile}"
	echo "[${arch}] stage2 timeout profile: ${WHUSE_STAGE2_TIMEOUT_PROFILE}, real-phase: ${WHUSE_STAGE2_REAL_PHASE}, gate-libctest-scope: ${WHUSE_STAGE2_GATE_LIBCTEST_SCOPE}, libcbench-scope: ${WHUSE_STAGE2_LIBCBENCH_SCOPE}, lmbench-scope: ${WHUSE_STAGE2_LMBENCH_SCOPE}, full-max-group: ${WHUSE_STAGE2_FULL_MAX_GROUP}, basic-profile: ${WHUSE_STAGE2_BASIC_PROFILE}, busybox-profile: ${WHUSE_STAGE2_BUSYBOX_PROFILE}, iozone-profile: ${WHUSE_STAGE2_IOZONE_PROFILE}, iozone-full-scope: ${WHUSE_STAGE2_IOZONE_FULL_SCOPE} (chain-real-steps=${WHUSE_STAGE2_CHAIN_REAL_STEPS}, oscomp-profile=${effective_profile}, case-filter=${WHUSE_OSCOMP_CASE_FILTER:-none}, runtime-filter=${runtime_filter})"
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
	setsid env WHUSE_DISK_IMAGE="${runtime_image}" "${XTASK_CMD[@]}" "${xtask_cmd}" >"${log}" 2>&1 &
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
	local effective_profile
	local effective_runtime_filter="${WHUSE_OSCOMP_RUNTIME_FILTER}"
	if [[ "${arch}" == "la" && "${effective_runtime_filter}" != "both" ]]; then
		echo "[${arch}] runtime-filter=${effective_runtime_filter} is not supported on loongarch yet; falling back to both"
		effective_runtime_filter="both"
	fi
	runtime_image="$(prepare_runtime_image "${arch}" "${image}")"
	inject_stage2_profile_files "${arch}" "${runtime_image}" "${effective_runtime_filter}"
	effective_profile="$(effective_oscomp_profile)"
	if [[ "${arch}" == "la" ]]; then
		cleanup_loongarch_stale_qemu
	fi

	echo "[${arch}] running ${xtask_cmd}, timeout=${TIMEOUT_SECS}s, image=${runtime_image}, oscomp-profile=${effective_profile}, runtime-filter=${effective_runtime_filter}"
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

	local -a expected_steps=()
	mapfile -t expected_steps < <(resolve_expected_root_steps "${effective_profile}")
	for step in "${expected_steps[@]}"; do
		if rg -q -F "whuse-oscomp-step-begin:${step}" "${text_log}"; then
			echo "[${arch}] step-begin ok: ${step}"
		else
			echo "[${arch}] missing step-begin: ${step}" >&2
			ok=1
		fi
		if rg -q -F "whuse-oscomp-step-end:${step}" "${text_log}" || \
			rg -q -F "whuse-oscomp-step-skip:${step}" "${text_log}" || \
			rg -q -F "whuse-oscomp-step-timeout:${step}" "${text_log}"; then
			echo "[${arch}] step-done ok: ${step}"
		else
			echo "[${arch}] missing step-done: ${step}" >&2
			ok=1
		fi
	done
	if [[ "${effective_profile}" == "basic" ]]; then
		if case_filter_matches_profile "basic"; then
			local musl_case_markers glibc_case_markers
			musl_case_markers="$(count_runtime_case_markers "${text_log}" "whuse-oscomp-basic-case" "musl")"
			glibc_case_markers="$(count_runtime_case_markers "${text_log}" "whuse-oscomp-basic-case" "glibc")"
			local basic_marker_fail=0
			if runtime_filter_selects_runtime "musl" "${effective_runtime_filter}" && [[ "${musl_case_markers}" -lt 1 ]]; then
				basic_marker_fail=1
			fi
			if runtime_filter_selects_runtime "glibc" "${effective_runtime_filter}" && [[ "${glibc_case_markers}" -lt 1 ]]; then
				basic_marker_fail=1
			fi
			if [[ "${basic_marker_fail}" -ne 0 ]]; then
				echo "[${arch}] basic profile failed semantic check: expected whuse-oscomp-basic-case markers in selected runtimes (filter=${effective_runtime_filter}, musl=${musl_case_markers}, glibc=${glibc_case_markers})" >&2
				ok=1
			else
				echo "[${arch}] basic profile semantic check ok: runtime-filter=${effective_runtime_filter}, musl basic-case markers=${musl_case_markers}, glibc basic-case markers=${glibc_case_markers}"
			fi
		else
			local musl_brk_count glibc_brk_count
			musl_brk_count="$(count_step_semantic_lines "${text_log}" "musl/basic_testcode.sh" '^Testing brk :')"
			glibc_brk_count="$(count_step_semantic_lines "${text_log}" "glibc/basic_testcode.sh" '^Testing brk :')"
			local basic_brk_fail=0
			if runtime_filter_selects_runtime "musl" "${effective_runtime_filter}" && [[ "${musl_brk_count}" -lt 1 ]]; then
				basic_brk_fail=1
			fi
			if runtime_filter_selects_runtime "glibc" "${effective_runtime_filter}" && [[ "${glibc_brk_count}" -lt 1 ]]; then
				basic_brk_fail=1
			fi
			if [[ "${basic_brk_fail}" -ne 0 ]]; then
				echo "[${arch}] basic profile failed semantic check: expected Testing brk output in selected runtimes (filter=${effective_runtime_filter}, musl=${musl_brk_count}, glibc=${glibc_brk_count})" >&2
				ok=1
			else
				echo "[${arch}] basic profile semantic check ok: runtime-filter=${effective_runtime_filter}, musl Testing brk=${musl_brk_count}, glibc Testing brk=${glibc_brk_count}"
			fi
		fi
	fi
	if [[ "${effective_profile}" == "busybox" ]]; then
		if case_filter_matches_profile "busybox"; then
			local musl_busybox_markers glibc_busybox_markers
			musl_busybox_markers="$(count_runtime_case_markers "${text_log}" "whuse-oscomp-busybox-case" "musl")"
			glibc_busybox_markers="$(count_runtime_case_markers "${text_log}" "whuse-oscomp-busybox-case" "glibc")"
			local busybox_marker_fail=0
			if runtime_filter_selects_runtime "musl" "${effective_runtime_filter}" && [[ "${musl_busybox_markers}" -lt 1 ]]; then
				busybox_marker_fail=1
			fi
			if runtime_filter_selects_runtime "glibc" "${effective_runtime_filter}" && [[ "${glibc_busybox_markers}" -lt 1 ]]; then
				busybox_marker_fail=1
			fi
			if [[ "${busybox_marker_fail}" -ne 0 ]]; then
				echo "[${arch}] busybox profile failed semantic check: expected whuse-oscomp-busybox-case markers in selected runtimes (filter=${effective_runtime_filter}, musl=${musl_busybox_markers}, glibc=${glibc_busybox_markers})" >&2
				ok=1
			else
				echo "[${arch}] busybox profile semantic check ok: runtime-filter=${effective_runtime_filter}, musl busybox-case markers=${musl_busybox_markers}, glibc busybox-case markers=${glibc_busybox_markers}"
			fi
		else
			local musl_busybox_cases glibc_busybox_cases
			musl_busybox_cases="$(count_step_semantic_lines "${text_log}" "musl/busybox_testcode.sh" 'testcase busybox .* success')"
			glibc_busybox_cases="$(count_step_semantic_lines "${text_log}" "glibc/busybox_testcode.sh" 'testcase busybox .* success')"
			local busybox_case_fail=0
			if runtime_filter_selects_runtime "musl" "${effective_runtime_filter}" && [[ "${musl_busybox_cases}" -lt 1 ]]; then
				busybox_case_fail=1
			fi
			if runtime_filter_selects_runtime "glibc" "${effective_runtime_filter}" && [[ "${glibc_busybox_cases}" -lt 1 ]]; then
				busybox_case_fail=1
			fi
			if [[ "${busybox_case_fail}" -ne 0 ]]; then
				echo "[${arch}] busybox profile failed semantic check: expected testcase busybox output in selected runtimes (filter=${effective_runtime_filter}, musl=${musl_busybox_cases}, glibc=${glibc_busybox_cases})" >&2
				ok=1
			else
				echo "[${arch}] busybox profile semantic check ok: runtime-filter=${effective_runtime_filter}, musl testcase busybox=${musl_busybox_cases}, glibc testcase busybox=${glibc_busybox_cases}"
			fi
		fi
	fi

	echo "[${arch}] marker summary:"
	rg "whuse-oscomp-step-(begin|end|timeout|skip)|whuse-oscomp-(basic|busybox)-case|whuse-oscomp-suite-done" "${text_log}" || true
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
	"${XTASK_CMD[@]}" oscomp-images
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
