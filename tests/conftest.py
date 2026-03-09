"""Shared pytest fixtures for template tests."""
import os
import pytest
import yaml

PROJECT_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))


@pytest.fixture
def project_root():
    return PROJECT_ROOT


@pytest.fixture
def values_full():
    path = os.path.join(PROJECT_ROOT, "tests", "fixtures", "values-full.yaml")
    with open(path) as f:
        return yaml.safe_load(f)


@pytest.fixture
def values_minimal():
    path = os.path.join(PROJECT_ROOT, "tests", "fixtures", "values-minimal.yaml")
    with open(path) as f:
        return yaml.safe_load(f)


@pytest.fixture
def values_invalid():
    path = os.path.join(PROJECT_ROOT, "tests", "fixtures", "values-invalid.yaml")
    with open(path) as f:
        return yaml.safe_load(f)
