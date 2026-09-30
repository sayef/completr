# Contributing to completr

Thanks for your interest in completr. Bug reports, benchmarks, documentation fixes and code are all welcome.

## Before you start

- For anything beyond a small fix, open an issue first so we can agree on the approach.
- Security issues go through [SECURITY.md](SECURITY.md), not public issues.
- By contributing, you agree that your contributions are licensed under the [MIT License](LICENSE).

## Development setup

You need Rust 1.94.1 or newer (the minimum supported version, checked in CI) and, for the Python
bindings, Python 3.11+ with [maturin](https://www.maturin.rs/).

```sh
git clone https://github.com/sayef/completr && cd completr
cargo test --release -p completr --features store

python -m venv .venv && . .venv/bin/activate
pip install maturin pytest numpy
maturin develop --release
pytest crates/completr-py/tests
```

Cloud tests run only when you point them at a bucket you own:

```sh
COMPLETR_TEST_S3_URL=s3://your-bucket/completr-tests cargo test --release -p completr --features aws-credentials
```

## Checklist for a pull request

- `cargo fmt --all` and `cargo clippy --all-targets --features store -- -D warnings` are clean.
- `cargo deny check` passes (licences, advisories and sources; install with `cargo install cargo-deny --locked`).
- New behaviour has tests. Ranking changes include before-and-after results on the benchmark
  (`cargo run --release -p completr --example bench`).
- Performance changes include criterion results (`cargo bench -p completr --bench search`); CI also posts a
  base-against-head comparison to the job summary of each pull request.
- Determinism holds: the same input gives bit-identical results, whatever the segmentation or thread
  count. Break ties explicitly (by id) in any new ordering.
- Changes to the segment format bump its version, and loading rejects what it cannot validate.
- Public API changes update the Python stubs (`crates/completr-py/python/completr/__init__.pyi`), the README and the
  [changelog](CHANGELOG.md).

## Style

- Follow the surrounding code. Keep comments short: say what the code does, and put the reasoning in the
  pull request.
- Prefer returning errors to panicking on bad input, especially on anything read from storage.
- Measure before optimising, and include the numbers in the pull request.

## Commit messages

Use the imperative mood ("Add hybrid fusion options"), with a short subject line and a body that explains
why.
