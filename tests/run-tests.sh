#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

ERRORS=0

echo "=== YAML Lint ==="
if command -v yamllint &>/dev/null; then
    yamllint -c "${PROJECT_ROOT}/.yamllint.yml" \
        "${PROJECT_ROOT}/gitops/" || ERRORS=$((ERRORS + 1))
else
    echo "SKIP: yamllint not installed"
fi

echo ""
echo "=== Rust CLI Tests (cargo test) ==="
if command -v cargo &>/dev/null; then
    cd "${PROJECT_ROOT}/scalex-cli" && cargo test || ERRORS=$((ERRORS + 1))
    cd "${PROJECT_ROOT}"
else
    echo "SKIP: cargo not installed"
fi

echo ""
echo "=== Rust Lint (clippy) ==="
if command -v cargo &>/dev/null; then
    cd "${PROJECT_ROOT}/scalex-cli" && cargo clippy -- -D warnings || ERRORS=$((ERRORS + 1))
    cd "${PROJECT_ROOT}"
else
    echo "SKIP: cargo/clippy not installed"
fi

echo ""
echo "=== Rust Format Check ==="
if command -v cargo &>/dev/null; then
    cd "${PROJECT_ROOT}/scalex-cli" && cargo fmt --check || ERRORS=$((ERRORS + 1))
    cd "${PROJECT_ROOT}"
else
    echo "SKIP: cargo/rustfmt not installed"
fi

echo ""
if [[ ${ERRORS} -gt 0 ]]; then
    echo "FAILED: ${ERRORS} test suite(s) had errors"
    exit 1
else
    echo "All tests passed!"
fi
