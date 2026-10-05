# Keep Rust builds bounded on memory-constrained developer machines.
# Override with `CARGO_BUILD_JOBS=1 just check` when the machine is under heavier load.
cargo_jobs := env_var_or_default("CARGO_BUILD_JOBS", "2")

default:
    @just --list

fmt:
    cargo fmt --check

test *args:
    cargo test -j {{cargo_jobs}} {{args}}

clippy:
    cargo clippy -j {{cargo_jobs}} --all-targets -- -D warnings

release:
    cargo build -j {{cargo_jobs}} --release

# Run sequentially so multiple Cargo builds never compete for memory.
check:
    just fmt
    just clippy
    just test
    just release
