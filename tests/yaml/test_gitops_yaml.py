"""Validate all GitOps YAML files."""
import os
import yaml
import pytest
import glob


def get_yaml_files(project_root):
    """Find all YAML files in gitops/."""
    gitops_dir = os.path.join(project_root, "gitops")
    files = []
    for ext in ("*.yaml", "*.yml"):
        files.extend(glob.glob(os.path.join(gitops_dir, "**", ext), recursive=True))
    return files


def test_all_gitops_yaml_valid(project_root):
    """All YAML files in gitops/ must be valid YAML."""
    files = get_yaml_files(project_root)
    assert len(files) > 0, "No YAML files found in gitops/"

    errors = []
    for f in files:
        try:
            with open(f) as fh:
                list(yaml.safe_load_all(fh))
        except yaml.YAMLError as e:
            errors.append(f"{f}: {e}")

    assert errors == [], f"YAML parse errors:\n" + "\n".join(errors)


def test_spread_yaml_has_three_documents(project_root):
    """spread.yaml should have 3 YAML documents."""
    path = os.path.join(project_root, "gitops", "bootstrap", "spread.yaml")
    with open(path) as f:
        docs = list(yaml.safe_load_all(f))
    assert len(docs) == 3


def test_spread_yaml_contains_root_project(project_root):
    path = os.path.join(project_root, "gitops", "bootstrap", "spread.yaml")
    with open(path) as f:
        docs = list(yaml.safe_load_all(f))

    project = docs[0]
    assert project["kind"] == "AppProject"
    assert project["metadata"]["name"] == "playbox-root"


def test_spread_yaml_disables_default_project(project_root):
    path = os.path.join(project_root, "gitops", "bootstrap", "spread.yaml")
    with open(path) as f:
        docs = list(yaml.safe_load_all(f))

    default_proj = docs[2]
    assert default_proj["metadata"]["name"] == "default"
    assert default_proj["spec"]["sourceRepos"] == []
