{
  description = "kbots — local coding workflow orchestrator (Bun daemon, Rust CLI/TUI/GUI)";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs =
    {
      self,
      nixpkgs,
    }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
      ];
      forAllSystems =
        f:
        nixpkgs.lib.genAttrs systems (
          system:
          f (
            import nixpkgs {
              inherit system;
            }
          )
        );
      version = (nixpkgs.lib.importTOML ./cli/Cargo.toml).package.version;
    in
    {
      packages = forAllSystems (
        pkgs:
        let
          inherit (pkgs) lib stdenvNoCC;

          # The TUI's source includes the router instructions it embeds with
          # include_str!, so it must keep the cli/templates path.
          cliSrc = lib.fileset.toSource {
            root = ./cli;
            fileset = ./cli;
          };
          tuiSrc = lib.fileset.toSource {
            root = ./.;
            fileset = lib.fileset.unions [
              ./tui/control-center
              ./cli/templates/bots/router
            ];
          };
          guiSrc = lib.fileset.toSource {
            root = ./gui;
            fileset = ./gui;
          };
          daemonSrc = lib.fileset.toSource {
            root = ./.;
            fileset = lib.fileset.unions [
              ./src/backend
              ./package.json
              ./bun.lock
            ];
          };

          # Bun dependencies from the lockfile, fetched once and cached in the
          # Nix store. The lockfile is Bun's v2 format, which bun2nix does not
          # read yet, so this is the same offline install bun2nix does.
          bunDeps = stdenvNoCC.mkDerivation {
            pname = "kbots-bun-deps";
            inherit version;
            src = lib.fileset.toSource {
              root = ./.;
              fileset = lib.fileset.unions [
                ./package.json
                ./bun.lock
              ];
            };
            nativeBuildInputs = [ pkgs.bun ];
            dontConfigure = true;
            dontFixup = true;
            dontPatchELF = true;
            dontStrip = true;
            buildPhase = ''
              runHook preBuild
              export HOME="$TMPDIR/home"
              mkdir -p "$HOME"
              bun install --frozen-lockfile --ignore-scripts --no-progress
              runHook postBuild
            '';
            installPhase = ''
              runHook preInstall
              cp -r node_modules "$out"
              runHook postInstall
            '';
            outputHashMode = "recursive";
            outputHashAlgo = "sha256";
            outputHash = "sha256-mVqFdf+dcsL72myo+WN/lfN100R4cidMFVRMhi+BwLY=";
          };

          kbots-cli = pkgs.rustPlatform.buildRustPackage {
            pname = "kbots";
            inherit version;
            src = cliSrc;
            cargoLock.lockFile = ./cli/Cargo.lock;
            meta = {
              description = "kbots automation CLI (alias: kb)";
              mainProgram = "kbots";
            };
          };

          kbots-tui = pkgs.rustPlatform.buildRustPackage {
            pname = "kbots-tui";
            inherit version;
            src = tuiSrc;
            cargoRoot = "tui/control-center";
            cargoLock.lockFile = ./tui/control-center/Cargo.lock;
            # The crate is not at the source root (the router template must stay
            # at cli/templates for include_str!), so point cargo at its manifest.
            cargoBuildFlags = [
              "--manifest-path"
              "tui/control-center/Cargo.toml"
            ];
            doCheck = false;
            installPhase = ''
              binary=$(find . -type f -name kbots-tui -perm -u+x | head -n 1)
              install -Dm755 "$binary" "$out/bin/kbots-tui"
            '';
            meta = {
              description = "kbots Ratatui terminal client";
              mainProgram = "kbots-tui";
            };
          };

          kbots-gui = pkgs.rustPlatform.buildRustPackage {
            pname = "kbots-gui";
            inherit version;
            src = guiSrc;
            cargoLock.lockFile = ./gui/Cargo.lock;
            nativeBuildInputs = [ pkgs.pkg-config ];
            buildInputs = [
              pkgs.fontconfig
              pkgs.freetype
              pkgs.expat
              pkgs.bzip2
              pkgs.libpng
              pkgs.brotli
              pkgs.zlib
            ];
            postInstall = ''
              mv "$out/bin/kbots-control-center" "$out/bin/kbots-gui"
            '';
            meta = {
              description = "kbots Slint desktop operator client";
              mainProgram = "kbots-gui";
            };
          };

          kbots-daemon = stdenvNoCC.mkDerivation {
            pname = "kbots-daemon";
            inherit version;
            src = daemonSrc;
            nativeBuildInputs = [ pkgs.bun ];
            dontConfigure = true;
            dontFixup = true;
            buildPhase = ''
              runHook preBuild
              export HOME="$TMPDIR/home"
              mkdir -p "$HOME"
              cp -r ${bunDeps} node_modules
              chmod -R u+w node_modules
              bun build --compile --minify --target=bun src/backend/daemon.ts --outfile kbots-daemon
              runHook postBuild
            '';
            installPhase = ''
              runHook preInstall
              install -Dm755 kbots-daemon "$out/bin/kbots-daemon"
              runHook postInstall
            '';
            meta = {
              description = "kbots workflow daemon (local JSON-RPC over a Unix socket)";
              mainProgram = "kbots-daemon";
            };
          };

          # One command to deploy: commit -> update flake input -> HM switch.
          kbdeploy = pkgs.writeShellScriptBin "kbdeploy" (builtins.readFile ./deploy/kbdeploy.sh);

          desktop = pkgs.makeDesktopItem {
            name = "kbots";
            desktopName = "kbots Control Center";
            comment = "Manage coding workflows and agents";
            exec = "${kbots-gui}/bin/kbots-gui";
            terminal = false;
            categories = [ "Development" ];
          };

          kbots = pkgs.symlinkJoin {
            name = "kbots-${version}";
            paths = [
              kbots-cli
              kbots-tui
              kbots-gui
              kbots-daemon
              kbdeploy
              desktop
            ];
            # `kb` is the historical short alias for the automation CLI.
            postBuild = ''
              ln -s kbots "$out/bin/kb"
            '';
            meta = {
              description = "kbots — local coding workflow orchestrator";
              mainProgram = "kbots";
              platforms = lib.platforms.linux;
            };
          };
        in
        rec {
          default = kbots;
          inherit
            kbots
            kbots-cli
            kbots-tui
            kbots-gui
            kbots-daemon
            ;
        }
      );

      # Home Manager integration: install the clients and run the daemon as a
      # user service. The daemon binary is self-contained; PATH still needs the
      # external programs it drives (`hc`, `opencode`, `codex`).
      homeModules.default =
        {
          config,
          lib,
          pkgs,
          ...
        }:
        let
          cfg = config.services.kbots;
        in
        {
          options.services.kbots = {
            enable = lib.mkEnableOption "the kbots daemon and clients";
            package = lib.mkOption {
              type = lib.types.package;
              default = self.packages.${pkgs.stdenv.hostPlatform.system}.default;
              defaultText = lib.literalExpression "kbots.packages.<system>.default";
              description = "The kbots package to install.";
            };
            path = lib.mkOption {
              type = lib.types.listOf lib.types.str;
              default = [
                "${config.home.homeDirectory}/.local/bin"
                "${config.home.homeDirectory}/.opencode/bin"
                "${config.home.homeDirectory}/.bun/bin"
                "/usr/local/bin"
                "/usr/bin"
                "/bin"
              ];
              description = "PATH for the daemon: where `hc`, `opencode`, and `codex` live.";
            };
            socket = lib.mkOption {
              type = lib.types.nullOr lib.types.str;
              default = null;
              example = "/run/user/1000/kbots.sock";
              description = "Override the daemon socket (KBOTS_SOCKET). Default: $XDG_RUNTIME_DIR/kbots.sock.";
            };
          };

          config = lib.mkIf cfg.enable {
            home.packages = [ cfg.package ];

            systemd.user.services.kbots-daemon = {
              Unit = {
                Description = "kbots workflow daemon (local JSON-RPC over Unix socket)";
                After = [ "default.target" ];
              };
              Service = {
                Type = "simple";
                ExecStart = "${cfg.package}/bin/kbots-daemon";
                Restart = "on-failure";
                RestartSec = 2;
                Environment = [
                  "KBOTS_DEPLOY_VERSION=${version}"
                  "PATH=${lib.concatStringsSep ":" cfg.path}"
                ]
                ++ lib.optional (cfg.socket != null) "KBOTS_SOCKET=${cfg.socket}";
              };
              Install.WantedBy = [ "default.target" ];
            };
          };
        };

      devShells = forAllSystems (pkgs: {
        default = pkgs.mkShell {
          packages = with pkgs; [
            bun
            cargo
            rustc
            rustfmt
            clippy
            pkg-config
            fontconfig
            freetype
          ];
        };
      });

      formatter = forAllSystems (pkgs: pkgs.nixfmt-rfc-style);
    };
}
