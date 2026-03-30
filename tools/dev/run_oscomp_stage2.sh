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
export WHUSE_QEMU_MODE="${WHUSE_QEMU_MODE:-contest}"
export WHUSE_STAGE2_USE_IMAGE_COPY="${WHUSE_STAGE2_USE_IMAGE_COPY:-0}"
export WHUSE_STAGE2_STOP_ON_SUITE_DONE="${WHUSE_STAGE2_STOP_ON_SUITE_DONE:-1}"
export WHUSE_STAGE2_CLEAN_QEMU="${WHUSE_STAGE2_CLEAN_QEMU:-1}"
export WHUSE_OSCOMP_PROFILE="${WHUSE_OSCOMP_PROFILE:-}"
export WHUSE_OSCOMP_CASE_FILTER="${WHUSE_OSCOMP_CASE_FILTER:-}"
export WHUSE_OSCOMP_RUNTIME_FILTER="${WHUSE_OSCOMP_RUNTIME_FILTER:-both}"
export WHUSE_LTP_PROFILE="${WHUSE_LTP_PROFILE:-score}"
export WHUSE_LTP_WHITELIST="${WHUSE_LTP_WHITELIST:-${REPO_ROOT}/tools/oscomp/ltp/score_whitelist.txt}"
export WHUSE_LTP_BLACKLIST="${WHUSE_LTP_BLACKLIST:-${REPO_ROOT}/tools/oscomp/ltp/score_blacklist.txt}"
export WHUSE_LTP_STEP_TIMEOUT="${WHUSE_LTP_STEP_TIMEOUT:-1800}"
export WHUSE_LTP_CASE_TIMEOUT="${WHUSE_LTP_CASE_TIMEOUT:-45}"
export WHUSE_LTP_APPLY_CANDIDATES="${WHUSE_LTP_APPLY_CANDIDATES:-0}"
WHUSE_STAGE2_IMAGE_POLICY="${WHUSE_STAGE2_IMAGE_POLICY:-auto}"

RV_IMG="${REPO_ROOT}/target/oscomp/sdcard-rv.img"
LA_IMG="${REPO_ROOT}/target/oscomp/sdcard-la.img"

if [[ "${WHUSE_OSCOMP_COMPAT}" != "0" ]]; then
    echo "WHUSE_OSCOMP_COMPAT must be 0 for stage2 real-execution runs" >&2
    exit 1
fi

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
    "ltp_testcode.sh"
)

validate_oscomp_profile() {
    local profile="${1:-}"
    case "${profile}" in
    "" | full | basic | busybox | iozone | libctest | libc-bench | lmbench | lua | ltp | unixbench | netperf | iperf | cyclic)
        return 0
        ;;
    *)
        echo "invalid WHUSE_OSCOMP_PROFILE=${profile}" >&2
        return 1
        ;;
    esac
}

validate_oscomp_profile "${WHUSE_OSCOMP_PROFILE}"
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
        printf '%s\n' "${full_root_steps[@]}"
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
    case "${WHUSE_OSCOMP_RUNTIME_FILTER}" in
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
cleanup_done=0
prepared_runtime_image=""

cleanup_runtime_images() {
    for img in "${runtime_images[@]}"; do
        rm -f "${img}"
    done
}

cleanup_stale_qemu() {
    if [[ "${WHUSE_STAGE2_CLEAN_QEMU}" != "1" ]]; then
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
    echo "[cleanup] scanning stale qemu/docker instances"
    bash "${cleanup_script}" "${cleanup_args[@]}" || true
}

cleanup_all() {
    local rc=$?
    if [[ "${cleanup_done}" -eq 1 ]]; then
        return "${rc}"
    fi
    cleanup_done=1
    cleanup_stale_qemu
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
    ltp-riscv) requested_arches=("rv") ;;
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
    if [[ "${WHUSE_STAGE2_USE_IMAGE_COPY}" != "1" && -z "${WHUSE_OSCOMP_CASE_FILTER}" && "${WHUSE_OSCOMP_RUNTIME_FILTER}" == "both" ]]; then
        cleanup_target_images+=("${src}")
        prepared_runtime_image="${src}"
        return
    fi
    local dst="/tmp/whuse-${arch}-stage2-${TS}.img"
    cp --reflink=auto "${src}" "${dst}"
    runtime_images+=("${dst}")
    cleanup_target_images+=("${dst}")
    prepared_runtime_image="${dst}"
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

