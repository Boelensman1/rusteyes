{
  description = "RustEyes Rust development environment";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    rust-overlay.url = "github:oxalica/rust-overlay";
    rust-overlay.inputs.nixpkgs.follows = "nixpkgs";
  };

  outputs =
    {
      self,
      nixpkgs,
      rust-overlay,
      ...
    }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "x86_64-darwin"
        "aarch64-darwin"
      ];

      rustVersion = "1.96.0";
      packageVersion = "0.1.0";
      overlays = [
        rust-overlay.overlays.default
      ];

      forAllSystems =
        function:
        nixpkgs.lib.genAttrs systems (
          system:
          function system (
            import nixpkgs {
              inherit system overlays;
            }
          )
        );
    in
    {
      packages = forAllSystems (
        system: pkgs:
        let
          inherit (pkgs) lib;

          rustToolchain = pkgs.rust-bin.stable.${rustVersion}.default;
          rustPlatform = pkgs.makeRustPlatform {
            cargo = rustToolchain;
            rustc = rustToolchain;
          };

          rusteyes = rustPlatform.buildRustPackage {
            pname = "rusteyes";
            version = packageVersion;
            src = lib.cleanSource ./.;
            cargoLock.lockFile = ./Cargo.lock;
            nativeBuildInputs = lib.optionals pkgs.stdenv.isLinux [
              pkgs.pkg-config
              pkgs.wrapGAppsHook3
            ];
            buildInputs = lib.optionals pkgs.stdenv.isLinux [
              pkgs.gtk3
              pkgs.libappindicator-gtk3
            ];
            # tray-icon (via libappindicator-sys) dlopens the appindicator
            # library at runtime, so it must be on the wrapped binary's
            # LD_LIBRARY_PATH rather than only present at build time.
            preFixup = lib.optionalString pkgs.stdenv.isLinux ''
              gappsWrapperArgs+=(
                --prefix LD_LIBRARY_PATH : "${lib.makeLibraryPath [ pkgs.libappindicator-gtk3 ]}"
              )
            '';
            meta = {
              description = "Minimal cross-platform Safe Eyes replacement";
              mainProgram = "rusteyes";
              platforms = [
                "x86_64-linux"
                "aarch64-linux"
                "x86_64-darwin"
                "aarch64-darwin"
              ];
            };
          };

          macosHelper = pkgs.swiftPackages.stdenv.mkDerivation {
            pname = "rusteyes-macos-helper";
            version = packageVersion;
            src = lib.cleanSource ./helpers/macos-helper;

            nativeBuildInputs = [
              pkgs.swift
              pkgs.swiftpm
            ];

            installPhase = ''
              runHook preInstall

              install -Dm755 "$(swiftpmBinPath)/rusteyes-macos-helper" \
                "$out/bin/rusteyes-macos-helper"

              runHook postInstall
            '';

            meta = {
              description = "RustEyes macOS helper process";
              mainProgram = "rusteyes-macos-helper";
              platforms = lib.platforms.darwin;
            };
          };

          macosApp = pkgs.stdenvNoCC.mkDerivation {
            pname = "rusteyes";
            version = packageVersion;

            dontUnpack = true;

            nativeBuildInputs = [
              pkgs.makeWrapper
              pkgs.darwin.sigtool
              pkgs.cctools
            ];

            installPhase = ''
              runHook preInstall

              app="$out/Applications/RustEyes.app"
              contents="$app/Contents"
              mkdir -p "$contents/MacOS" "$contents/Resources" "$out/bin"

              install -m0644 ${./package/macos/Info.plist} "$contents/Info.plist"
              install -m0644 ${./package/macos/RustEyes.icns} "$contents/Resources/RustEyes.icns"
              install -m0755 ${rusteyes}/bin/rusteyes "$contents/MacOS/rusteyes"
              install -m0755 ${macosHelper}/bin/rusteyes-macos-helper \
                "$contents/Resources/rusteyes-macos-helper"

              export CODESIGN_ALLOCATE="${pkgs.cctools}/bin/${pkgs.cctools.targetPrefix}codesign_allocate"
              codesign --force --sign - "$contents/MacOS/rusteyes"
              codesign --force --sign - "$contents/Resources/rusteyes-macos-helper"

              makeWrapper "$contents/MacOS/rusteyes" "$out/bin/rusteyes"

              runHook postInstall
            '';

            meta = {
              description = "Minimal cross-platform Safe Eyes replacement";
              mainProgram = "rusteyes";
              platforms = lib.platforms.darwin;
            };
          };
        in
        {
          default = if pkgs.stdenv.isDarwin then macosApp else rusteyes;
          inherit rusteyes;
        }
        // lib.optionalAttrs pkgs.stdenv.isDarwin {
          macos-app = macosApp;
          macos-helper = macosHelper;
        }
      );

      apps = forAllSystems (
        system: pkgs:
        {
          default = {
            type = "app";
            program = pkgs.lib.getExe self.packages.${system}.default;
          };
        }
      );

      nixosModules.default = import ./nix/nixos-module.nix { inherit self; };
      homeManagerModules.default = import ./nix/home-manager-module.nix { inherit self; };

      devShells = forAllSystems (
        system: pkgs:
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

      formatter = forAllSystems (system: pkgs: pkgs.nixfmt-rfc-style);
    };
}
