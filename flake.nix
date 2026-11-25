{
  description = "Paper Plane: a Telegram client for GNOME";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  inputs.crane.url = "github:ipetkov/crane";

  outputs =
    {
      self,
      nixpkgs,
      crane,
    }:
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
          libGL # egl.pc, for the GL video sink (gst-plugin-gtk4)
          libadwaita
          libshumate
          libvpx
          rlottie
          (tdlibFor pkgs)
        ];

      version = "0.1.0-beta.6";

      # The vendored crates, shared by the dependencies and the application:
      # Cargo tells crates apart by the path of their sources, so building
      # them from two different copies would rebuild all of them.
      cargoVendorDirFor = pkgs: (crane.mkLib pkgs).vendorCargoDeps { src = pkgs.lib.cleanSource ./.; };

      # The compiler wrapper of Nix seeds the compiler with the hash of the
      # derivation, which bindgen sees through BINDGEN_EXTRA_CLANG_ARGS. The
      # seed is left out so that the dependencies and the application build
      # with the same arguments, else bindgen and the crates using it rebuild.
      dropRandomSeed = ''
        BINDGEN_EXTRA_CLANG_ARGS=$(sed -E 's/ ?-frandom-seed=[^ ]*//g' <<< "$BINDGEN_EXTRA_CLANG_ARGS")
        export BINDGEN_EXTRA_CLANG_ARGS
      '';

      # A release build without link-time optimization, split into many
      # codegen units, for test builds: the release profile of Cargo.toml
      # compiles the application on a single core, which takes an hour for
      # aarch64 on an emulating builder. The settings apply to the
      # dependencies as well, as Cargo rebuilds crates built with others.
      quickProfile = {
        CARGO_PROFILE_RELEASE_LTO = "false";
        CARGO_PROFILE_RELEASE_CODEGEN_UNITS = "16";
      };

      # The dependencies of the application, built once by crane and reused
      # by the Meson build as long as Cargo.lock and the build environment do
      # not change, so that a change to the application itself only rebuilds
      # the application. `profile` holds the environment of the Cargo profile.
      cargoArtifactsFor =
        pkgs: profile:
        let
          craneLib = crane.mkLib pkgs;
          src = pkgs.lib.cleanSource ./.;
        in
        craneLib.buildDepsOnly (
          profile
          // {
            pname = "paper-plane";
            inherit version src;
            cargoVendorDir = cargoVendorDirFor pkgs;
            # The crates of the workspace are replaced by empty ones, except for
            # the vendored tdlib-rs: generating and compiling the TDLib API is
            # the longest part of the build, and it only changes with TDLib.
            dummySrc = craneLib.mkDummySrc {
              inherit src;
              extraDummyScript = ''
                rm -rf $out/tdlib-rs
                cp -r --no-preserve=mode ${./tdlib-rs} $out/tdlib-rs
              '';
            };
            # The same inputs as the Meson build, as the build scripts of the
            # dependencies look at the environment they make up, and would run
            # again in a different one, but cargo builds instead of Meson.
            nativeBuildInputs = nativeBuildInputsFor pkgs;
            buildInputs = buildInputsFor pkgs;
            dontUseMesonConfigure = true;
            dontUseNinjaBuild = true;
            dontUseNinjaInstall = true;
            dontUseNinjaCheck = true;
            dontWrapGApps = true;
            preBuild = dropRandomSeed;
            # The same profile as the Meson build, see src/meson.build.
            cargoCheckCommand = "true";
            cargoBuildCommand = "cargo build --release";
            doCheck = false;
          }
        );
    in
    {
      packages = forAllSystems (pkgs: {
        default = self.packages.${pkgs.stdenv.hostPlatform.system}.paper-plane;

        paper-plane-deps = cargoArtifactsFor pkgs { };

        paper-plane = pkgs.stdenv.mkDerivation {
          pname = "paper-plane";
          inherit version;

          src = pkgs.lib.cleanSource ./.;

          cargoVendorDir = cargoVendorDirFor pkgs;

          nativeBuildInputs = nativeBuildInputsFor pkgs ++ [ pkgs.zstd ];
          buildInputs = buildInputsFor pkgs;

          # The Cargo target directory of the Meson build (src/meson.build)
          # starts with the prebuilt dependencies.
          cargoArtifacts = self.packages.${pkgs.stdenv.hostPlatform.system}.paper-plane-deps;
          # The settings match the ones crane builds the dependencies with.
          preConfigure = ''
            mkdir -p .cargo build/src
            cat "$cargoVendorDir/config.toml" >> .cargo/config.toml
            export CARGO_BUILD_INCREMENTAL=false
            ${dropRandomSeed}
            zstd -d "$cargoArtifacts/target.tar.zst" --stdout | tar -x -C build/src
          '';

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
            # The tests and clippy build two more debug targets; without
            # debug info they take a fraction of the disk space.
            export CARGO_PROFILE_DEV_DEBUG=0
            GDK_BACKEND=x11 GSK_RENDERER=cairo LIBGL_ALWAYS_SOFTWARE=1 GTK_A11Y=none PAPER_PLANE_REQUIRE_DISPLAY=1 \
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

        # A quicker build to try changes on a device, see quickProfile.
        paper-plane-quick-deps = cargoArtifactsFor pkgs quickProfile;
        paper-plane-quick = self.packages.${pkgs.stdenv.hostPlatform.system}.paper-plane.overrideAttrs (
          quickProfile
          // {
            pname = "paper-plane-quick";
            cargoArtifacts = self.packages.${pkgs.stdenv.hostPlatform.system}.paper-plane-quick-deps;
            doCheck = false;
          }
        );

        # The Devel profile, which has the automation actions of the
        # end-to-end tests (src/automation.rs).
        paper-plane-devel =
          self.packages.${pkgs.stdenv.hostPlatform.system}.paper-plane.overrideAttrs
            (old: {
              pname = "paper-plane-devel";
              mesonFlags = (old.mesonFlags or [ ]) ++ [ "-Dprofile=development" ];
              doCheck = false;
            });

        # A scripted libtdjson for the end-to-end tests (tests/e2e).
        fake-tdjson = pkgs.rustPlatform.buildRustPackage {
          pname = "fake-tdjson";
          version = "0.1.0";
          src = pkgs.lib.cleanSource ./.;
          cargoRoot = "tests/e2e/fake-tdjson";
          buildAndTestSubdir = "tests/e2e/fake-tdjson";
          cargoLock.lockFile = ./tests/e2e/fake-tdjson/Cargo.lock;
          postInstall = ''
            install -Dm644 "$(find target -name libtdjson.so -path '*/release/*' | head -1)" \
              $out/lib/libtdjson.so
          '';
        };
      });

      checks = forAllSystems (
        pkgs:
        let
          packages = self.packages.${pkgs.stdenv.hostPlatform.system};
        in
        {
          # Builds the application and runs its Meson tests.
          inherit (packages) paper-plane fake-tdjson;

          # Drives the Devel build against the fake TDLib, see tests/e2e.
          e2e =
            pkgs.runCommand "paper-plane-e2e"
              {
                nativeBuildInputs = with pkgs; [
                  dbus
                  glib.bin # gdbus
                  python3
                  xvfb-run
                ];
                FONTCONFIG_FILE = pkgs.makeFontsConf {
                  fontDirectories = [
                    pkgs.cantarell-fonts
                    pkgs.dejavu_fonts
                  ];
                };
              }
              ''
                export HOME=$TMPDIR XDG_RUNTIME_DIR=$TMPDIR
                mkdir -p $out
                GDK_BACKEND=x11 GSK_RENDERER=cairo LIBGL_ALWAYS_SOFTWARE=1 \
                  dbus-run-session --config-file=${pkgs.dbus}/share/dbus-1/session.conf \
                  xvfb-run -a -s "-screen 0 1280x800x24" \
                  python3 ${./tests/e2e}/run.py \
                    --app ${packages.paper-plane-devel}/bin/paper-plane \
                    --fake-lib ${packages.fake-tdjson}/lib/libtdjson.so \
                    --artifacts $out
              '';
        }
      );

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
