{
  inputs.nixify.inputs.nixlib.follows = "nixlib";
  inputs.nixify.url = github:rvolosatovs/nixify;
  inputs.nixlib.url = github:nix-community/nixpkgs.lib;

  outputs = {
    self,
    nixify,
    nixlib,
    ...
  }:
    with nixlib.lib;
    with builtins;
    with nixify.lib; let
      lib.depit = {
        lock,
        manifest,
        pkgs,
      }: let
        system = pkgs.buildPlatform.system;
        lock' = mapAttrs (
          id: {sha512, ...}:
            pkgs.stdenv.mkDerivation {
              name = "depit-dep-${id}.tar";
              builder = pkgs.writeShellScript "depit-tar" ''
                ${self.packages.${system}.depit}/bin/depit --lock ${lock} --manifest ${manifest} tar ${id} --output $out
              '';

              preferLocalBuild = true;

              outputHashAlgo = "sha512";
              outputHash = sha512;
              outputType = "flat";
            }
        ) (readTOML lock);
      in
        pkgs.stdenv.mkDerivation {
          name = "depit-deps";

          dontUnpack = true;
          installPhase =
            ''
              mkdir -p $out
            ''
            + concatLines (attrValues (
              mapAttrs (
                id: tar: ''
                  unpackFile ${tar}
                  mv wit $out/${id}
                ''
              )
              lock'
            ));
        };
    in
      rust.mkFlake {
        src = ./.;

        name = "depit";

        clippy.allTargets = true;
        clippy.workspace = true;
        test.workspace = true;

        doCheck = false;

        buildOverrides = {pkgs, ...} @ args: {
          doCheck,
          preBuild ? "",
          ...
        }: let
          deps.github-build = lib.depit {
            inherit pkgs;

            lock = ./tests/github-build/wit/deps.lock;
            manifest = ./tests/github-build/wit/deps.toml;
          };
        in
          optionalAttrs doCheck {
            preBuild =
              preBuild
              + ''
                ln -s ${deps.github-build} ./tests/github-build/wit/deps
              '';
          };

        withChecks = {
          checks,
          pkgs,
          ...
        }:
          checks
          // {
            example-github = self.lib.depit {
              inherit
                pkgs
                ;
              lock = ./examples/github/wit/deps.lock;
              manifest = ./examples/github/wit/deps.toml;
            };
          };
      }
      // {
        inherit lib;
      };
}
