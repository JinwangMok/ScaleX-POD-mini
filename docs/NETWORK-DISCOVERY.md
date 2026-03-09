# Network Discovery

## How to Get NIC Information

### Automated
```bash
./playbox discover-nics
```

### Manual (per node)
```bash
# List all interfaces with details
ip -j link show | python3 -c "
import json, sys
for d in json.load(sys.stdin):
    if d.get('link_type') == 'loopback': continue
    name = d.get('ifname', '?')
    mac = d.get('address', '?')
    state = d.get('operstate', '?').lower()
    print(f'{name}: mac={mac} state={state}')
"

# Get speed and driver
for iface in $(ls /sys/class/net/ | grep -v lo); do
    speed=$(cat /sys/class/net/$iface/speed 2>/dev/null || echo "?")
    driver=$(basename $(readlink /sys/class/net/$iface/device/driver 2>/dev/null) 2>/dev/null || echo "virtual")
    echo "$iface: speed=${speed}Mbps driver=${driver}"
done
```

## What to Put in values.yaml

For each interface, record:
- `name`: Interface name (e.g., eno1, ens2f0np0)
- `role`: common (general traffic), data (high-speed), mgmt (management), unused
- `mac`: MAC address (critical for netplan matching)
- `type`: ether or wifi
- `speed`: 1G, 10G, etc.
- `mtu`: Usually 1500
- `state`: up or down (down interfaces excluded from bond)

## Bond Behavior
- **1-NIC nodes**: bond0 wraps single NIC (consistency pattern)
- **Multi-NIC nodes**: bond0 combines all active NICs matching configured roles
- **Tower host (playbox-0)**: Gets br0 bridge over bond0 for VM networking
