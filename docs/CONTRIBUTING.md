# Contributing

## Code Style

### YAML
- Indent: 2 spaces
- Quotes: Double quotes for strings with variables, domains, IPs
- Comments: Above the line, not inline
- Naming: kebab-case for K8s resource names, snake_case for values.yaml keys
- Lint: `yamllint -c .yamllint.yml` must pass

### Shell Scripts
- Shebang: `#!/usr/bin/env bash`
- Set flags: `set -euo pipefail`
- Functions: snake_case, prefixed with module name
- Logging: Use `log_info`, `log_warn`, `log_error` from `lib/common.sh`
- Idempotency: Check state before acting
- Helm: Always `helm upgrade --install --atomic --wait --timeout 5m`
- kubectl: Always `kubectl apply` (never `create` in scripts)

### Templates
- All templates read from `values.yaml` only
- Jinja2 templates end with `.j2`, Go templates with `.tpl`
- Generated output goes to `_generated/` (gitignored)
- Every template has a corresponding test

## Testing

```bash
# Run all tests
./tests/run-tests.sh

# Individual suites
bats tests/bats/*.bats
pytest tests/ -v
shellcheck playbox lib/*.sh
```

### TDD Workflow
1. RED: Write failing test
2. GREEN: Write minimal implementation
3. REFACTOR: Clean up
4. All tests must pass before merging

## Git Conventions
- Branch: `feat/`, `fix/`, `docs/` prefixes
- Commits: conventional commits (`feat:`, `fix:`, `docs:`, `chore:`)
- PR template: summary, test plan, verification steps
