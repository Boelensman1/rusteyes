{
  description = "RustEyes Milly container image";

  inputs.milly-base.url = "git+https://github.com/wtflegal/milly2.git?dir=nix";

  outputs =
    { self, milly-base, ... }:
    {
      nixosConfigurations.container = milly-base.lib.mkMillyContainer {
        # The repository root; `self` alone would be this nix/ subdirectory.
        src = self.sourceInfo;
        module = ./milly.nix;
      };
    };
}
