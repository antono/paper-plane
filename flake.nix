{
  description = "Paper Plane: a Telegram client for GNOME";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs =
    { self, nixpkgs }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
      ];
      forAllSystems = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});

      # The TDLib API is generated from tdlib-rs/tl/api_<version>.tl of the
      # TDLib found through pkg-config, so TDLib has to be a version whose
      # schema is in the tree. Bump together with tdlib-rs/tl/ and meson.build.
      tdlibFor =
        pkgs:
        pkgs.tdlib.overrideAttrs {
          version = "1.8.65";
          src = pkgs.fetchFromGitHub {
            owner = "tdlib";
            repo = "td";
            rev = "a8f21f5230172634becc1739050ef23ecd6ea291";
            hash = "sha256-cCNXRyeu6ZMf/0oxipPPUyniGuLzvWFLWCvklPIYvzk=";
          };
        };

      nativeBuildInputsFor =
        pkgs: with pkgs; [
          appstream
          blueprint-compiler
          cargo
          desktop-file-utils
          gettext
          glib
          meson
          ninja
          pkg-config
          rustPlatform.bindgenHook # libvpx-native-sys
          rustc
          wrapGAppsHook4
        ];

      buildInputsFor =
        pkgs: with pkgs; [
          gst_all_1.gst-libav
          gst_all_1.gst-plugins-bad
          gst_all_1.gst-plugins-base
          gst_all_1.gst-plugins-good
          gst_all_1.gstreamer
          gtk4
          libadwaita
          libshumate
          libvpx
          rlottie
          (tdlibFor pkgs)
        ];
    in
    {
      packages = forAllSystems (pkgs: {
        default = self.packages.${pkgs.stdenv.hostPlatform.system}.paper-plane;

        paper-plane = pkgs.stdenv.mkDerivation {
          pname = "paper-plane";
          version = "0.1.0-beta.6";

          src = pkgs.lib.cleanSource ./.;

          cargoDeps = pkgs.rustPlatform.importCargoLock {
            lockFile = ./Cargo.lock;
            allowBuiltinFetchGit = true; # origami and gtk-rlottie-rs
          };

          nativeBuildInputs = nativeBuildInputsFor pkgs ++ [ pkgs.rustPlatform.cargoSetupHook ];
          buildInputs = buildInputsFor pkgs;

          # Telegram API credentials default to the limited test ones, see
          # meson_options.txt. Override with
          #   mesonFlags = [ "-Dtg_api_id=…" "-Dtg_api_hash=…" ];
          mesonBuildType = "release";

          # meson test: clippy, the unit tests and the GTK checks, which need
          # a display (Xvfb, software rendering).
          doCheck = true;
          nativeCheckInputs = with pkgs; [
            clippy
            dbus
            python3
            xvfb-run
          ];
          checkPhase = ''
            runHook preCheck
            export HOME=$TMPDIR XDG_RUNTIME_DIR=$TMPDIR
            GDK_BACKEND=x11 GSK_RENDERER=cairo LIBGL_ALWAYS_SOFTWARE=1 GTK_A11Y=none \
              dbus-run-session --config-file=${pkgs.dbus}/share/dbus-1/session.conf \
              xvfb-run -a meson test --print-errorlogs --num-processes 1
            runHook postCheck
          '';

          meta = {
            description = "Chat over Telegram on a modern and elegant client";
            homepage = "https://github.com/vitlav/paper-plane";
            license = pkgs.lib.licenses.gpl3Plus;
            mainProgram = "paper-plane";
            platforms = pkgs.lib.platforms.linux;
          };
        };
      });

      devShells = forAllSystems (pkgs: {
        default = pkgs.mkShell {
          nativeBuildInputs = nativeBuildInputsFor pkgs ++ [
            pkgs.clippy
            pkgs.rust-analyzer
            pkgs.rustfmt
          ];
          buildInputs = buildInputsFor pkgs;
        };
      });

      formatter = forAllSystems (pkgs: pkgs.nixfmt);
    };
}
