"""Validate values.yaml against expected schema."""
import os
import yaml
import pytest


def validate_required_fields(values):
    """Check all required fields are present and non-empty."""
    errors = []

    required = [
        ("cluster.name", values.get("cluster", {}).get("name")),
        ("cluster.domain", values.get("cluster", {}).get("domain")),
        ("nodes.superuser", values.get("nodes", {}).get("superuser")),
        ("nodes.ansible_user", values.get("nodes", {}).get("ansible_user")),
        ("nodes.ssh_key", values.get("nodes", {}).get("ssh_key")),
        ("network.gateway", values.get("network", {}).get("gateway")),
    ]

    for field_name, value in required:
        if not value:
            errors.append(f"Missing required field: {field_name}")

    # At least one control plane node
    cp = values.get("nodes", {}).get("control_plane", [])
    if not cp:
        errors.append("At least one control_plane node required")

    # All nodes must have MAC addresses
    all_nodes = cp + values.get("nodes", {}).get("workers", [])
    for node in all_nodes:
        for iface in node.get("interfaces", []):
            if not iface.get("mac"):
                errors.append(f"Interface {iface.get('name')} on {node.get('name')} missing MAC")

    return errors


def test_full_values_valid(values_full):
    errors = validate_required_fields(values_full)
    assert errors == [], f"Validation errors: {errors}"


def test_minimal_values_valid(values_minimal):
    errors = validate_required_fields(values_minimal)
    assert errors == [], f"Validation errors: {errors}"


def test_invalid_values_fail(values_invalid):
    errors = validate_required_fields(values_invalid)
    assert len(errors) > 0, "Invalid values should have validation errors"


def test_full_values_has_all_sections(values_full):
    required_sections = [
        "cluster", "nodes", "network", "management",
        "tower", "sandbox", "cloudflare", "domains",
        "keycloak", "argocd", "versions"
    ]
    for section in required_sections:
        assert section in values_full, f"Missing section: {section}"


def test_versions_present(values_full):
    versions = values_full["versions"]
    assert "cilium" in versions
    assert "argocd_chart" in versions
    assert "keycloak_chart" in versions
    assert "cert_manager" in versions
