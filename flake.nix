{
  description = "Discord Rich Presence for Helix (Steel plugin)";

  inputs = {
    nhx.url = "github:Ra77a3l3-jar/nhx";

    nixpkgs.follows = "nhx/nixpkgs";

    flake-compat.url = "github:NixOS/flake-compat";
    systems.url = "github:nix-systems/default";

    flake-parts = {
      url = "github:hercules-ci/flake-parts";
      inputs.nixpkgs-lib.follows = "nixpkgs";
    };

    naersk = {
      url = "github:nix-community/naersk";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = inputs:
    inputs.flake-parts.lib.mkFlake {inherit inputs;} {
      systems = import inputs.systems;
      imports = [./nix];
    };
}
