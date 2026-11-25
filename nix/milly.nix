# Milly IDE container for RustEyes. Lives in its own flake under nix/ so
# consumers of the root flake (package, NixOS and Home Manager modules) don't
# inherit the Milly base input. In the Milly IDE, set the repository's
# Settings -> Repository -> Nix Flake Directory to `nix`.
{ ... }:

{
  milly.project = {
    enable = true;
    name = "rusteyes";

    source.git = {
      url = "https://github.com/Boelensman1/rusteyes.git";
      ref = "main";
    };

    # The Rust toolchain comes from the root flake's dev shell, which the
    # Makefile enters by itself when cargo isn't on PATH. Build it here, with
    # the crates and a first compile, so the first `make check` is quick.
    setup.command = ''
      nix develop --command cargo build --all-targets --all-features
    '';

    startup.command = ''
      exec claude --dangerously-skip-permissions
    '';
  };

  milly.claude.enable = true;
  milly.claude.seedConfig = true;
  milly.codex.enable = true;
  milly.codex.seedConfig = true;

  milly.agentGuidance.extraMd = ''
    RustEyes in this container:
    - Rust isn't installed globally. Run Cargo through `make` (fmt, lint, test,
      check, build), which enters `nix develop` itself, or `nix develop --command cargo ...`.
    - There is no X11 display, so `make run` and anything needing a graphical
      session (overlays, tray, input blocking) can't be exercised here; rely on
      `make check` and say so when a change needs checking on a desktop.
  '';
}
