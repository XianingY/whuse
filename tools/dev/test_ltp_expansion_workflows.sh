#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
LTP_DIR="${REPO_ROOT}/tools/oscomp/ltp"
REFRESHER="${REPO_ROOT}/tools/dev/refresh_ltp_reference_seeds.py"
ROUND_HELPER="${REPO_ROOT}/tools/dev/run_ltp_rv_batch_round.sh"
IMAGE_REFRESHER="${REPO_ROOT}/tools/dev/refresh_ltp_image_bins.py"
IMAGE_ROUND_GENERATOR="${REPO_ROOT}/tools/dev/generate_ltp_image_rounds.py"

assert_file_exists() {
    local path="$1"
    if [[ ! -f "${path}" ]]; then
        echo "missing file: ${path}" >&2
        exit 1
    fi
}

assert_contains() {
    local path="$1"
    local needle="$2"
    if ! grep -Fq "${needle}" "${path}"; then
        echo "missing expected content in ${path}: ${needle}" >&2
        exit 1
    fi
}

assert_no_overlap() {
    local lhs="$1"
    local rhs="$2"
    local overlap
    overlap="$(comm -12 <(sort "${lhs}") <(sort "${rhs}"))"
    if [[ -n "${overlap}" ]]; then
        echo "unexpected overlap between ${lhs} and ${rhs}:" >&2
        echo "${overlap}" >&2
        exit 1
    fi
}

assert_line_count_ge() {
    local path="$1"
    local min_count="$2"
    local actual
    actual="$(wc -l < "${path}")"
    if (( actual < min_count )); then
        echo "expected ${path} to have at least ${min_count} lines, got ${actual}" >&2
        exit 1
    fi
}

assert_file_not_exists() {
    local path="$1"
    if [[ -e "${path}" ]]; then
        echo "unexpected file present: ${path}" >&2
        exit 1
    fi
}

assert_image_phase_layout() {
    local round_name="$1"
    local chunk_size="$2"
    local base_file phase1 phase2 total
    base_file="${LTP_DIR}/musl_rv_image_round_${round_name}.txt"
    phase1="${LTP_DIR}/musl_rv_image_round_${round_name}_phase1.txt"
    phase2="${LTP_DIR}/musl_rv_image_round_${round_name}_phase2.txt"
    assert_file_exists "${base_file}"
    total="$(wc -l < "${base_file}")"
    if (( total == 0 )); then
        assert_file_not_exists "${phase1}"
        assert_file_not_exists "${phase2}"
    elif (( total > chunk_size )); then
        assert_file_exists "${phase1}"
        assert_line_count_ge "${phase1}" 1
        assert_file_exists "${phase2}"
        assert_line_count_ge "${phase2}" 1
    else
        assert_file_exists "${phase1}"
        assert_line_count_ge "${phase1}" 1
        assert_file_not_exists "${phase2}"
    fi
}

IMAGE_PHASE_SIZE="${WHUSE_LTP_IMAGE_PHASE_SIZE:-128}"
if ! [[ "${IMAGE_PHASE_SIZE}" =~ ^[0-9]+$ ]] || (( IMAGE_PHASE_SIZE <= 0 )); then
    echo "invalid WHUSE_LTP_IMAGE_PHASE_SIZE=${IMAGE_PHASE_SIZE}" >&2
    exit 1
fi

assert_file_exists "${REFRESHER}"
assert_file_exists "${ROUND_HELPER}"
assert_file_exists "${IMAGE_REFRESHER}"
assert_file_exists "${IMAGE_ROUND_GENERATOR}"

assert_contains "${REFRESHER}" "musl_rv_seed_ref_undefined.txt"
assert_contains "${REFRESHER}" "musl_rv_seed_ref_nighthawk.txt"
assert_contains "${REFRESHER}" "musl_rv_seed_ref_starry_mix.txt"
assert_contains "${REFRESHER}" "runltp_rvml"
assert_contains "${REFRESHER}" "ltp_testlist"
assert_contains "${REFRESHER}" "all_testcases"
assert_contains "${IMAGE_REFRESHER}" "ltp/testcases/bin"
assert_contains "${IMAGE_REFRESHER}" "debugfs"
assert_contains "${IMAGE_ROUND_GENERATOR}" "musl_rv_seed_image_bins.txt"
assert_contains "${IMAGE_ROUND_GENERATOR}" "musl_rv_seed_ref_nighthawk.txt"
assert_contains "${IMAGE_ROUND_GENERATOR}" "musl_rv_curated_whitelist.txt"
assert_contains "${IMAGE_ROUND_GENERATOR}" "musl_rv_curated_blacklist.txt"
assert_contains "${IMAGE_ROUND_GENERATOR}" "musl_rv_flaky_score_cases.txt"

assert_contains "${ROUND_HELPER}" "stable-pass.txt"
assert_contains "${ROUND_HELPER}" "stable-bad.txt"
assert_contains "${ROUND_HELPER}" "score_whitelist.txt"
assert_contains "${ROUND_HELPER}" "score_blacklist.txt"
assert_contains "${ROUND_HELPER}" "ltp-riscv-curated"

