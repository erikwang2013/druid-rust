# Contributing to Druid-Rust

## Development Setup

```bash
git clone https://github.com/alibaba/druid-rust.git
cd druid-rust
cargo build
```

## Before Submitting

```bash
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test --workspace
```

## Project Structure

10 crates in a Cargo workspace. See [README.md](README.md#项目结构) for the full layout.

## Commit Style

- Use Chinese or English commit messages
- Prefix with the affected crate: `druid-pool: fix waiting_count leak`
- Keep commits focused — one logical change per commit

## License

Apache 2.0. All contributions are under this license.
