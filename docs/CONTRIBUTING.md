# Contributing

## Code Style

### Rust (scalex CLI)
- Pure functions for all generators — no side effects
- `thiserror` for error types, `clap` derive for CLI
- `cargo clippy -- -D warnings` must pass
- `cargo fmt --check` must pass

### YAML
- Indent: 2 spaces
- Quotes: Double quotes for strings with variables, domains, IPs
- Comments: Above the line, not inline
- Naming: kebab-case for K8s resource names, snake_case for config YAML keys
- Lint: `yamllint -c .yamllint.yml` must pass

### Templates
- All config reads from `credentials/` and `config/` files only
- Jinja2 templates end with `.j2`
- Generated output goes to `_generated/` (gitignored)

## Testing

```bash
# Run all tests
./tests/run-tests.sh

# Rust tests only
cd scalex-cli && cargo test
```

### TDD Workflow
1. RED: Write failing test
2. GREEN: Write minimal implementation
3. REFACTOR: Clean up
4. All tests must pass before committing

## Git Conventions
- Branch: `feat/`, `fix/`, `docs/` prefixes
- Commits: conventional commits (`feat:`, `fix:`, `docs:`, `chore:`)
- PR template: summary, test plan, verification steps
