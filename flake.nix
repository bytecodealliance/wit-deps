{
  inputs.nix-log.inputs.nixify.follows = "nixify";
  inputs.nix-log.inputs.nixlib.follows = "nixlib";
  inputs.nix-log.url = github:rvolosatovs/nix-log;
  inputs.nixify.inputs.nix-log.follows = "nix-log";
  inputs.nixify.inputs.nixlib.follows = "nixlib";
  inputs.nixify.url = github:rvolosatovs/nixify;
  inputs.nixlib.url = github:nix-community/nixpkgs.lib;

  outputs = {
    self,
    nix-log,
    nixify,
    nixlib,
    ...
  }:
    with nixlib.lib;
    with builtins;
    with nix-log.lib;
    with nixify.lib; let
      lib.tar = {
        depit ? self.packages.${pkgs.buildPlatform.system}.depit,
        id,
        lock,
        manifest,
        outputHashAlgo ? "sha512",
        pkgs,
      }: let
        outputHash = (readTOML lock).${id}.${outputHashAlgo};
      in
        trace' "depit.lib.tar" {
          inherit
            id
            lock
            manifest
            outputHash
            outputHashAlgo
            ;
        }
        pkgs.stdenv.mkDerivation {
          inherit
            outputHash
            outputHashAlgo
            ;

          name = "depit-dep-${id}.tar";
          builder = pkgs.writeShellScript "depit-tar" ''
            ${depit}/bin/depit --lock ${lock} --manifest ${manifest} tar ${id} --output $out
          '';

          preferLocalBuild = true;

          outputType = "flat";
        };

      lib.lock = {
        depit ? self.packages.${pkgs.buildPlatform.system}.depit,
        lock,
        manifest,
        pkgs,
      }:
        trace' "depit.lib.lock" {
          inherit
            lock
            manifest
            ;
        }
        mapAttrs (id: _:
          pkgs.stdenv.mkDerivation {
            name = "depit-dep-${id}";
            src = lib.tar {
              inherit
                depit
                id
                lock
                manifest
                pkgs
                ;
            };
            installPhase = ''
              mkdir -p $out
              mv * $out
            '';
            preferLocalBuild = true;
          })
        (readTOML lock);

      lib.writeLockScript = {
        depit ? self.packages.${pkgs.buildPlatform.system}.depit,
        lock,
        manifest,
        out ? "$out",
        pkgs,
      } @ args: let
        lock' = lib.lock {
          inherit
            depit
            lock
            manifest
            pkgs
            ;
        };
      in
        trace' "depit.lib.writeLockScript" {
          inherit
            lock
            manifest
            out
            ;
        }
        pkgs.writeShellScript "depit-lock" (concatLines (
          [
            ''
              mkdir -p ${out}
            ''
          ]
          ++ (
            attrValues (
              mapAttrs (id: dep: ''
                ln -s ${dep} ${out}/${id}
              '')
              lock'
            )
          )
        ));
    in
      rust.mkFlake {
        src = ./.;

        name = "depit";

        excludePaths = [
          ".github"
          ".gitignore"
          "flake.lock"
          "flake.nix"
          "garnix.yaml"
          "LICENSE.asl2"
          "LICENSE.mit"
          "README.md"
        ];

        targets.wasm32-wasi = false;
        targets.x86_64-pc-windows-gnu = false;

        test.workspace = true;

        buildOverrides = {
          pkgs,
          pkgsCross ? pkgs,
          ...
        } @ args: {
          depsBuildBuild ? [],
          doCheck,
          preCheck ? "",
          ...
        } @ craneArgs:
          with pkgsCross; let
            lock.github-build = lib.writeLockScript ({
                inherit pkgs;

                lock = ./tests/github-build/wit/deps.lock;
                manifest = ./tests/github-build/wit/deps.toml;
                out = "./tests/github-build/wit/deps";
              }
              // optionalAttrs (doCheck && !(args ? pkgsCross)) {
                # for native builds, break the recursive dependency cycle by using untested depit to lock deps
                depit = self.packages.${pkgs.buildPlatform.system}.depit.overrideAttrs (_: {
                  inherit preCheck;
                  doCheck = false;
                });
              });
          in
            {
              depsBuildBuild =
                depsBuildBuild
                ++ optionals stdenv.hostPlatform.isDarwin [
                  libiconv
                ];
            }
            # only lock deps in non-dep builds
            // optionalAttrs (doCheck && craneArgs ? cargoArtifacts) {
              preCheck =
                preCheck
                + ''
                  ${lock.github-build}
                '';
            };

        withChecks = {
          checks,
          pkgs,
          ...
        }:
          checks
          // self.lib.lock {
            inherit
              pkgs
              ;
            lock = ./examples/github/wit/deps.lock;
            manifest = ./examples/github/wit/deps.toml;
          };
      }
      // {
        inherit lib;
      };
}
