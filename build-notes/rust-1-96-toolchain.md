# Rust 1.96 Toolchain

## Summary

- Switched the project Rust floor from 1.85 to 1.96.
- Added an exact Rust 1.96.0 Nix toolchain pin through `oxalica/rust-overlay`.
- Wired both the Nix development shell and package build to the same pinned
  toolchain.
- Updated existing duration literals and one scheduler divisibility check for
  Rust 1.96 Clippy compatibility.

## Decisions

- Treat latest Rust as the latest stable release, not nightly.
- Keep Cargo as the source of truth for Rust package metadata.
- Keep `nixos-unstable` for non-Rust packages and pin Rust independently so
  the compiler version does not drift with nixpkgs.

## Verification

- `nix develop --command rustc --version` reports
  `rustc 1.96.0 (ac68faa20 2026-05-25)`.
- `nix develop --command cargo --version` reports
  `cargo 1.96.0 (30a34c682 2026-05-25)`.
- `nix develop --command make check` passes.
- `nix build` passes.
