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
          # Androidノードのビルド用。SDK/NDKのライセンスはNix式で明示的に受理する。
          # `nix develop .#android` からのみ評価される（既定シェルには含めない）。
          androidPkgs = import nixpkgs {
            inherit system;
            config.allowUnfree = true;
          };
          androidSdk = (androidPkgs.androidenv.override { licenseAccepted = true; }).composeAndroidPackages {
            platformVersions = [ "37.2" ];
            buildToolsVersions = [ "37.0.0" ];
            includeNDK = true;
            ndkVersions = [ "30.0.16248370" ];
            includeSources = false;
            includeEmulator = true;
            includeSystemImages = true;
            systemImageTypes = [ "google_apis" ];
            abiVersions = [ "x86_64" ];
            cmdLineToolsVersion = "latest";
          };
          androidRust = fenixPkgs.combine [
            (fenixPkgs.stable.withComponents [
              "cargo"
              "rustc"
              "rustfmt"
              "clippy"
              "rust-src"
            ])
            fenixPkgs.targets.aarch64-linux-android.stable.rust-std
            fenixPkgs.targets.armv7-linux-androideabi.stable.rust-std
            fenixPkgs.targets.x86_64-linux-android.stable.rust-std
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
              # ring's wasm C sources need an unwrapped clang; the wrapped
              # compiler injects x86 hardening flags that wasm rejects.
              pkgs.llvmPackages.clang-unwrapped
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
              export HARMION_WASM_CC="${pkgs.llvmPackages.clang-unwrapped}/bin/clang"
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
          android = androidPkgs.mkShell {
            packages = [
              androidRust
              # ring / libsqlite3-sys / NDK linkers need a C toolchain.
              androidPkgs.stdenv.cc
              androidPkgs.jdk
              androidPkgs.gradle_9
              androidSdk.androidsdk
              androidPkgs.android-tools
              androidPkgs.pkg-config
              androidPkgs.sqlite
              androidPkgs.nodejs
              androidPkgs.python3
              # Signing material is fetched from 1Password during release.
              androidPkgs._1password-cli
              androidPkgs.jq
            ];
            shellHook = ''
              export ANDROID_HOME="${androidSdk.androidsdk}/libexec/android-sdk"
              export ANDROID_SDK_ROOT="$ANDROID_HOME"
              export ANDROID_NDK_ROOT="$ANDROID_HOME/ndk/30.0.16248370"
              export JAVA_HOME="${androidPkgs.jdk}"
              # The sandbox may make ~/.android and ~/.gradle read-only; keep
              # tool caches inside the checkout.
              export ANDROID_USER_HOME="''${ANDROID_USER_HOME:-$PWD/target/android-home}"
              export GRADLE_USER_HOME="''${GRADLE_USER_HOME:-$PWD/target/gradle-home}"
              export CARGO_TARGET_DIR="''${CARGO_TARGET_DIR:-$PWD/target}"
              echo "harmion android shell: $(rustc --version), Java $(${androidPkgs.jdk}/bin/java -version 2>&1 | head -1), SDK $ANDROID_HOME"
            '';
          };
        }
      );

      packages = eachSystem (
        system:
        let
          pkgs = nixpkgs.legacyPackages.${system};
          lib = pkgs.lib;
          fenixPkgs = fenix.packages.${system};
          rust = fenixPkgs.combine [
            (fenixPkgs.stable.withComponents [
              "cargo"
              "rustc"
            ])
          ];
          rustPlatform = pkgs.makeRustPlatform {
            cargo = rust;
            rustc = rust;
          };

          admin-ui = pkgs.buildNpmPackage {
            pname = "harmion-admin-ui";
            version = "0.1.0";
            src = ./apps/admin/ui;
            npmDepsHash = "sha256-1nM8RFpbtsW/hNJdHosZr0Kl9dhG3o9PmLafy5Bh/x0=";
            npmBuildScript = "build";
            installPhase = ''
              runHook preInstall
              mkdir -p $out
              cp -r dist/. $out/
              runHook postInstall
            '';
          };

          mkHarmion =
            {
              pname,
              package,
              bins,
              extraInstall ? "",
              extraAttrs ? { },
            }:
            rustPlatform.buildRustPackage (
              {
                inherit pname;
                version = "0.1.0";
                src = self;
                cargoLock = {
                  lockFile = ./Cargo.lock;
                  # AntiWebUI is a git dependency used by the Wiki GUI. The
                  # workspace lockfile is vendored as a whole, so every crate
                  # from that repository needs the same source hash.
                  outputHashes = {
                    "anti-webui-0.1.0" = "sha256-PLR9jCkBfWKA69L4qtDE1qFHkVPU1UgFRk9OC3Oqsqo=";
                    "anti-webui-app-0.1.0" = "sha256-PLR9jCkBfWKA69L4qtDE1qFHkVPU1UgFRk9OC3Oqsqo=";
                    "anti-webui-code-0.1.0" = "sha256-PLR9jCkBfWKA69L4qtDE1qFHkVPU1UgFRk9OC3Oqsqo=";
                    "anti-webui-core-0.1.0" = "sha256-PLR9jCkBfWKA69L4qtDE1qFHkVPU1UgFRk9OC3Oqsqo=";
                    "anti-webui-internal-0.1.0" = "sha256-PLR9jCkBfWKA69L4qtDE1qFHkVPU1UgFRk9OC3Oqsqo=";
                    "anti-webui-macros-0.1.0" = "sha256-PLR9jCkBfWKA69L4qtDE1qFHkVPU1UgFRk9OC3Oqsqo=";
                    "anti-webui-markdown-0.1.0" = "sha256-PLR9jCkBfWKA69L4qtDE1qFHkVPU1UgFRk9OC3Oqsqo=";
                    "anti-webui-style-0.1.0" = "sha256-PLR9jCkBfWKA69L4qtDE1qFHkVPU1UgFRk9OC3Oqsqo=";
                    "anti-webui-symbols-outlined-0.1.0" = "sha256-PLR9jCkBfWKA69L4qtDE1qFHkVPU1UgFRk9OC3Oqsqo=";
                    "anti-webui-wayland-0.1.0" = "sha256-PLR9jCkBfWKA69L4qtDE1qFHkVPU1UgFRk9OC3Oqsqo=";
                    "anti-webui-wgpu-0.1.0" = "sha256-PLR9jCkBfWKA69L4qtDE1qFHkVPU1UgFRk9OC3Oqsqo=";
                    "anti-webui-widgets-0.1.0" = "sha256-PLR9jCkBfWKA69L4qtDE1qFHkVPU1UgFRk9OC3Oqsqo=";
                    "anti-webui-x11-0.1.0" = "sha256-PLR9jCkBfWKA69L4qtDE1qFHkVPU1UgFRk9OC3Oqsqo=";
                  };
                };
                cargoBuildFlags = [ "--package" package ];
                doCheck = false;
                nativeBuildInputs = [ pkgs.pkg-config ];
                buildInputs = [ pkgs.sqlite ];
                installPhase = ''
                  runHook preInstall
                  mkdir -p $out/bin
                  ${lib.concatMapStrings (bin: ''
                    binpath="$(find target -type f -path "*/release/${bin}" -print -quit)"
                    [ -n "$binpath" ] || { echo "missing target binary: ${bin}" >&2; exit 1; }
                    install -m755 "$binpath" $out/bin/${bin}
                  '') bins}
                  ${extraInstall}
                  runHook postInstall
                '';
              }
              // extraAttrs
            );

          harmiond = mkHarmion {
            pname = "harmiond";
            package = "harmiond";
            bins = [ "harmiond" ];
            extraInstall = ''
              install -Dm644 ${./deploy/systemd}/harmion@.service $out/share/systemd/user/harmion@.service
              install -Dm755 ${./deploy/backup/harmion-backup.sh} $out/share/harmion/harmion-backup.sh
            '';
          };

          harmion = mkHarmion {
            pname = "harmion";
            package = "harmion";
            bins = [ "harmion" ];
          };

          harmion-admin = mkHarmion {
            pname = "harmion-admin";
            package = "harmion-admin";
            bins = [ "harmion-admin" ];
            extraAttrs = {
              HARMION_ADMIN_UI_DIST = "${admin-ui}";
            };
          };

          harmion-update = pkgs.writeShellScriptBin "harmion-update" (
            builtins.readFile ./scripts/harmion-update
          );
        in
        {
          inherit
            admin-ui
            harmion
            harmion-admin
            harmion-update
            harmiond
            ;

          default = pkgs.buildEnv {
            name = "harmion";
            paths = [
              harmiond
              harmion
              harmion-admin
              harmion-update
            ];
          };
        }
      );

      formatter = eachSystem (system: nixpkgs.legacyPackages.${system}.nixfmt);
    };
}
