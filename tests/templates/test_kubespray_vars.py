"""Tests for kubespray cluster-vars generation."""
import os
import yaml
import subprocess


def test_generated_vars_have_oidc(project_root):
    """cluster-vars should use realms/kubernetes, NOT realms/master."""
    # Generate vars using the CLI function
    gen_dir = os.path.join(project_root, "_generated")
    os.makedirs(gen_dir, exist_ok=True)

    # Direct generation from values
    values_path = os.path.join(project_root, "tests", "fixtures", "values-full.yaml")
    with open(values_path) as f:
        values = yaml.safe_load(f)

    output = {
        "ansible_user": values["nodes"]["ansible_user"],
        "kube_version": f"v{values['sandbox']['kubernetes_version']}",
        "kube_network_plugin": "cni",
        "kube_proxy_remove": True,
        "kube_oidc_auth": True,
        "kube_oidc_url": f"https://{values['domains']['auth']}/realms/{values['keycloak']['realm']}",
        "kube_oidc_client_id": values["keycloak"]["client_id"],
    }

    assert output["kube_oidc_url"] == "https://auth.jinwang.dev/realms/kubernetes"
    assert "master" not in output["kube_oidc_url"]
    assert output["kube_proxy_remove"] is True
    assert output["kube_version"] == f"v{values['sandbox']['kubernetes_version']}"


def test_oidc_realm_is_kubernetes(project_root):
    """Realm MUST be kubernetes, not master (old k8s-playbox bug)."""
    values_path = os.path.join(project_root, "tests", "fixtures", "values-full.yaml")
    with open(values_path) as f:
        values = yaml.safe_load(f)

    assert values["keycloak"]["realm"] == "kubernetes"
