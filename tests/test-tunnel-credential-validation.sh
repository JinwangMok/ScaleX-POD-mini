#!/usr/bin/env bash
# Test: validate_tunnel_credentials exits with code 2 on missing/invalid credentials

set -uo pipefail

PASS=0
FAIL=0
INSTALL_SH="/home/jinwang/local-workspace/ScaleX-POD-mini/install.sh"

# Extract function body
func_body=$(awk '/^validate_tunnel_credentials\(\)/{found=1; depth=0} found{print; for(i=1;i<=length($0);i++){c=substr($0,i,1); if(c=="{") depth++; if(c=="}") depth--}; if(found && depth==0 && NR>1){exit}}' "$INSTALL_SH")

assert_exit_code() {
  local desc="$1" expected="$2" repo_dir="$3"
  local actual=0
  # Run in subshell so exit 2 doesn't terminate our test script
  ( 
    i18n() { echo "$1"; }
    log_info() { :; }
    log_error() { echo "[ERROR] $*" >&2; }
    error_msg() { echo "[ERROR_MSG] $1" >&2; }
    eval "$func_body"
    validate_tunnel_credentials "$repo_dir"
  ) 2>/dev/null || actual=$?
  if [[ "$actual" == "$expected" ]]; then
    echo "PASS: $desc (exit $actual)"
    PASS=$((PASS+1))
  else
    echo "FAIL: $desc (expected exit $expected, got $actual)"
    FAIL=$((FAIL+1))
  fi
}

TMPDIR=$(mktemp -d)
trap 'rm -rf "$TMPDIR"' EXIT

setup_repo() {
  local dir="$TMPDIR/$1"
  mkdir -p "$dir/credentials"
  echo "$dir"
}

# Test 1: SSH key missing → exit 2
T=$(setup_repo t1)
printf 'nodes:\n  - name: "node1"\n    sshAuthMode: "key"\n' > "$T/credentials/.baremetal-init.yaml"
printf 'SSH_KEY_PATH="/nonexistent/path/id_ed25519"\n' > "$T/credentials/.env"
assert_exit_code "SSH key file missing → exit 2" 2 "$T"

# Test 2: SSH key exists but not a private key → exit 2
T=$(setup_repo t2)
printf 'nodes:\n  - name: "node1"\n    sshAuthMode: "key"\n' > "$T/credentials/.baremetal-init.yaml"
echo "this is not a private key" > "$TMPDIR/fake_key"
printf 'SSH_KEY_PATH="%s"\n' "$TMPDIR/fake_key" > "$T/credentials/.env"
assert_exit_code "SSH key invalid (no PEM header) → exit 2" 2 "$T"

# Test 3: SSH password mode, no CF → exit 0
T=$(setup_repo t3)
printf 'nodes:\n  - name: "node1"\n    sshAuthMode: "password"\n' > "$T/credentials/.baremetal-init.yaml"
printf 'PLAYBOX_0_PASSWORD="somepass"\n' > "$T/credentials/.env"
assert_exit_code "SSH password mode, no CF → exit 0" 0 "$T"

# Test 4: CF Tunnel referenced but cloudflare-tunnel.json missing → exit 2
T=$(setup_repo t4)
printf 'nodes:\n  - name: "node1"\n    sshAuthMode: "password"\n' > "$T/credentials/.baremetal-init.yaml"
printf 'cloudflare:\n  credentials_file: "credentials/cloudflare-tunnel.json"\n' > "$T/credentials/secrets.yaml"
assert_exit_code "CF Tunnel referenced but file missing → exit 2" 2 "$T"

# Test 5: CF Tunnel JSON present with placeholder values → exit 2
T=$(setup_repo t5)
printf 'nodes:\n  - name: "node1"\n    sshAuthMode: "password"\n' > "$T/credentials/.baremetal-init.yaml"
printf '{"AccountTag":"<YOUR_CLOUDFLARE_ACCOUNT_ID>","TunnelSecret":"<YOUR_TUNNEL_SECRET>","TunnelID":"<YOUR_TUNNEL_ID>"}\n' > "$T/credentials/cloudflare-tunnel.json"
assert_exit_code "CF Tunnel JSON has placeholders → exit 2" 2 "$T"

# Test 6: CF Tunnel JSON missing TunnelSecret → exit 2
T=$(setup_repo t6)
printf 'nodes:\n  - name: "node1"\n    sshAuthMode: "password"\n' > "$T/credentials/.baremetal-init.yaml"
printf '{"AccountTag":"real-account-tag","TunnelSecret":"","TunnelID":"real-tunnel-id"}\n' > "$T/credentials/cloudflare-tunnel.json"
assert_exit_code "CF Tunnel JSON empty TunnelSecret → exit 2" 2 "$T"

# Test 7: Valid SSH key + valid CF Tunnel → exit 0
T=$(setup_repo t7)
ssh-keygen -t ed25519 -N "" -f "$TMPDIR/valid_key" -q 2>/dev/null || true
printf 'nodes:\n  - name: "node1"\n    sshAuthMode: "key"\n' > "$T/credentials/.baremetal-init.yaml"
printf 'SSH_KEY_PATH="%s"\n' "$TMPDIR/valid_key" > "$T/credentials/.env"
printf '{"AccountTag":"real-account-12345","TunnelSecret":"real-secret-abcdef","TunnelID":"real-tunnel-id-abcd"}\n' > "$T/credentials/cloudflare-tunnel.json"
assert_exit_code "Valid SSH key + valid CF Tunnel → exit 0" 0 "$T"

# Test 8: No SSH key needed, no CF Tunnel → exit 0
T=$(setup_repo t8)
printf 'nodes:\n  - name: "node1"\n    sshAuthMode: "password"\n' > "$T/credentials/.baremetal-init.yaml"
printf 'PLAYBOX_0_PASSWORD="somepass"\n' > "$T/credentials/.env"
assert_exit_code "No key/CF Tunnel needed → exit 0" 0 "$T"

echo ""
echo "Results: $PASS passed, $FAIL failed"
[[ $FAIL -eq 0 ]]
