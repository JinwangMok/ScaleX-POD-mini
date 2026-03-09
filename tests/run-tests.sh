#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

ERRORS=0

echo "=== YAML Lint ==="
if command -v yamllint &>/dev/null; then
    yamllint -c "${PROJECT_ROOT}/.yamllint.yml" \
        "${PROJECT_ROOT}/gitops/" \
        "${PROJECT_ROOT}/values.yaml" || ERRORS=$((ERRORS + 1))
else
    echo "SKIP: yamllint not installed"
fi

echo ""
echo "=== Template & YAML Tests (pytest) ==="
if command -v pytest &>/dev/null; then
    pytest "${SCRIPT_DIR}/templates/" "${SCRIPT_DIR}/yaml/" -v || ERRORS=$((ERRORS + 1))
else
    echo "SKIP: pytest not installed"
fi

echo ""
echo "=== Shell Tests (BATS) ==="
if command -v bats &>/dev/null; then
    bats "${SCRIPT_DIR}/bats/"*.bats || ERRORS=$((ERRORS + 1))
else
    echo "SKIP: bats not installed"
fi

echo ""
echo "=== ShellCheck ==="
if command -v shellcheck &>/dev/null; then
    shellcheck "${PROJECT_ROOT}/playbox" "${PROJECT_ROOT}"/lib/*.sh || ERRORS=$((ERRORS + 1))
else
    echo "SKIP: shellcheck not installed"
fi

echo ""
echo "=== OpenTofu Validate ==="
if command -v tofu &>/dev/null; then
    cd "${PROJECT_ROOT}/tofu" && tofu init -backend=false -input=false 2>/dev/null && tofu validate && cd "${PROJECT_ROOT}" || ERRORS=$((ERRORS + 1))
else
    echo "SKIP: tofu not installed"
fi

echo ""
if [[ ${ERRORS} -gt 0 ]]; then
    echo "FAILED: ${ERRORS} test suite(s) had errors"
    exit 1
else
    echo "All tests passed!"
fi
