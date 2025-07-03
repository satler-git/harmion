{
  description = "A very basic flake";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs?ref=nixos-unstable";
    flake-parts.url = "github:hercules-ci/flake-parts";

    treefmt-nix = {
      url = "github:numtide/treefmt-nix";

      inputs.nixpkgs.follows = "nixpkgs";
    };

    rust-overlay = {
      url = "github:oxalica/rust-overlay";

      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    inputs@{ flake-parts, ... }:
    flake-parts.lib.mkFlake { inherit inputs; } ({
      imports = [
        inputs.treefmt-nix.flakeModule
      ];
      systems = [
        "x86_64-linux"
      ];
      perSystem =
        {
          config,
          pkgs,
          system,
          ...
        }:
        let
          rust-bin = pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml;
        in
        {

          _module.args.pkgs = import inputs.nixpkgs {
            inherit system;

            overlays = [
              inputs.rust-overlay.overlays.default
            ];
          };

          packages = {
            ci = pkgs.buildEnv {
              name = "ci";
              paths = with pkgs; [
                rust-bin

                cargo-nextest
              ];
            };
          };

          treefmt = {
            programs = {
              rustfmt = {
                enable = true;
                package = rust-bin;
              };
              nixfmt.enable = true;
              # typos.enable = true;
              taplo.enable = true;
            };

            projectRootFile = "flake.nix";
          };

          devShells.default = pkgs.mkShell {
            buildInputs = [
              rust-bin
            ];
          };
        };
    });
}
