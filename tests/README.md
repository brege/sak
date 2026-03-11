# Tests

## Purpose

- verify that `sak import` can write a restic-format repository from a real upstream fixture
- verify that repeated imports behave like upstream `rustic` backup tests
- verify that external tools can read what `sak` wrote

## Fixture

Taken from upstream `rustic`:
- `refs/rustic/tests/repository-fixtures/src-snapshot.tar.gz`

> [!TIP]
>
> ```bash
> mkdir refs/rustic
> git clone https://github.com/rustic-rs/rustic refs/rustic

## Commands

Run the normal test suite:

```bash
cargo test
```

Run only the import integration tests:

```bash
cargo test --test import
```

Run the manual smoke comparison against vendored `rustic`:

```bash
cargo build --release
cargo build --release --manifest-path refs/rustic/Cargo.toml
cargo test --test smoke -- --ignored --nocapture
```

Format-check before finishing:

```bash
cargo fmt --check
```
