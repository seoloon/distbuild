set shell := ["bash", "-uc"]

default:
    @just --list

# Run the desktop app in Tauri dev mode
dev:
    cd apps/desktop && pnpm tauri dev

# Build the whole workspace (release) and the desktop app bundle
build:
    cargo build --workspace --release
    cd apps/desktop && pnpm tauri build

# Format Rust and frontend code
fmt:
    cargo fmt --all
    cd apps/desktop && pnpm exec prettier --write ui

# Run the Rust test suite
test:
    cargo test --workspace
