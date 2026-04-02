#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
LTP_DIR="${REPO_ROOT}/tools/oscomp/ltp"
STAGE2="${STAGE2:-${REPO_ROOT}/tools/dev/run_oscomp_stage2.sh}"
SCORE_WHITELIST="${LTP_DIR}/score_whitelist.txt"
SCORE_BLACKLIST="${LTP_DIR}/score_blacklist.txt"
CURATED_WHITELIST="${LTP_DIR}/musl_rv_curated_whitelist.txt"
CURATED_BLACKLIST="${LTP_DIR}/musl_rv_curated_blacklist.txt"

TARGET="${1:-}"
if [[ -z "${TARGET}" ]]; then
    echo "usage: $(basename "$0") <round-name|round-file>" >&2
    exit 1
fi

ROUNDS="${WHUSE_LTP_ROUNDS:-2}"
TIMEOUT_SECS="${TIMEOUT_SECS:-2400}"
BASE_BLACKLIST="${WHUSE_LTP_BLACKLIST:-${CURATED_BLACKLIST}}"
APPLY_STABLE="${WHUSE_LTP_APPLY_STABLE:-0}"
RUN_ID="$(date +%Y%m%d-%H%M%S)-$$-${RANDOM}"

canonicalize_path() {
    local path="$1"
    local dir base
    dir="$(dirname "${path}")"
    base="$(basename "${path}")"
    if [[ -d "${dir}" ]]; then
        (
            cd "${dir}"
            printf '%s/%s\n' "$(pwd -P)" "${base}"
        )
        return 0
    fi
    printf '%s\n' "${path}"
}

resolve_round_file() {
    local target="$1"
    if [[ -f "${target}" ]]; then
        printf '%s\n' "${target}"
        return 0
    fi
    if [[ -f "${LTP_DIR}/musl_rv_round1_${target}.txt" ]]; then
        printf '%s\n' "${LTP_DIR}/musl_rv_round1_${target}.txt"
        return 0
    fi
    echo "unknown round target: ${target}" >&2
    return 1
}

round_file_has_candidates() {
    local path="$1"
    grep -Eq '^[[:space:]]*[^#[:space:]]' "${path}"
}

assert_not_score_file() {
    local path="$1"
    local real
    real="$(canonicalize_path "${path}")"
    if [[ "${real}" == "$(canonicalize_path "${SCORE_WHITELIST}")" ]]; then
        echo "refusing to use protected score whitelist: ${path}" >&2
        return 1
    fi
    if [[ "${real}" == "$(canonicalize_path "${SCORE_BLACKLIST}")" ]]; then
        echo "refusing to use protected score blacklist: ${path}" >&2
        return 1
    fi
}

merge_into_curated() {
    local stable_pass="$1"
    local stable_bad="$2"
    local tmp_whitelist tmp_blacklist
    tmp_whitelist="$(mktemp)"
    tmp_blacklist="$(mktemp)"
    cat "${CURATED_WHITELIST}" "${stable_pass}" | sort -u > "${tmp_whitelist}"
    cat "${CURATED_BLACKLIST}" "${stable_bad}" | sort -u > "${tmp_blacklist}"
    grep -Fvx -f "${tmp_whitelist}" "${tmp_blacklist}" > "${tmp_blacklist}.filtered" || true
    mv "${tmp_whitelist}" "${CURATED_WHITELIST}"
    mv "${tmp_blacklist}.filtered" "${CURATED_BLACKLIST}"
    rm -f "${tmp_blacklist}"
}

ROUND_FILE="$(resolve_round_file "${TARGET}")"
assert_not_score_file "${ROUND_FILE}"
assert_not_score_file "${BASE_BLACKLIST}"

ROUND_NAME="$(basename "${ROUND_FILE}" .txt)"
OUT_DIR="${WHUSE_LTP_OUTPUT_DIR:-/tmp/whuse-${ROUND_NAME}-${RUN_ID}}"
mkdir -p "${OUT_DIR}"

if ! round_file_has_candidates "${ROUND_FILE}"; then
    : > "${OUT_DIR}/stable-pass.txt"
    : > "${OUT_DIR}/stable-bad.txt"
    echo "[${ROUND_NAME}] no candidate cases in ${ROUND_FILE}; skipping batch run"
    echo "[${ROUND_NAME}] stable-pass: ${OUT_DIR}/stable-pass.txt (0)"
    echo "[${ROUND_NAME}] stable-bad:  ${OUT_DIR}/stable-bad.txt (0)"
    exit 0
fi

pass_files=()
bad_files=()

for run_idx in $(seq 1 "${ROUNDS}"); do
    driver_log="${OUT_DIR}/run${run_idx}.driver.log"
    echo "[${ROUND_NAME}] round ${run_idx}/${ROUNDS} whitelist=${ROUND_FILE} blacklist=${BASE_BLACKLIST}" | tee "${driver_log}"
    set +e
    TIMEOUT_SECS="${TIMEOUT_SECS}" \
    WHUSE_LTP_PROFILE=curated \
    WHUSE_LTP_WHITELIST="${ROUND_FILE}" \
    WHUSE_LTP_BLACKLIST="${BASE_BLACKLIST}" \
    WHUSE_LTP_APPLY_CANDIDATES=0 \
    "${STAGE2}" ltp-riscv-curated | tee -a "${driver_log}"
    stage2_status=("${PIPESTATUS[@]}")
    set -e
    stage2_rc="${stage2_status[0]:-1}"
    tee_rc="${stage2_status[1]:-1}"
    echo "[${ROUND_NAME}] round ${run_idx} stage2-exit=${stage2_rc}" | tee -a "${driver_log}"
    if (( tee_rc != 0 )); then
        echo "tee failed for ${driver_log}" >&2
        exit "${tee_rc}"
    fi

    pass_path="$(sed -n 's/^.*pass-candidates: \([^ ]*\) (.*/\1/p' "${driver_log}" | tail -n1)"
    bad_path="$(sed -n 's/^.*bad-candidates:  \([^ ]*\) (.*/\1/p' "${driver_log}" | tail -n1)"
    if [[ -z "${pass_path}" || -z "${bad_path}" ]]; then
        echo "failed to parse candidate paths from ${driver_log} (stage2-exit=${stage2_rc})" >&2
        exit 1
    fi
    cp "${pass_path}" "${OUT_DIR}/run${run_idx}-pass.txt"
    cp "${bad_path}" "${OUT_DIR}/run${run_idx}-bad.txt"
    pass_files+=("${OUT_DIR}/run${run_idx}-pass.txt")
    bad_files+=("${OUT_DIR}/run${run_idx}-bad.txt")
done

stable_pass="${OUT_DIR}/stable-pass.txt"
stable_bad="${OUT_DIR}/stable-bad.txt"

cp "${pass_files[0]}" "${stable_pass}"
for pass_file in "${pass_files[@]:1}"; do
    comm -12 <(sort "${stable_pass}") <(sort "${pass_file}") > "${stable_pass}.next"
    mv "${stable_pass}.next" "${stable_pass}"
done

cat "${bad_files[@]}" | sort -u > "${stable_bad}"

echo "[${ROUND_NAME}] stable-pass: ${stable_pass} ($(wc -l < "${stable_pass}"))"
echo "[${ROUND_NAME}] stable-bad:  ${stable_bad} ($(wc -l < "${stable_bad}"))"

if [[ "${APPLY_STABLE}" == "1" ]]; then
    merge_into_curated "${stable_pass}" "${stable_bad}"
    echo "[${ROUND_NAME}] applied stable-pass/stable-bad to curated whitelist/blacklist"
fi