assert_file_exists "${LTP_DIR}/musl_rv_seed_ref_undefined.txt"
assert_file_exists "${LTP_DIR}/musl_rv_seed_ref_nighthawk.txt"
assert_file_exists "${LTP_DIR}/musl_rv_seed_ref_starry_mix.txt"
assert_file_exists "${LTP_DIR}/musl_rv_seed_image_bins.txt"
assert_file_exists "${LTP_DIR}/musl_rv_flaky_score_cases.txt"

assert_line_count_ge "${LTP_DIR}/musl_rv_seed_ref_undefined.txt" 300
assert_line_count_ge "${LTP_DIR}/musl_rv_seed_ref_nighthawk.txt" 500
assert_line_count_ge "${LTP_DIR}/musl_rv_seed_ref_starry_mix.txt" 300
assert_line_count_ge "${LTP_DIR}/musl_rv_seed_image_bins.txt" 2500
assert_contains "${LTP_DIR}/musl_rv_flaky_score_cases.txt" "epoll_pwait01"
assert_contains "${LTP_DIR}/musl_rv_flaky_score_cases.txt" "epoll_wait04"

for round_file in \
    "${LTP_DIR}/musl_rv_round1_sync.txt" \
    "${LTP_DIR}/musl_rv_round1_path.txt" \
    "${LTP_DIR}/musl_rv_round1_open_io.txt" \
    "${LTP_DIR}/musl_rv_round1_file_syscalls.txt" \
    "${LTP_DIR}/musl_rv_round1_sync_phase2.txt" \
    "${LTP_DIR}/musl_rv_round1_open_io_phase1.txt" \
    "${LTP_DIR}/musl_rv_score_wave1_ab_tail.txt" \
    "${LTP_DIR}/musl_rv_score_wave2_vector_tail.txt" \
    "${LTP_DIR}/musl_rv_score_wave3_process_signal_phase2.txt" \
    "${LTP_DIR}/musl_rv_score_wave4_curated_backlog.txt" \
    "${LTP_DIR}/musl_rv_score_wave5_image_phase1.txt" \
    "${LTP_DIR}/musl_rv_score_wave6_image_phase2.txt"
do
    assert_file_exists "${round_file}"
    if [[ -s "${round_file}" || "${round_file}" == *"open_io_phase1.txt" ]]; then
        assert_line_count_ge "${round_file}" 1
    fi
done

assert_contains "${LTP_DIR}/musl_rv_score_wave1_ab_tail.txt" "pipe08"
assert_contains "${LTP_DIR}/musl_rv_score_wave1_ab_tail.txt" "write04"
assert_contains "${LTP_DIR}/musl_rv_score_wave2_vector_tail.txt" "openat02"
assert_contains "${LTP_DIR}/musl_rv_score_wave2_vector_tail.txt" "writev02"
assert_contains "${LTP_DIR}/musl_rv_score_wave3_process_signal_phase2.txt" "waitpid08"
assert_contains "${LTP_DIR}/musl_rv_score_wave4_curated_backlog.txt" "access01"
assert_contains "${LTP_DIR}/musl_rv_score_wave4_curated_backlog.txt" "waitpid09"
assert_contains "${LTP_DIR}/musl_rv_score_wave5_image_phase1.txt" "signal03"
assert_contains "${LTP_DIR}/musl_rv_score_wave5_image_phase1.txt" "writev06"
assert_contains "${LTP_DIR}/musl_rv_score_wave6_image_phase2.txt" "chmod03"
assert_contains "${LTP_DIR}/musl_rv_score_wave6_image_phase2.txt" "writev06"

for image_round in \
    "${LTP_DIR}/musl_rv_image_round_fs_path.txt" \
    "${LTP_DIR}/musl_rv_image_round_open_io.txt" \
    "${LTP_DIR}/musl_rv_image_round_process_signal.txt" \
    "${LTP_DIR}/musl_rv_image_round_socket_basic.txt" \
    "${LTP_DIR}/musl_rv_image_round_time.txt"
do
    assert_file_exists "${image_round}"
done

assert_image_phase_layout "fs_path" "${IMAGE_PHASE_SIZE}"
assert_image_phase_layout "open_io" "${IMAGE_PHASE_SIZE}"
assert_image_phase_layout "process_signal" "${IMAGE_PHASE_SIZE}"

assert_file_exists "${LTP_DIR}/musl_rv_blacklist_review_round1.txt"
assert_contains "${LTP_DIR}/musl_rv_blacklist_review_round1.txt" "dup03"
assert_contains "${LTP_DIR}/musl_rv_blacklist_review_round1.txt" "dup06"
assert_contains "${LTP_DIR}/musl_rv_blacklist_review_round1.txt" "dup205"
assert_contains "${LTP_DIR}/musl_rv_blacklist_review_round1.txt" "fcntl05"
assert_contains "${LTP_DIR}/musl_rv_blacklist_review_round1.txt" "fstat03"
assert_contains "${LTP_DIR}/musl_rv_blacklist_review_round1.txt" "statvfs02"

tmpdir="$(mktemp -d)"
trap 'rm -rf "${tmpdir}"' EXIT

