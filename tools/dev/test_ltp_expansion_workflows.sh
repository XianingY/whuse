#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
LTP_DIR="${REPO_ROOT}/tools/oscomp/ltp"
REFRESHER="${REPO_ROOT}/tools/dev/refresh_ltp_reference_seeds.py"
ROUND_HELPER="${REPO_ROOT}/tools/dev/run_ltp_rv_batch_round.sh"

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

assert_file_exists "${REFRESHER}"
assert_file_exists "${ROUND_HELPER}"

assert_contains "${REFRESHER}" "musl_rv_seed_ref_undefined.txt"
assert_contains "${REFRESHER}" "musl_rv_seed_ref_nighthawk.txt"
assert_contains "${REFRESHER}" "musl_rv_seed_ref_starry_mix.txt"
assert_contains "${REFRESHER}" "runltp_rvml"
assert_contains "${REFRESHER}" "ltp_testlist"
assert_contains "${REFRESHER}" "all_testcases"

assert_contains "${ROUND_HELPER}" "stable-pass.txt"
assert_contains "${ROUND_HELPER}" "stable-bad.txt"
assert_contains "${ROUND_HELPER}" "score_whitelist.txt"
assert_contains "${ROUND_HELPER}" "score_blacklist.txt"
assert_contains "${ROUND_HELPER}" "ltp-riscv-curated"

assert_file_exists "${LTP_DIR}/musl_rv_seed_ref_undefined.txt"
assert_file_exists "${LTP_DIR}/musl_rv_seed_ref_nighthawk.txt"
assert_file_exists "${LTP_DIR}/musl_rv_seed_ref_starry_mix.txt"

assert_line_count_ge "${LTP_DIR}/musl_rv_seed_ref_undefined.txt" 300
assert_line_count_ge "${LTP_DIR}/musl_rv_seed_ref_nighthawk.txt" 500
assert_line_count_ge "${LTP_DIR}/musl_rv_seed_ref_starry_mix.txt" 300

for round_file in \
    "${LTP_DIR}/musl_rv_round1_sync.txt" \
    "${LTP_DIR}/musl_rv_round1_path.txt" \
    "${LTP_DIR}/musl_rv_round1_open_io.txt" \
    "${LTP_DIR}/musl_rv_round1_file_syscalls.txt" \
    "${LTP_DIR}/musl_rv_round1_sync_phase2.txt" \
    "${LTP_DIR}/musl_rv_round1_open_io_phase1.txt"
do
    assert_file_exists "${round_file}"
    if [[ -s "${round_file}" || "${round_file}" == *"open_io_phase1.txt" ]]; then
        assert_line_count_ge "${round_file}" 1
    fi
done

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
assert_contains "${tmpdir}/out/stable-pass.txt" "alpha"
assert_contains "${tmpdir}/out/stable-bad.txt" "beta"
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
assert_contains "${empty_log}" "no candidate cases"
if [[ -e "${empty_count_file}" ]]; then
    echo "empty round unexpectedly invoked stage2" >&2
    exit 1
fi

echo "ok"
