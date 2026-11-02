# Contributing to TursoBench

Thanks for helping improve TursoBench. This is an independent, local-first Turso
database workbench and currently a Linux preview. Please follow the
[Code of conduct](CODE_OF_CONDUCT.md).

## Issues and ideas

Search existing [issues](https://github.com/pmfleming/tursobench/issues) before
opening one. For bugs, include the commit or version, Linux distribution, steps
to reproduce, and expected and actual behavior. Use a small synthetic database
or schema; remove private data, SQL literals and credentials from logs and images.
For vulnerabilities, follow [Security](SECURITY.md) instead of posting details publicly.

Discuss substantial changes in an issue first. Keep contributions focused on the
project's Turso-only scope and explain any change to database safety or behavior.

## Development

Fork the repository, create a branch, then enter the pinned development environment:

```sh
nix develop
cargo run --locked -p tursobench-ui
```

Without Nix, use the toolchain in `rust-toolchain.toml` and install the desktop
dependencies described in the [guide](docs/guide.md). Use disposable databases
for development and testing.

Before submitting Rust changes, run the checks used by CI:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo run --locked -p engine-spike
cargo build --workspace --locked
bash scripts/desktop-smoke.sh
bash scripts/discovery-smoke.sh
bash scripts/local-project-smoke.sh
```

The smoke tests need Python, Xvfb, `timeout` and Mesa, available in the Nix
environment. For documentation-only changes, check links and rendered formatting.
See the [manual desktop checklist](docs/desktop-smoke-test.md) for UI changes.

## Pull requests

- Explain the problem, resulting behavior and how you validated the change.
- Add regression coverage for changed behavior and update relevant documentation.
- Include screenshots for visible UI changes, using synthetic data.
- Keep unrelated changes out and note any checks you could not run.

Contributions are made under the project's [MIT license](LICENSE). Preserve
third-party license notices when adding or changing dependencies or assets.
