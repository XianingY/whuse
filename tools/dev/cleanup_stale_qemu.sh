#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"

dry_run=0
declare -a explicit_images=()

usage() {
	cat <<'EOF'
Usage: cleanup_stale_qemu.sh [--dry-run] [--image /abs/path]...

Kill stale qemu-system-loongarch64 processes by matching their -drive file path.
Without --image, default targets are:
  - <repo>/target/oscomp/sdcard-la.img
  - /tmp/whuse-la-*.img
  - /tmp/whuse-la-stage1-*.img
  - /tmp/whuse-la-sysprobe-*.img
EOF
}

while [[ $# -gt 0 ]]; do
	case "$1" in
	--dry-run)
		dry_run=1
		shift
		;;
	--image)
		if [[ $# -lt 2 ]]; then
			echo "--image requires a value" >&2
			exit 2
		fi
		explicit_images+=("$2")
		shift 2
		;;
	-h | --help)
		usage
		exit 0
		;;
	*)
		echo "unknown argument: $1" >&2
		usage >&2
		exit 2
		;;
	esac
done

extract_drive_image() {
	local cmd="$1"
	local image
	image="$(sed -n 's/.*-drive file=\([^,[:space:]]*\),.*/\1/p' <<<"${cmd}")"
	printf '%s' "${image}"
}

matches_default_image() {
	local image="$1"
	[[ "${image}" == "${REPO_ROOT}/target/oscomp/sdcard-la.img" ]] && return 0
	[[ "${image}" == /tmp/whuse-la-*.img ]] && return 0
	[[ "${image}" == /tmp/whuse-la-stage1-*.img ]] && return 0
	[[ "${image}" == /tmp/whuse-la-sysprobe-*.img ]] && return 0
	return 1
}

matches_target_image() {
	local image="$1"
	if [[ ${#explicit_images[@]} -gt 0 ]]; then
		local target
		for target in "${explicit_images[@]}"; do
			[[ "${image}" == "${target}" ]] && return 0
		done
		return 1
	fi
	matches_default_image "${image}"
}

declare -a candidate_pids=()
declare -a candidate_images=()

while IFS= read -r line; do
	[[ -z "${line}" ]] && continue
	local_pid="${line%% *}"
	local_cmd="${line#* }"
	drive_image="$(extract_drive_image "${local_cmd}")"
	[[ -z "${drive_image}" ]] && continue
	if matches_target_image "${drive_image}"; then
		candidate_pids+=("${local_pid}")
		candidate_images+=("${drive_image}")
	fi
done < <(ps -eo pid=,args= | awk '/qemu-system-loongarch64/ { sub(/^ +/, "", $0); print }')

if [[ ${#candidate_pids[@]} -eq 0 ]]; then
	echo "no matching stale loongarch qemu processes found"
	exit 0
fi

echo "matching loongarch qemu processes:"
for idx in "${!candidate_pids[@]}"; do
	echo "  pid=${candidate_pids[$idx]} image=${candidate_images[$idx]}"
done

if [[ "${dry_run}" -eq 1 ]]; then
	echo "dry-run enabled; no process was killed"
	exit 0
fi

echo "sending TERM..."
kill -TERM "${candidate_pids[@]}" 2>/dev/null || true
sleep 2

declare -a survivors=()
for pid in "${candidate_pids[@]}"; do
	if ps -p "${pid}" >/dev/null 2>&1; then
		survivors+=("${pid}")
	fi
done

if [[ ${#survivors[@]} -gt 0 ]]; then
	echo "still alive after TERM, sending KILL: ${survivors[*]}"
	kill -KILL "${survivors[@]}" 2>/dev/null || true
fi

echo "remaining loongarch qemu processes:"
pgrep -af qemu-system-loongarch64 || true
