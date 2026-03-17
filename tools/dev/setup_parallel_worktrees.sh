#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"

RV_BRANCH="${RV_BRANCH:-arch/riscv-stage1}"
LA_BRANCH="${LA_BRANCH:-arch/loongarch-stage1}"
INT_BRANCH="${INT_BRANCH:-integration/stage1}"
BASE_REF="${BASE_REF:-master}"

RV_TREE="${RV_TREE:-${REPO_ROOT}/../whuse-rv}"
LA_TREE="${LA_TREE:-${REPO_ROOT}/../whuse-la}"

normalize_path() {
    realpath -m "$1"
}

ensure_branch() {
    local branch="$1"
    if git show-ref --verify --quiet "refs/heads/${branch}"; then
        return
    fi
    git branch "${branch}" "${BASE_REF}"
    echo "created branch ${branch} from ${BASE_REF}"
}

ensure_worktree() {
    local branch="$1"
    local path="$2"
    local canonical_path
    canonical_path="$(normalize_path "${path}")"
    while read -r listed; do
        if [[ "$(normalize_path "${listed}")" == "${canonical_path}" ]]; then
            echo "worktree already exists: ${canonical_path}"
            return
        fi
    done < <(git worktree list --porcelain | sed -n 's/^worktree //p')
    if [[ -e "${canonical_path}" ]]; then
        echo "path exists and is not a registered worktree: ${canonical_path}" >&2
        exit 1
    fi
    git worktree add "${canonical_path}" "${branch}"
}

cd "${REPO_ROOT}"

ensure_branch "${RV_BRANCH}"
ensure_branch "${LA_BRANCH}"
ensure_branch "${INT_BRANCH}"

ensure_worktree "${RV_BRANCH}" "${RV_TREE}"
ensure_worktree "${LA_BRANCH}" "${LA_TREE}"

git checkout "${INT_BRANCH}" >/dev/null

echo "parallel setup complete:"
echo "  integration: ${INT_BRANCH} -> ${REPO_ROOT}"
echo "  riscv worktree: $(normalize_path "${RV_TREE}") (${RV_BRANCH})"
echo "  loongarch worktree: $(normalize_path "${LA_TREE}") (${LA_BRANCH})"
