{inputs, ...}: {
  imports = [
    ./shell.nix
    ./package.nix
    ./checks.nix
    ./module.nix
  ];

  perSystem = {system, ...}: {
    _module.args = let
      pkgs = import inputs.nixpkgs {
        inherit system;
        overlays = [inputs.naersk.inputs.fenix.overlays.default];
      };

      toolchain = pkgs.fenix.fromToolchainFile {
        file = ../rust-toolchain.toml;
        sha256 = "sha256-p8h3Sl/YRByZfZTAKXdsvF6xEenXKrXSVvpphmZENH4=";
      };

      naersk = pkgs.callPackage inputs.naersk {
        cargo = toolchain;
        rustc = toolchain;
      };
    in {
      inherit pkgs toolchain naersk;
    };
  };
}
