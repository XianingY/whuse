#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"

dry_run=0
host_only=0
docker_only=0
all_oscomp_containers=0
docker_image="${WHUSE_OSCOMP_DOCKER_IMAGE:-docker.educg.net/cg/os-contest:20260104}"
declare -a explicit_images=()
declare -a candidate_pids=()
declare -a candidate_images=()
declare -a candidate_container_ids=()
declare -a candidate_container_images=()
declare -a candidate_container_drives=()

usage() {
	cat <<'EOF'
Usage: cleanup_stale_qemu.sh [--dry-run] [--host-only|--docker-only] [--all-oscomp-containers] [--image /abs/path]...

Kill stale qemu-system-riscv64/qemu-system-loongarch64 processes and optional
Docker os-contest containers by matching their -drive file path.
Without --image, default targets are:
  - <repo>/target/oscomp/sdcard-rv.img
  - <repo>/target/oscomp/sdcard-la.img
  - /work/target/oscomp/sdcard-rv.img
  - /work/target/oscomp/sdcard-la.img
  - /tmp/whuse-*.img
  - /tmp/rv-*.img
  - /tmp/la-*.img

Options:
  --all-oscomp-containers  stop every running container from WHUSE_OSCOMP_DOCKER_IMAGE
                           whose command includes qemu-system-*
  --host-only              only kill host-side qemu/docker wrapper processes
  --docker-only            only stop matching Docker containers
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
	--host-only)
		host_only=1
		shift
		;;
	--docker-only)
		docker_only=1
		shift
		;;
	--all-oscomp-containers)
		all_oscomp_containers=1
		shift
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

if [[ "${host_only}" -eq 1 && "${docker_only}" -eq 1 ]]; then
	echo "--host-only and --docker-only cannot be used together" >&2
	exit 2
fi

extract_drive_image() {
	local cmd="$1"
	local image
	image="$(sed -n 's/.*-drive file=\([^,[:space:]]*\),.*/\1/p' <<<"${cmd}")"
	printf '%s' "${image}"
}

docker_available() {
	command -v docker >/dev/null 2>&1 && docker info >/dev/null 2>&1
}

to_container_image_path() {
	local image="$1"
	if [[ "${image}" == "${REPO_ROOT}/"* ]]; then
		printf '/work/%s' "${image#${REPO_ROOT}/}"
	fi
}

matches_default_image() {
	local image="$1"
	[[ "${image}" == "${REPO_ROOT}/target/oscomp/sdcard-rv.img" ]] && return 0
	[[ "${image}" == "${REPO_ROOT}/target/oscomp/sdcard-la.img" ]] && return 0
	[[ "${image}" == /work/target/oscomp/sdcard-rv.img ]] && return 0
	[[ "${image}" == /work/target/oscomp/sdcard-la.img ]] && return 0
	[[ "${image}" == /tmp/whuse-*.img ]] && return 0
	[[ "${image}" == /tmp/rv-*.img ]] && return 0
	[[ "${image}" == /tmp/la-*.img ]] && return 0
	return 1
}

matches_target_image() {
	local image="$1"
	if [[ ${#explicit_images[@]} -gt 0 ]]; then
		local target
		for target in "${explicit_images[@]}"; do
			[[ "${image}" == "${target}" ]] && return 0
			local mapped_target
			mapped_target="$(to_container_image_path "${target}")"
			if [[ -n "${mapped_target}" && "${image}" == "${mapped_target}" ]]; then
				return 0
			fi
		done
		return 1
	fi
	matches_default_image "${image}"
}

collect_host_candidates() {
	[[ "${docker_only}" -eq 1 ]] && return
	while IFS= read -r line; do
		[[ -z "${line}" ]] && continue
		local local_pid="${line%% *}"
		local local_cmd="${line#* }"
		local drive_image
		drive_image="$(extract_drive_image "${local_cmd}")"
		[[ -z "${drive_image}" ]] && continue
		if matches_target_image "${drive_image}"; then
			candidate_pids+=("${local_pid}")
			candidate_images+=("${drive_image}")
		fi
	done < <(ps -eo pid=,args= | awk '/qemu-system-(loongarch64|riscv64)/ { sub(/^ +/, "", $0); print }')
}

collect_container_candidates() {
	[[ "${host_only}" -eq 1 ]] && return
	if ! docker_available; then
		return
	fi
	while IFS='|' read -r container_id image command; do
		[[ -z "${container_id}" ]] && continue
		if [[ "${command}" != *qemu-system-riscv64* && "${command}" != *qemu-system-loongarch64* ]]; then
			continue
		fi
		if [[ "${all_oscomp_containers}" -eq 1 ]]; then
			if [[ "${image}" != "${docker_image}" ]]; then
				continue
			fi
			candidate_container_ids+=("${container_id}")
			candidate_container_images+=("${image}")
			candidate_container_drives+=("*")
			continue
		fi
		local drive_image
		drive_image="$(extract_drive_image "${command}")"
		[[ -z "${drive_image}" ]] && continue
		if matches_target_image "${drive_image}"; then
			candidate_container_ids+=("${container_id}")
			candidate_container_images+=("${image}")
			candidate_container_drives+=("${drive_image}")
		fi
	done < <(docker ps --no-trunc --format '{{.ID}}|{{.Image}}|{{.Command}}')
}

collect_host_candidates
collect_container_candidates

if [[ ${#candidate_pids[@]} -eq 0 && ${#candidate_container_ids[@]} -eq 0 ]]; then
	echo "no matching stale qemu processes or containers found"
	exit 0
fi

if [[ ${#candidate_container_ids[@]} -gt 0 ]]; then
	echo "matching docker qemu containers:"
	for idx in "${!candidate_container_ids[@]}"; do
		echo "  container=${candidate_container_ids[$idx]} image=${candidate_container_images[$idx]} drive=${candidate_container_drives[$idx]}"
	done
fi

if [[ ${#candidate_pids[@]} -gt 0 ]]; then
	echo "matching host qemu processes:"
	for idx in "${!candidate_pids[@]}"; do
		echo "  pid=${candidate_pids[$idx]} image=${candidate_images[$idx]}"
	done
fi

if [[ "${dry_run}" -eq 1 ]]; then
	echo "dry-run enabled; nothing was stopped"
	exit 0
fi

if [[ ${#candidate_container_ids[@]} -gt 0 ]]; then
	echo "stopping docker containers..."
	docker stop "${candidate_container_ids[@]}" >/dev/null 2>&1 || true
	declare -a container_survivors=()
	local_id_list="$(docker ps -q --no-trunc || true)"
	for container_id in "${candidate_container_ids[@]}"; do
		if grep -qx "${container_id}" <<<"${local_id_list}"; then
			container_survivors+=("${container_id}")
		fi
	done
	if [[ ${#container_survivors[@]} -gt 0 ]]; then
		echo "still alive after stop, forcing kill: ${container_survivors[*]}"
		docker kill "${container_survivors[@]}" >/dev/null 2>&1 || true
	fi
fi

if [[ ${#candidate_pids[@]} -gt 0 ]]; then
	echo "sending TERM to host qemu processes..."
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
fi

echo "remaining qemu host processes:"
pgrep -af 'qemu-system-(loongarch64|riscv64)' || true
if docker_available; then
	echo "remaining docker qemu containers:"
	docker ps --format '{{.ID}} {{.Image}} {{.Command}}' | awk '/qemu-system-(loongarch64|riscv64)/ { print }' || true
fi
