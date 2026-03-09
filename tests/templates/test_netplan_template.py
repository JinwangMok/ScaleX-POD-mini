"""Tests for Jinja2 netplan template rendering."""
import pytest
import yaml
from jinja2 import Environment, FileSystemLoader
import os


@pytest.fixture
def jinja_env(project_root):
    return Environment(
        loader=FileSystemLoader(os.path.join(project_root, "ansible", "templates")),
        keep_trailing_newline=True,
    )


def render_netplan(jinja_env, bond_interfaces, node_ip, gateway, nameservers,
                   bond_mode="active-backup", create_bridge=False):
    template = jinja_env.get_template("netplan.yml.j2")
    rendered = template.render(
        bond_interfaces=bond_interfaces,
        node_ip=node_ip,
        network={"gateway": gateway, "nameservers": nameservers},
        gateway=gateway,
        nameservers=nameservers,
        bond_mode=bond_mode,
        create_bridge=create_bridge,
    )
    return yaml.safe_load(rendered)


class TestSingleNIC:
    """Case 1: Single NIC (playbox-1) — bond0 with 1 slave, IP on bond0."""

    def test_renders_valid_yaml(self, jinja_env):
        result = render_netplan(
            jinja_env,
            bond_interfaces=[
                {"name": "eno1", "mac": "64:00:6a:5c:b0:53", "mtu": 1500}
            ],
            node_ip="192.168.88.9/24",
            gateway="192.168.88.1",
            nameservers=["8.8.8.8", "8.8.4.4"],
        )
        assert result is not None
        assert "network" in result

    def test_bond0_has_single_slave(self, jinja_env):
        result = render_netplan(
            jinja_env,
            bond_interfaces=[
                {"name": "eno1", "mac": "64:00:6a:5c:b0:53", "mtu": 1500}
            ],
            node_ip="192.168.88.9/24",
            gateway="192.168.88.1",
            nameservers=["8.8.8.8", "8.8.4.4"],
        )
        bond = result["network"]["bonds"]["bond0"]
        assert bond["interfaces"] == ["eno1"]
        assert "192.168.88.9/24" in bond["addresses"]

    def test_no_bridge(self, jinja_env):
        result = render_netplan(
            jinja_env,
            bond_interfaces=[
                {"name": "eno1", "mac": "64:00:6a:5c:b0:53", "mtu": 1500}
            ],
            node_ip="192.168.88.9/24",
            gateway="192.168.88.1",
            nameservers=["8.8.8.8", "8.8.4.4"],
        )
        assert "bridges" not in result["network"]


class TestSingleNICWithBridge:
    """Case 2: Single NIC + tower host — bond0 + br0, IP on br0."""

    def test_bridge_has_ip(self, jinja_env):
        result = render_netplan(
            jinja_env,
            bond_interfaces=[
                {"name": "eno1", "mac": "64:00:6a:5c:ac:dd", "mtu": 1500}
            ],
            node_ip="192.168.88.8/24",
            gateway="192.168.88.1",
            nameservers=["8.8.8.8", "8.8.4.4"],
            create_bridge=True,
        )
        assert "bridges" in result["network"]
        br0 = result["network"]["bridges"]["br0"]
        assert "192.168.88.8/24" in br0["addresses"]
        assert br0["interfaces"] == ["bond0"]

    def test_bond0_has_no_ip_when_bridge(self, jinja_env):
        result = render_netplan(
            jinja_env,
            bond_interfaces=[
                {"name": "eno1", "mac": "64:00:6a:5c:ac:dd", "mtu": 1500}
            ],
            node_ip="192.168.88.8/24",
            gateway="192.168.88.1",
            nameservers=["8.8.8.8", "8.8.4.4"],
            create_bridge=True,
        )
        bond = result["network"]["bonds"]["bond0"]
        assert "addresses" not in bond


class TestMultiNIC:
    """Case 3: Multi-NIC (playbox-3) — bond0 with 2 slaves, 10G as primary."""

    def test_two_slaves(self, jinja_env):
        result = render_netplan(
            jinja_env,
            bond_interfaces=[
                {"name": "ens2f0np0", "mac": "b8:59:9f:f5:a5:a2", "mtu": 1500},
                {"name": "eno1", "mac": "40:b0:34:1a:80:0b", "mtu": 1500},
            ],
            node_ip="192.168.88.11/24",
            gateway="192.168.88.1",
            nameservers=["8.8.8.8", "8.8.4.4"],
        )
        bond = result["network"]["bonds"]["bond0"]
        assert len(bond["interfaces"]) == 2
        assert "ens2f0np0" in bond["interfaces"]
        assert "eno1" in bond["interfaces"]

    def test_primary_is_first_interface(self, jinja_env):
        result = render_netplan(
            jinja_env,
            bond_interfaces=[
                {"name": "ens2f0np0", "mac": "b8:59:9f:f5:a5:a2", "mtu": 1500},
                {"name": "eno1", "mac": "40:b0:34:1a:80:0b", "mtu": 1500},
            ],
            node_ip="192.168.88.11/24",
            gateway="192.168.88.1",
            nameservers=["8.8.8.8", "8.8.4.4"],
        )
        bond = result["network"]["bonds"]["bond0"]
        assert bond["parameters"]["primary"] == "ens2f0np0"
        assert bond["parameters"]["mii-monitor-interval"] == 100


class TestEmptyInterfaces:
    """Case 4: No valid interfaces — template should produce empty/minimal output."""

    def test_no_bonds_with_empty_list(self, jinja_env):
        result = render_netplan(
            jinja_env,
            bond_interfaces=[],
            node_ip="192.168.88.8/24",
            gateway="192.168.88.1",
            nameservers=["8.8.8.8", "8.8.4.4"],
        )
        # With no interfaces, there should be no bonds section
        assert result["network"].get("bonds") is None or result["network"].get("bonds") == {}