inject_oscomp_runtime_filter_env() {
    local image="$1"
    local runtime_filter="$2"
    write_runtime_image_config "${image}" "/musl/.whuse_stage2_local.env" "WHUSE_OSCOMP_RUNTIME_FILTER=${runtime_filter}"
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

inject_ltp_runtime_config() {
    local image="$1"
    local ltp_whitelist_path="${WHUSE_LTP_WHITELIST:-}"
    local ltp_blacklist_path="${WHUSE_LTP_BLACKLIST:-}"
    inject_oscomp_profile "${image}" "ltp"
    write_runtime_image_config "${image}" "/musl/.whuse_ltp_profile" "${WHUSE_LTP_PROFILE}"
    if [[ -n "${ltp_whitelist_path}" && -f "${ltp_whitelist_path}" ]]; then
        write_runtime_image_file "${image}" "/musl/ltp_score_whitelist.host.txt" "${ltp_whitelist_path}"
        ltp_whitelist_path="/musl/ltp_score_whitelist.host.txt"
    fi
    if [[ -n "${ltp_blacklist_path}" && -f "${ltp_blacklist_path}" ]]; then
        write_runtime_image_file "${image}" "/musl/ltp_score_blacklist.host.txt" "${ltp_blacklist_path}"
        ltp_blacklist_path="/musl/ltp_score_blacklist.host.txt"
    fi
    if [[ -n "${WHUSE_LTP_WHITELIST:-}" ]]; then
        write_runtime_image_config "${image}" "/musl/.whuse_ltp_whitelist" "${ltp_whitelist_path}"
    fi
    if [[ -n "${WHUSE_LTP_BLACKLIST:-}" ]]; then
        write_runtime_image_config "${image}" "/musl/.whuse_ltp_blacklist" "${ltp_blacklist_path}"
    fi
    if [[ -n "${WHUSE_LTP_STEP_TIMEOUT:-}" ]]; then
        write_runtime_image_config "${image}" "/musl/.whuse_ltp_step_timeout" "${WHUSE_LTP_STEP_TIMEOUT}"
    fi
    if [[ -n "${WHUSE_LTP_CASE_TIMEOUT:-}" ]]; then
        write_runtime_image_config "${image}" "/musl/.whuse_ltp_case_timeout" "${WHUSE_LTP_CASE_TIMEOUT}"
    fi
}

generate_ltp_candidate_lists() {
    local text_log="$1"
    local pass_out="$2"
    local bad_out="$3"
    local pass_tmp bad_tmp summary_tmp
    pass_tmp="$(mktemp)"
    bad_tmp="$(mktemp)"
    summary_tmp="$(mktemp)"
    awk -F: -v pass_file="${pass_tmp}" -v bad_file="${bad_tmp}" -v summary_file="${summary_tmp}" '
        /^RUN LTP CASE / {
            current_case = $0
            sub(/^RUN LTP CASE /, "", current_case)
            next
        }
        {
            if (current_case != "") {
                if ($0 ~ /TPASS/) {
                    stream_tpass[current_case]++
                }
                if ($0 ~ /TFAIL/) {
                    stream_tfail[current_case]++
                }
                if ($0 ~ /TBROK/) {
                    stream_tbrok[current_case]++
                }
                if ($0 ~ /TCONF/) {
                    stream_tconf[current_case]++
                }
            }
        }
        /^whuse-ltp-case-result:/ {
            case_name=$2
            delete kvs
            for (i = 3; i <= NF; i++) {
                n = split($i, kv, "=")
                if (n == 2) {
                    kvs[kv[1]] = kv[2]
                }
            }
            rc = kvs["rc"] + 0
            tpass = kvs["tpass"] + 0
            tfail = kvs["tfail"] + 0
            tbrok = kvs["tbrok"] + 0
            class = kvs["class"]
            if (tpass == 0 && (case_name in stream_tpass)) {
                tpass = stream_tpass[case_name]
            }
            if (tfail == 0 && (case_name in stream_tfail)) {
                tfail = stream_tfail[case_name]
            }
            if (tbrok == 0 && (case_name in stream_tbrok)) {
                tbrok = stream_tbrok[case_name]
            }
            class_count[class]++
            if (rc == 0 && tpass > 0 && tfail == 0 && tbrok == 0) {
                print case_name >> pass_file
            } else if (class == "rc255" || class == "timeout" || class == "missing" || class == "tbrok" || class == "tfail" || class == "nonzero") {
                print case_name >> bad_file
            }
            current_case = ""
        }
        END {
            for (k in class_count) {
                printf("%s=%d\n", k, class_count[k]) >> summary_file
            }
        }
    ' "${text_log}"
    sort -u "${pass_tmp}" > "${pass_out}"
    sort -u "${bad_tmp}" > "${bad_out}"
    echo "[rv-ltp] class summary:"
    if [[ -s "${summary_tmp}" ]]; then
        sort "${summary_tmp}" | sed 's/^/[rv-ltp] class-count /'
    else
        echo "[rv-ltp] class-count none"
    fi
    rm -f "${pass_tmp}" "${bad_tmp}" "${summary_tmp}"
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
    local effective_profile
    prepare_runtime_image "${arch}" "${image}"
    runtime_image="${prepared_runtime_image}"
    if [[ -n "${WHUSE_OSCOMP_PROFILE}" ]]; then
        inject_oscomp_profile "${runtime_image}" "${WHUSE_OSCOMP_PROFILE}"
    fi
    effective_profile="$(effective_oscomp_profile)"
    validate_oscomp_profile "${effective_profile}"
    if [[ "${WHUSE_OSCOMP_RUNTIME_FILTER}" != "both" ]]; then
        inject_oscomp_runtime_filter "${runtime_image}" "${WHUSE_OSCOMP_RUNTIME_FILTER}"
        inject_oscomp_runtime_filter_env "${runtime_image}" "${WHUSE_OSCOMP_RUNTIME_FILTER}"
    fi
    inject_case_filter_files "${runtime_image}" "${effective_profile}"

    echo "[${arch}] running ${xtask_cmd}, timeout=${TIMEOUT_SECS}s, image=${runtime_image}, stop-on-suite-done=${WHUSE_STAGE2_STOP_ON_SUITE_DONE}, oscomp-profile=${effective_profile}, case-filter=${WHUSE_OSCOMP_CASE_FILTER:-none}, runtime-filter=${WHUSE_OSCOMP_RUNTIME_FILTER}"
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

    local -a expected_steps=()
    mapfile -t expected_steps < <(resolve_expected_root_steps "${effective_profile}")
    for step in "${expected_steps[@]}"; do
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

    if [[ "${effective_profile}" == "basic" ]]; then
        if case_filter_matches_profile "basic"; then
            local musl_case_markers glibc_case_markers
            musl_case_markers="$(count_runtime_case_markers "${text_log}" "whuse-oscomp-basic-case" "musl")"
            glibc_case_markers="$(count_runtime_case_markers "${text_log}" "whuse-oscomp-basic-case" "glibc")"
            local basic_marker_fail=0
            if runtime_filter_selects_runtime "musl" && [[ "${musl_case_markers}" -lt 1 ]]; then
                basic_marker_fail=1
            fi
            if runtime_filter_selects_runtime "glibc" && [[ "${glibc_case_markers}" -lt 1 ]]; then
                basic_marker_fail=1
            fi
            if [[ "${basic_marker_fail}" -ne 0 ]]; then
                echo "[${arch}] basic profile failed semantic check: expected whuse-oscomp-basic-case markers in selected runtimes (filter=${WHUSE_OSCOMP_RUNTIME_FILTER}, musl=${musl_case_markers}, glibc=${glibc_case_markers})" >&2
                ok=1
            else
                echo "[${arch}] basic profile semantic check ok: runtime-filter=${WHUSE_OSCOMP_RUNTIME_FILTER}, musl basic-case markers=${musl_case_markers}, glibc basic-case markers=${glibc_case_markers}"
            fi
        else
            local musl_brk_count glibc_brk_count
            musl_brk_count="$(count_step_semantic_lines "${text_log}" "musl/basic_testcode.sh" '^Testing brk :')"
            glibc_brk_count="$(count_step_semantic_lines "${text_log}" "glibc/basic_testcode.sh" '^Testing brk :')"
            local basic_brk_fail=0
            if runtime_filter_selects_runtime "musl" && [[ "${musl_brk_count}" -lt 1 ]]; then
                basic_brk_fail=1
            fi
            if runtime_filter_selects_runtime "glibc" && [[ "${glibc_brk_count}" -lt 1 ]]; then
                basic_brk_fail=1
            fi
            if [[ "${basic_brk_fail}" -ne 0 ]]; then
                echo "[${arch}] basic profile failed semantic check: expected Testing brk output in selected runtimes (filter=${WHUSE_OSCOMP_RUNTIME_FILTER}, musl=${musl_brk_count}, glibc=${glibc_brk_count})" >&2
                ok=1
            else
                echo "[${arch}] basic profile semantic check ok: runtime-filter=${WHUSE_OSCOMP_RUNTIME_FILTER}, musl Testing brk=${musl_brk_count}, glibc Testing brk=${glibc_brk_count}"
            fi
        fi
    fi

    if [[ "${effective_profile}" == "busybox" ]]; then
        if case_filter_matches_profile "busybox"; then
            local musl_busybox_markers glibc_busybox_markers
            musl_busybox_markers="$(count_runtime_case_markers "${text_log}" "whuse-oscomp-busybox-case" "musl")"
            glibc_busybox_markers="$(count_runtime_case_markers "${text_log}" "whuse-oscomp-busybox-case" "glibc")"
            local busybox_marker_fail=0
            if runtime_filter_selects_runtime "musl" && [[ "${musl_busybox_markers}" -lt 1 ]]; then
                busybox_marker_fail=1
            fi
            if runtime_filter_selects_runtime "glibc" && [[ "${glibc_busybox_markers}" -lt 1 ]]; then
                busybox_marker_fail=1
            fi
            if [[ "${busybox_marker_fail}" -ne 0 ]]; then
                echo "[${arch}] busybox profile failed semantic check: expected whuse-oscomp-busybox-case markers in selected runtimes (filter=${WHUSE_OSCOMP_RUNTIME_FILTER}, musl=${musl_busybox_markers}, glibc=${glibc_busybox_markers})" >&2
                ok=1
            else
                echo "[${arch}] busybox profile semantic check ok: runtime-filter=${WHUSE_OSCOMP_RUNTIME_FILTER}, musl busybox-case markers=${musl_busybox_markers}, glibc busybox-case markers=${glibc_busybox_markers}"
            fi
        else
            local musl_busybox_cases glibc_busybox_cases
            musl_busybox_cases="$(count_step_semantic_lines "${text_log}" "musl/busybox_testcode.sh" 'testcase busybox .* success')"
            glibc_busybox_cases="$(count_step_semantic_lines "${text_log}" "glibc/busybox_testcode.sh" 'testcase busybox .* success')"
            local busybox_case_fail=0
            if runtime_filter_selects_runtime "musl" && [[ "${musl_busybox_cases}" -lt 1 ]]; then
                busybox_case_fail=1
            fi
            if runtime_filter_selects_runtime "glibc" && [[ "${glibc_busybox_cases}" -lt 1 ]]; then
                busybox_case_fail=1
            fi
            if [[ "${busybox_case_fail}" -ne 0 ]]; then
                echo "[${arch}] busybox profile failed semantic check: expected testcase busybox output in selected runtimes (filter=${WHUSE_OSCOMP_RUNTIME_FILTER}, musl=${musl_busybox_cases}, glibc=${glibc_busybox_cases})" >&2
                ok=1
            else
                echo "[${arch}] busybox profile semantic check ok: runtime-filter=${WHUSE_OSCOMP_RUNTIME_FILTER}, musl testcase busybox=${musl_busybox_cases}, glibc testcase busybox=${glibc_busybox_cases}"
            fi
        fi
    fi

    echo "[${arch}] marker summary:"
    rg "whuse-oscomp-step-(begin|end|timeout|skip)|whuse-oscomp-(basic|busybox)-case|whuse-oscomp-suite-done" "${text_log}" || true

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

run_ltp_riscv() {
    local log="/tmp/rv-ltp-stage2-${TS}.log"
    local text_log="/tmp/rv-ltp-stage2-${TS}.strings.log"
    local runtime_image
    local suite_done_seen=0
    local terminated_by_suite_done=0
    prepare_runtime_image "rv" "${RV_IMG}"
    runtime_image="${prepared_runtime_image}"
    inject_ltp_runtime_config "${runtime_image}"

    echo "[rv-ltp] running ltp-only, timeout=${TIMEOUT_SECS}s, image=${runtime_image}, profile=${WHUSE_LTP_PROFILE}, stop-on-suite-done=${WHUSE_STAGE2_STOP_ON_SUITE_DONE}"
    if [[ "${WHUSE_STAGE2_STOP_ON_SUITE_DONE}" == "1" ]]; then
        local runner_pid
        setsid timeout "${TIMEOUT_SECS}s" env \
            WHUSE_DISK_IMAGE="${runtime_image}" \
            WHUSE_LTP_PROFILE="${WHUSE_LTP_PROFILE}" \
            WHUSE_LTP_CASE_TIMEOUT="${WHUSE_LTP_CASE_TIMEOUT}" \
            "${XTASK_CMD[@]}" qemu-riscv >"${log}" 2>&1 &
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
                kill -KILL -- "-${runner_pid}" 2>/dev/null || true
                break
            fi
            sleep 2
        done
        wait "${runner_pid}" 2>/dev/null || true
    else
        timeout "${TIMEOUT_SECS}s" env \
            WHUSE_DISK_IMAGE="${runtime_image}" \
            WHUSE_LTP_PROFILE="${WHUSE_LTP_PROFILE}" \
            WHUSE_LTP_CASE_TIMEOUT="${WHUSE_LTP_CASE_TIMEOUT}" \
            "${XTASK_CMD[@]}" qemu-riscv >"${log}" 2>&1 || true
    fi

    strings "${log}" >"${text_log}" || true
    echo "[rv-ltp] log: ${log}"

    if rg -q "KERNEL PANIC|panic|pid 1 \(init\).*trap" "${text_log}"; then
        echo "[rv-ltp] detected kernel panic or init crash" >&2
        rg "KERNEL PANIC|panic|pid 1 \(init\).*trap" "${text_log}" >&2 || true
        return 1
    fi

    local ok=0
    if rg -q "whuse-oscomp-step-begin:ltp_testcode.sh" "${text_log}"; then
        echo "[rv-ltp] step-begin ok: ltp_testcode.sh"
    else
        echo "[rv-ltp] missing step-begin: ltp_testcode.sh" >&2
        ok=1
    fi
    if rg -q "whuse-oscomp-step-end:ltp_testcode.sh:" "${text_log}" || rg -q "whuse-oscomp-step-timeout:ltp_testcode.sh" "${text_log}"; then
        echo "[rv-ltp] step-close ok: ltp_testcode.sh"
    else
        echo "[rv-ltp] missing step-close: ltp_testcode.sh" >&2
        ok=1
    fi

    local tpass tfail tbrok tconf timeout_count
    local pass_candidates bad_candidates
    tpass="$(count_matches "TPASS" "${text_log}")"
    tfail="$(count_matches "TFAIL" "${text_log}")"
    tbrok="$(count_matches "TBROK" "${text_log}")"
    tconf="$(count_matches "TCONF" "${text_log}")"
    timeout_count="$(count_matches "whuse-oscomp-step-timeout:ltp_testcode.sh" "${text_log}")"
    pass_candidates="/tmp/rv-ltp-pass-candidates-${TS}.txt"
    bad_candidates="/tmp/rv-ltp-bad-candidates-${TS}.txt"
    generate_ltp_candidate_lists "${text_log}" "${pass_candidates}" "${bad_candidates}"
    echo "[rv-ltp] pass-candidates: ${pass_candidates} ($(wc -l < "${pass_candidates}"))"
    echo "[rv-ltp] bad-candidates:  ${bad_candidates} ($(wc -l < "${bad_candidates}"))"
    if [[ "${WHUSE_LTP_APPLY_CANDIDATES}" == "1" ]]; then
        cp "${pass_candidates}" "${REPO_ROOT}/tools/oscomp/ltp/score_whitelist.txt"
        cp "${bad_candidates}" "${REPO_ROOT}/tools/oscomp/ltp/score_blacklist.txt"
        echo "[rv-ltp] applied candidate lists to tools/oscomp/ltp/score_whitelist.txt and score_blacklist.txt"
    fi
    if [[ "${suite_done_seen}" == "0" ]] && rg -q "whuse-oscomp-suite-done" "${text_log}"; then
        suite_done_seen=1
    fi
    echo "[rv-ltp] summary: TPASS=${tpass} TFAIL=${tfail} TBROK=${tbrok} TCONF=${tconf} step-timeout=${timeout_count} suite_done_seen=${suite_done_seen} terminated_by_suite_done=${terminated_by_suite_done}"
    rg "whuse-oscomp-step-(begin|end|timeout|skip):ltp_testcode.sh|whuse-oscomp-suite-done|whuse-ltp-(skip-case|case-result):" "${text_log}" || true
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
ltp-riscv)
    run_ltp_riscv
    ;;
both)
    run_arch "rv" "${RV_IMG}" "qemu-riscv"
    run_arch "la" "${LA_IMG}" "qemu-loongarch"
    ;;
*)
    echo "usage: $0 [riscv|loongarch|ltp-riscv|both]" >&2
    exit 2
    ;;
esac
