{
  description = "Harmion — local-first personal data cluster (native node, daemon, CLI)";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    fenix = {
      url = "github:nix-community/fenix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      self,
      nixpkgs,
      fenix,
    }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
      ];
      eachSystem = nixpkgs.lib.genAttrs systems;
    in
    {
      devShells = eachSystem (
        system:
        let
          pkgs = nixpkgs.legacyPackages.${system};
          fenixPkgs = fenix.packages.${system};
          rust = fenixPkgs.combine [
            (fenixPkgs.stable.withComponents [
              "cargo"
              "rustc"
              "rustfmt"
              "clippy"
              "rust-src"
            ])
            # `cargo check -p harmion-domain -p harmion-protocol -p harmion-node
            # --target wasm32-unknown-unknown` needs this target's std.
            fenixPkgs.targets.wasm32-unknown-unknown.stable.rust-std
          ];
        in
        {
          default = pkgs.mkShell {
            packages = [
              rust
              # ring / libsqlite3-sys ("bundled") need a C toolchain in build scripts.
              pkgs.stdenv.cc
              # deploy/cloudflare/build.sh post-processes the wasm bridge.
              pkgs.wasm-bindgen-cli
              # apps/admin/build.rs shells out to npm to build the Svelte UI.
              pkgs.nodejs
              # Cloudflare Workers/DO の OAuth ログインと deploy。
              pkgs.wrangler
              # Worker alarm から native を起こす Cloudflare Tunnel。
              pkgs.cloudflared
              # design_checks/*.py
              pkgs.python3
              # Inspecting node DBs during operation/recovery.
              pkgs.sqlite
            ];
            shellHook = ''
              echo "harmion dev shell: $(rustc --version), node $(node --version), $(python3 --version)"
            '';
          };
        }
      );

      formatter = eachSystem (system: nixpkgs.legacyPackages.${system}.nixfmt);
    };
}
