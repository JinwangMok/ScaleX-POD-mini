"""Verify catalog.yaml is consistent with app directories."""
import os
import yaml
import pytest


def test_base_apps_have_directories(project_root):
    """Every app in generators.base must have a directory under apps/base/."""
    catalog_path = os.path.join(project_root, "gitops", "clusters", "playbox", "catalog.yaml")
    with open(catalog_path) as f:
        catalog = yaml.safe_load(f)

    base_apps = catalog["generators"]["base"]
    apps_dir = os.path.join(project_root, "gitops", "clusters", "playbox", "apps", "base")

    missing = []
    for app in base_apps:
        app_path = os.path.join(apps_dir, app)
        if not os.path.isdir(app_path):
            missing.append(app)

    assert missing == [], f"Apps in catalog but missing directory: {missing}"


def test_test_apps_have_directories(project_root):
    """Every app in generators.test must have a directory under apps/test/."""
    catalog_path = os.path.join(project_root, "gitops", "clusters", "playbox", "catalog.yaml")
    with open(catalog_path) as f:
        catalog = yaml.safe_load(f)

    test_apps = catalog["generators"]["test"]
    apps_dir = os.path.join(project_root, "gitops", "clusters", "playbox", "apps", "test")

    missing = []
    for app in test_apps:
        app_path = os.path.join(apps_dir, app)
        if not os.path.isdir(app_path):
            missing.append(app)

    assert missing == [], f"Apps in catalog but missing directory: {missing}"


def test_no_orphan_base_directories(project_root):
    """No directory under apps/base/ that isn't in the catalog."""
    catalog_path = os.path.join(project_root, "gitops", "clusters", "playbox", "catalog.yaml")
    with open(catalog_path) as f:
        catalog = yaml.safe_load(f)

    base_apps = set(catalog["generators"]["base"])
    apps_dir = os.path.join(project_root, "gitops", "clusters", "playbox", "apps", "base")

    orphans = []
    if os.path.isdir(apps_dir):
        for d in os.listdir(apps_dir):
            if os.path.isdir(os.path.join(apps_dir, d)) and d not in base_apps:
                orphans.append(d)

    assert orphans == [], f"Orphan directories not in catalog: {orphans}"


def test_all_catalog_apps_have_required_fields(project_root):
    """Every app in catalog.apps must have destinationNamespace and syncWave."""
    catalog_path = os.path.join(project_root, "gitops", "clusters", "playbox", "catalog.yaml")
    with open(catalog_path) as f:
        catalog = yaml.safe_load(f)

    errors = []
    for app_name, app_config in catalog["apps"].items():
        if "destinationNamespace" not in app_config:
            errors.append(f"{app_name}: missing destinationNamespace")
        if "syncWave" not in app_config:
            errors.append(f"{app_name}: missing syncWave")

    assert errors == [], f"Catalog validation errors:\n" + "\n".join(errors)


def test_each_app_has_kustomization(project_root):
    """Every app directory must have a kustomization.yaml."""
    catalog_path = os.path.join(project_root, "gitops", "clusters", "playbox", "catalog.yaml")
    with open(catalog_path) as f:
        catalog = yaml.safe_load(f)

    base_dir = os.path.join(project_root, "gitops", "clusters", "playbox", "apps", "base")
    test_dir = os.path.join(project_root, "gitops", "clusters", "playbox", "apps", "test")

    missing = []
    for app in catalog["generators"]["base"]:
        kust = os.path.join(base_dir, app, "kustomization.yaml")
        if not os.path.isfile(kust):
            missing.append(f"base/{app}/kustomization.yaml")

    for app in catalog["generators"]["test"]:
        kust = os.path.join(test_dir, app, "kustomization.yaml")
        if not os.path.isfile(kust):
            missing.append(f"test/{app}/kustomization.yaml")

    assert missing == [], f"Missing kustomization.yaml: {missing}"
