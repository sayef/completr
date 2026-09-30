# Contributing to strato

Thanks for your interest in strato. Bug reports, benchmarks, documentation fixes and code are all welcome.

## Before you start

- For anything beyond a small fix, open an issue first so we can agree on the approach.
- Security issues go through [SECURITY.md](SECURITY.md), not public issues.
- By contributing, you agree that your contributions are licensed under the [MIT License](LICENSE).

## Development setup

You need a stable Rust toolchain and, for the Python bindings, Python 3.11+ with
[maturin](https://www.maturin.rs/).

```sh
git clone https://github.com/sayef/strato && cd strato
cargo test --release -p strato --features store

python -m venv .venv && . .venv/bin/activate
pip install maturin pytest numpy
maturin develop --release
pytest crates/strato-py/tests
```

Cloud tests run only when you point them at a bucket you own:

```sh
STRATO_TEST_S3_URL=s3://your-bucket/strato-tests cargo test --release -p strato --features aws-credentials
```

## Checklist for a pull request

- `cargo fmt --all` and `cargo clippy --all-targets --features store -- -D warnings` are clean.
- New behaviour has tests. Ranking changes include before-and-after results on the benchmark
  (`cargo run --release -p strato --example bench`).
- Determinism holds: the same input gives bit-identical results, whatever the segmentation or thread
  count. Break ties explicitly (by id) in any new ordering.
- Changes to the segment format bump its version, and loading rejects what it cannot validate.
- Public API changes update the Python stubs (`crates/strato-py/python/strato/__init__.pyi`), the README and the
  [changelog](CHANGELOG.md).

## Style

- Follow the surrounding code. Keep comments short: say what the code does, and put the reasoning in the
  pull request.
- Prefer returning errors to panicking on bad input, especially on anything read from storage.
- Measure before optimising, and include the numbers in the pull request.

## Commit messages

Use the imperative mood ("Add hybrid fusion options"), with a short subject line and a body that explains
why.
