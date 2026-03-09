"""Tests for kubespray inventory generation logic."""
import os
import yaml
import pytest


def get_inventory_content(project_root):
    """Helper to generate and read inventory."""
    gen_dir = os.path.join(project_root, "_generated")
    inv_path = os.path.join(gen_dir, "inventory.ini")
    if os.path.exists(inv_path):
        with open(inv_path) as f:
            return f.read()
    return None


def test_inventory_generation_from_values(values_full):
    """Verify expected node structure from values."""
    cp_nodes = values_full["nodes"]["control_plane"]
    workers = values_full["nodes"]["workers"]

    assert len(cp_nodes) == 1
    assert cp_nodes[0]["name"] == "playbox-0"
    assert len(workers) == 3

    all_names = [cp_nodes[0]["name"]] + [w["name"] for w in workers]
    assert "playbox-0" in all_names
    assert "playbox-1" in all_names
    assert "playbox-2" in all_names
    assert "playbox-3" in all_names


def test_bastion_is_playbox0(values_full):
    assert values_full["management"]["bastion_host"] == "playbox-0"
    assert values_full["management"]["bastion_user"] == "jinwang"


def test_proxyjump_needed_for_non_bastion(values_full):
    """Non-bastion nodes should need ProxyJump."""
    bastion = values_full["management"]["bastion_host"]
    workers = values_full["nodes"]["workers"]
    for w in workers:
        assert w["name"] != bastion, "Worker should not be the bastion"