round_file="${tmpdir}/round.txt"
pass_file="${tmpdir}/pass.txt"
bad_file="${tmpdir}/bad.txt"
conf_file="${tmpdir}/conf.txt"
count_file="${tmpdir}/count"
fake_stage2="${tmpdir}/fake-stage2.sh"

cat > "${round_file}" <<'EOF'
dummy_case
EOF

cat > "${pass_file}" <<'EOF'
alpha
EOF

cat > "${bad_file}" <<'EOF'
beta
EOF

cat > "${conf_file}" <<'EOF'
gamma
EOF

cat > "${fake_stage2}" <<EOF
#!/usr/bin/env bash
set -euo pipefail
count_file="${count_file}"
count=0
if [[ -f "\${count_file}" ]]; then
    count="\$(cat "\${count_file}")"
fi
count=\$((count + 1))
printf '%s\n' "\${count}" > "\${count_file}"
echo "[rv-ltp-curated] pass-candidates: ${pass_file} (1)"
echo "[rv-ltp-curated] bad-candidates:  ${bad_file} (1)"
echo "[rv-ltp-curated] conf-candidates: ${conf_file} (1)"
if [[ "\${count}" == "1" ]]; then
    exit 1
fi
exit 0
EOF
chmod +x "${fake_stage2}"

helper_log="${tmpdir}/helper.log"
STAGE2="${fake_stage2}" WHUSE_LTP_ROUNDS=2 WHUSE_LTP_OUTPUT_DIR="${tmpdir}/out" \
    "${ROUND_HELPER}" "${round_file}" > "${helper_log}" 2>&1

assert_file_exists "${tmpdir}/out/stable-pass.txt"
assert_file_exists "${tmpdir}/out/stable-bad.txt"
assert_file_exists "${tmpdir}/out/stable-conf.txt"
assert_contains "${tmpdir}/out/stable-pass.txt" "alpha"
assert_contains "${tmpdir}/out/stable-bad.txt" "beta"
assert_contains "${tmpdir}/out/stable-conf.txt" "gamma"
assert_contains "${helper_log}" "round 2/2"

empty_round="${tmpdir}/empty-round.txt"
touch "${empty_round}"
empty_count_file="${tmpdir}/empty-count"
empty_stage2="${tmpdir}/empty-stage2.sh"

cat > "${empty_stage2}" <<EOF
#!/usr/bin/env bash
set -euo pipefail
printf 'invoked\n' >> "${empty_count_file}"
exit 99
EOF
chmod +x "${empty_stage2}"

empty_log="${tmpdir}/empty-helper.log"
STAGE2="${empty_stage2}" WHUSE_LTP_OUTPUT_DIR="${tmpdir}/empty-out" \
    "${ROUND_HELPER}" "${empty_round}" > "${empty_log}" 2>&1

assert_file_exists "${tmpdir}/empty-out/stable-pass.txt"
assert_file_exists "${tmpdir}/empty-out/stable-bad.txt"
assert_file_exists "${tmpdir}/empty-out/stable-conf.txt"
assert_contains "${empty_log}" "no candidate cases"
if [[ -e "${empty_count_file}" ]]; then
    echo "empty round unexpectedly invoked stage2" >&2
    exit 1
fi

recover_round="${tmpdir}/recover-round.txt"
recover_stage2_log="${tmpdir}/recover-stage2.log"
recover_stage2="${tmpdir}/recover-stage2.sh"
recover_helper_log="${tmpdir}/recover-helper.log"
cat > "${recover_round}" <<'EOF'
alpha
beta
gamma
delta
EOF

cat > "${recover_stage2_log}" <<'EOF'
RUN LTP CASE alpha
whuse-ltp-case-result:alpha:rc=0:tpass=1:tfail=0:tbrok=0:tconf=0:class=pass
RUN LTP CASE beta
whuse-ltp-case-result:beta:rc=1:tpass=0:tfail=1:tbrok=0:tconf=0:class=tfail
RUN LTP CASE gamma
whuse-ltp-case-result:gamma:rc=0:tpass=0:tfail=0:tbrok=0:tconf=1:class=conf-only
RUN LTP CASE delta
EOF

cat > "${recover_stage2}" <<EOF
#!/usr/bin/env bash
set -euo pipefail
echo "[rv-ltp-curated] log: ${recover_stage2_log}"
echo "[rv-ltp-curated] detected kernel panic or init crash"
exit 1
EOF
chmod +x "${recover_stage2}"

STAGE2="${recover_stage2}" WHUSE_LTP_ROUNDS=1 WHUSE_LTP_OUTPUT_DIR="${tmpdir}/recover-out" \
    "${ROUND_HELPER}" "${recover_round}" > "${recover_helper_log}" 2>&1

assert_contains "${recover_helper_log}" "recovered candidates from stage2 log"
assert_contains "${tmpdir}/recover-out/stable-pass.txt" "alpha"
assert_contains "${tmpdir}/recover-out/stable-bad.txt" "beta"
assert_contains "${tmpdir}/recover-out/stable-bad.txt" "delta"
assert_contains "${tmpdir}/recover-out/stable-conf.txt" "gamma"

echo "ok"
