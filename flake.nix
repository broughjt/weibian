{
  description = "weibian static site generator";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    crane.url = "github:ipetkov/crane";
    fenix = {
      url = "github:nix-community/fenix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, crane, fenix, flake-utils, ... }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = nixpkgs.legacyPackages.${system};

        toolchain = fenix.packages.${system}.stable.toolchain;

        craneLib = (crane.mkLib pkgs).overrideToolchain toolchain;
        src = craneLib.cleanCargoSource ./.;
        commonArgs = {
          inherit src;
          strictDeps = true;
          nativeBuildInputs = with pkgs; [
            perl
          ];
        };
        cargoArtifacts = craneLib.buildDepsOnly commonArgs;
        weibian = craneLib.buildPackage (commonArgs // {
          inherit cargoArtifacts;
          meta.mainProgram = "wb";
        });
      in
      {
        packages.default = weibian;

        devShells.default = craneLib.devShell {
          checks = self.checks.${system};
          packages = with pkgs; [
            rust-analyzer
            cargo-edit
            cargo-machete
          ];
        };

        checks = {
          inherit weibian;

          weibian-clippy = craneLib.cargoClippy (commonArgs // {
            inherit cargoArtifacts;
            cargoClippyExtraArgs = "--all-targets -- --deny warnings";
          });

          weibian-fmt = craneLib.cargoFmt { inherit src; };
        };
      });
}
