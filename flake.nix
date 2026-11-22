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
              # design_checks/*.py
              pkgs.python3
              # Inspecting node DBs during operation/recovery.
              pkgs.sqlite
              # AntiWebUI (Wiki M0): pkg-config metadata, fonts and Wayland.
              pkgs.pkg-config
              pkgs.fontconfig
              pkgs.freetype
              pkgs.noto-fonts-cjk-sans
              pkgs.jetbrains-mono
              pkgs.wayland
              pkgs.libxkbcommon
              pkgs.vulkan-loader
              pkgs.mesa
              pkgs.libglvnd
              # X11 backend (Xvfb screenshots) and the XCB libraries it links.
              pkgs.libxcb
              pkgs.xcbutil
              pkgs.xcbutilcursor
              pkgs.xcbutilimage
              pkgs.xcbutilkeysyms
              pkgs.xcbutilrenderutil
              pkgs.xcbutilwm
              pkgs.xcbutilxrm
              pkgs.libx11
              pkgs.xkeyboardconfig
            ];
            shellHook = ''
              echo "harmion dev shell: $(rustc --version), node $(node --version), $(python3 --version)"
              # wgpu and AntiWebUI load Vulkan/EGL at runtime. NixOS puts the
              # drivers in /run/opengl-driver, outside the default loader
              # search path of a dev shell.
              export LD_LIBRARY_PATH="${pkgs.lib.makeLibraryPath [ pkgs.vulkan-loader pkgs.libglvnd pkgs.mesa ]}''${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
              export XDG_DATA_DIRS="/run/opengl-driver/share''${XDG_DATA_DIRS:+:$XDG_DATA_DIRS}"
              if [ -f /run/opengl-driver/share/glvnd/egl_vendor.d/50_mesa.json ]; then
                export __EGL_VENDOR_LIBRARY_FILENAMES="/run/opengl-driver/share/glvnd/egl_vendor.d/50_mesa.json''${__EGL_VENDOR_LIBRARY_FILENAMES:+:$__EGL_VENDOR_LIBRARY_FILENAMES}"
              fi
              export WIKI_SANS_FONT="${pkgs.noto-fonts-cjk-sans}/share/fonts/opentype/noto-cjk/NotoSansCJK-VF.otf.ttc"
              export WIKI_MONO_FONT="${pkgs.jetbrains-mono}/share/fonts/opentype/JetBrainsMono-Regular.otf"
            '';
          };
        }
      );

      formatter = eachSystem (system: nixpkgs.legacyPackages.${system}.nixfmt);
    };
}
