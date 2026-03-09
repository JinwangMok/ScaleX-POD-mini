"""Tests for client kubeconfig template rendering."""
import os
import yaml
from jinja2 import Environment, FileSystemLoader
import pytest


@pytest.fixture
def jinja_env(project_root):
    return Environment(
        loader=FileSystemLoader(os.path.join(project_root, "client")),
        keep_trailing_newline=True,
    )


def test_kubeconfig_server_url(jinja_env, values_full):
    template = jinja_env.get_template("kubeconfig-oidc.yaml.j2")
    rendered = yaml.safe_load(template.render(**values_full))

    cluster = rendered["clusters"][0]["cluster"]
    assert cluster["server"] == "https://api.k8s.jinwang.dev"


def test_kubeconfig_no_insecure_flag(jinja_env, values_full):
    template = jinja_env.get_template("kubeconfig-oidc.yaml.j2")
    rendered_str = template.render(**values_full)

    assert "insecure-skip-tls-verify" not in rendered_str
    assert "certificate-authority-data" not in rendered_str


def test_kubeconfig_oidc_issuer(jinja_env, values_full):
    template = jinja_env.get_template("kubeconfig-oidc.yaml.j2")
    rendered = yaml.safe_load(template.render(**values_full))

    user = rendered["users"][0]["user"]
    args = user["exec"]["args"]
    issuer_arg = [a for a in args if "oidc-issuer-url" in a][0]
    assert "auth.jinwang.dev/realms/kubernetes" in issuer_arg


def test_kubeconfig_oidc_client_id(jinja_env, values_full):
    template = jinja_env.get_template("kubeconfig-oidc.yaml.j2")
    rendered = yaml.safe_load(template.render(**values_full))

    user = rendered["users"][0]["user"]
    args = user["exec"]["args"]
    client_arg = [a for a in args if "oidc-client-id" in a][0]
    assert "kubernetes" in client_arg
