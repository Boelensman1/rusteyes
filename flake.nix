{
  description = "RustEyes Rust development environment";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    rust-overlay.url = "github:oxalica/rust-overlay";
    rust-overlay.inputs.nixpkgs.follows = "nixpkgs";
  };

  outputs =
    { nixpkgs, rust-overlay, ... }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "x86_64-darwin"
        "aarch64-darwin"
      ];

      rustVersion = "1.96.0";
      overlays = [
        rust-overlay.overlays.default
      ];

      forAllSystems =
        function:
        nixpkgs.lib.genAttrs systems (
          system:
          function (
            import nixpkgs {
              inherit system overlays;
            }
          )
        );
    in
    {
      packages = forAllSystems (
        pkgs:
        let
          rustToolchain = pkgs.rust-bin.stable.${rustVersion}.default;
          rustPlatform = pkgs.makeRustPlatform {
            cargo = rustToolchain;
            rustc = rustToolchain;
          };
        in
        {
          default = rustPlatform.buildRustPackage {
            pname = "rusteyes";
            version = "0.1.0";
            src = ./.;
            cargoLock.lockFile = ./Cargo.lock;
            nativeBuildInputs = pkgs.lib.optionals pkgs.stdenv.isLinux [
              pkgs.pkg-config
            ];
            buildInputs = pkgs.lib.optionals pkgs.stdenv.isLinux [
              pkgs.gtk3
              pkgs.libappindicator-gtk3
            ];
          };
        }
      );

      devShells = forAllSystems (
        pkgs:
        let
          rustToolchain = pkgs.rust-bin.stable.${rustVersion}.default;
        in
        {
          default = pkgs.mkShell {
            packages = with pkgs; [
              rustToolchain
              gnumake
              rust-analyzer
            ] ++ pkgs.lib.optionals pkgs.stdenv.isLinux [
              pkg-config
              gtk3
              libappindicator-gtk3
            ];
          };
        }
      );

      formatter = forAllSystems (pkgs: pkgs.nixfmt-rfc-style);
    };
}
