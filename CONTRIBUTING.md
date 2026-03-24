# Contributing to RustyTalon

Thanks for your interest! This is a nights-and-weekends personal project, but PRs are welcome.

## Branching Model

```
main        ← stable releases only, protected (requires PR + passing CI)
develop     ← integration branch, target your PRs here
feature/*   ← your work
```

**All PRs should target `develop`, not `main`.**

## Getting Started

```bash
git clone https://github.com/roninjanitor/rustytalon
cd rustytalon
git checkout develop
git checkout -b feature/my-feature
```

## Before Opening a PR

Run the full quality gate locally:

```bash
cargo fmt
cargo clippy --all --benches --tests --examples --all-features
cargo test --lib
```

All three must pass. CI will enforce this, but it's faster to catch issues locally.

## Pull Request Process

1. Target `develop` (not `main`)
2. Keep PRs focused — one feature or fix per PR
3. Update `CHANGELOG.md` under `[Unreleased]` for any user-facing change
4. If your change affects a tracked capability, update `FEATURE_PARITY.md`
5. A maintainer will review and merge into `develop`; `develop` is periodically merged to `main` for releases

## Adding New Features

- **New tool?** Follow the guide in [CLAUDE.md](CLAUDE.md) under "Adding a New Tool"
- **New channel?** Follow the guide in [CLAUDE.md](CLAUDE.md) under "Adding a New Channel"
- **Database changes?** Must support both PostgreSQL and libSQL backends — see [CLAUDE.md](CLAUDE.md) under "Database"

## Code Style

- `cargo fmt` before committing
- No `.unwrap()` or `.expect()` in production code (tests are fine)
- Use `crate::` imports, not `super::`
- Comments for non-obvious logic only
