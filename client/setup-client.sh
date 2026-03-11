#!/usr/bin/env bash
# setup-client.sh — Client setup helper
set -euo pipefail

echo "=== ScaleX-POD-mini Client Setup ==="
echo ""

# Check prerequisites
for tool in kubectl; do
    if ! command -v "${tool}" &>/dev/null; then
        echo "ERROR: ${tool} not found. Please install it first."
        exit 1
    fi
done

# Check kubelogin
if ! kubectl oidc-login --help &>/dev/null 2>&1; then
    echo "Installing kubelogin (kubectl oidc-login plugin)..."
    if command -v kubectl-krew &>/dev/null || kubectl krew version &>/dev/null 2>&1; then
        kubectl krew install oidc-login
    else
        echo "Please install krew first: https://krew.sigs.k8s.io/docs/user-guide/setup/install/"
        echo "Then run: kubectl krew install oidc-login"
        exit 1
    fi
fi

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
KUBECONFIG_SRC="${SCRIPT_DIR}/kubeconfig-oidc.yaml"

if [[ ! -f "${KUBECONFIG_SRC}" ]]; then
    echo "ERROR: kubeconfig-oidc.yaml not found. Run 'scalex cluster init' first."
    exit 1
fi

echo ""
echo "Kubeconfig found at: ${KUBECONFIG_SRC}"
echo ""
echo "To use this kubeconfig:"
echo "  export KUBECONFIG=${KUBECONFIG_SRC}"
echo ""
echo "Or copy to default location:"
echo "  mkdir -p ~/.kube"
echo "  cp ${KUBECONFIG_SRC} ~/.kube/config"
echo ""
echo "Then test with:"
echo "  kubectl get nodes"
echo ""
echo "This will open a browser for OIDC authentication."
