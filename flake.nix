{
  nixConfig.extra-substituters = [
    "https://bytecodealliance.cachix.org"
    "https://wasmcloud.cachix.org"
    "https://nixify.cachix.org"
    "https://crane.cachix.org"
    "https://nix-community.cachix.org"
  ];
  nixConfig.extra-trusted-substituters = [
    "https://bytecodealliance.cachix.org"
    "https://wasmcloud.cachix.org"
    "https://nixify.cachix.org"
    "https://crane.cachix.org"
    "https://nix-community.cachix.org"
  ];
  nixConfig.extra-trusted-public-keys = [
    "bytecodealliance.cachix.org-1:0SBgh//n2n0heh0sDFhTm+ZKBRy2sInakzFGfzN531Y="
    "wasmcloud.cachix.org-1:9gRBzsKh+x2HbVVspreFg/6iFRiD4aOcUQfXVDl3hiM="
    "nixify.cachix.org-1:95SiUQuf8Ij0hwDweALJsLtnMyv/otZamWNRp1Q1pXw="
    "crane.cachix.org-1:8Scfpmn9w+hGdXH/Q9tTLiYAE/2dnJYRJP7kl80GuRk="
    "nix-community.cachix.org-1:mB9FSh9qf2dCimDSUo8Zy7bkq5CX+/rkCWyvRCYg3Fs="
  ];

  inputs.nix-log.inputs.nixify.follows = "nixify";
  inputs.nix-log.inputs.nixlib.follows = "nixlib";
  inputs.nix-log.url = "github:rvolosatovs/nix-log";
  inputs.nixify.inputs.nix-log.follows = "nix-log";
  inputs.nixify.inputs.nixlib.follows = "nixlib";
  inputs.nixify.url = "github:rvolosatovs/nixify";
  inputs.nixlib.url = "github:nix-community/nixpkgs.lib";

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
        wit-deps ? self.packages.${pkgs.buildPlatform.system}.wit-deps,
        id,
        lock,
        manifest,
        outputHashAlgo ? "sha512",
        pkgs,
      }: let
        outputHash = (readTOML lock).${id}.${outputHashAlgo};
      in
        trace' "wit-deps.lib.tar" {
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

          name = "wit-deps-dep-${id}.tar";
          builder = pkgs.writeShellScript "wit-deps-tar" ''
            ${wit-deps}/bin/wit-deps --lock ${lock} --manifest ${manifest} tar ${id} --output $out
          '';

          preferLocalBuild = true;

          outputType = "flat";
        };

      lib.lock = {
        wit-deps ? self.packages.${pkgs.buildPlatform.system}.wit-deps,
        lock,
        manifest,
        pkgs,
      }:
        trace' "wit-deps.lib.lock" {
          inherit
            lock
            manifest
            ;
        }
        mapAttrs (id: _:
          pkgs.stdenv.mkDerivation {
            name = "wit-deps-dep-${id}";
            src = lib.tar {
              inherit
                wit-deps
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
        wit-deps ? self.packages.${pkgs.buildPlatform.system}.wit-deps,
        lock,
        manifest,
        out ? "$out",
        pkgs,
      }: let
        lock' = lib.lock {
          inherit
            wit-deps
            lock
            manifest
            pkgs
            ;
        };
      in
        trace' "wit-deps.lib.writeLockScript" {
          inherit
            lock
            manifest
            out
            ;
        }
        pkgs.writeShellScript "wit-deps-lock" (concatLines (
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

        name = "wit-deps";

        excludePaths = [
          ".github"
          ".gitignore"
          "CHANGELOG.md"
          "CODE_OF_CONDUCT.md"
          "flake.lock"
          "flake.nix"
          "garnix.yaml"
          "LICENSE"
          "ORG_CODE_OF_CONDUCT.md"
          "README.md"
        ];

        targets.wasm32-unknown-unknown = false;
        targets.wasm32-wasip1 = false;
        targets.wasm32-wasip2 = false;

        clippy.allTargets = true;
        clippy.deny = ["warnings"];
        clippy.workspace = true;

        doc.packages = ["wit-deps"];

        test.workspace = true;

        buildOverrides = {
          pkgs,
          pkgsCross ? pkgs,
          ...
        } @ args: {
          doCheck,
          preBuild ? "",
          ...
        } @ craneArgs:
          with pkgsCross; let
            lock.build-test = lib.writeLockScript ({
                inherit pkgs;

                lock = ./tests/build/wit/deps.lock;
                manifest = ./tests/build/wit/deps.toml;
                out = "./tests/build/wit/deps";
              }
              // optionalAttrs (doCheck && !(args ? pkgsCross)) {
                # for native builds, break the recursive dependency cycle by using untested wit-deps to lock deps
                wit-deps = self.packages.${pkgs.buildPlatform.system}.wit-deps.overrideAttrs (_: {
                  inherit preBuild;
                  doCheck = false;
                });
              });
          in
            optionalAttrs (craneArgs ? cargoArtifacts) {
              # only lock deps in non-dep builds
              preBuild =
                preBuild
                + ''
                  ${lock.build-test}
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
            lock = ./examples/http/wit/deps.lock;
            manifest = ./examples/http/wit/deps.toml;
          };
      }
      // {
        inherit lib;
      };
}
