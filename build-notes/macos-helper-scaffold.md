# macos-helper-scaffold

## Summary

- Added a standalone SwiftPM executable package for the macOS helper at
  `helpers/macos-helper`.
- The helper currently compiles as a stub binary named `resteyes-macos-helper`,
  prints `hello world`, and imports the macOS frameworks planned for later
  backend work.
- Added `helpers/macos-helper/.build/debug/resteyes-macos-helper` as the real
  Make target, with `make macos-helper-build` as its convenience alias. It
  builds the helper on Darwin and skips successfully on other operating
  systems.
- The helper build clears Nix-provided Apple SDK variables before invoking
  SwiftPM so it works from inside `nix develop` with the system Xcode toolchain.
- Rust daemon behavior is unchanged; non-Linux runs still use the no-op
  backend.

## Decisions

- SwiftPM is the build source of truth for the helper scaffold.
- The package product is named `resteyes-macos-helper`; the Swift target uses
  the identifier-friendly module name `ResteyesMacOSHelper`.
- The helper build is not part of `make check` yet because the project should
  keep Rust checks working on non-macOS systems.
- `make macos-helper-build` intentionally uses the system Swift/Xcode toolchain
  on macOS instead of the Nix Apple SDK, because Swift compiler and SDK versions
  must match.
- The Make artifact target depends on the Swift package manifest and Swift
  source files, then delegates incremental decisions to SwiftPM when it runs.

## Verification

- `make macos-helper-build` initially failed in the sandbox because SwiftPM
  could not write user Swift/Clang caches.
- `make macos-helper-build` passed with approved cache access on macOS.
- `make -B helpers/macos-helper/.build/debug/resteyes-macos-helper` passed with
  approved cache access on macOS.
- `make macos-helper-build` reported the helper alias up to date after the
  artifact target existed.
- `nix develop --command make macos-helper-build` passed after clearing the Nix
  Apple SDK environment for SwiftPM.
- `make check` initially failed in the sandbox because the Nix daemon socket was
  unavailable.
- `make check` passed with approved Nix daemon access.

## Follow-up

- Define local IPC between the Rust daemon and the helper in
  `macos-helper-ipc`.
