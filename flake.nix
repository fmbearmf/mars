{
  description = "absolute CINEMA";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixpkgs-unstable";
    naersk = {
      url = "github:nix-community/naersk";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    fenix = {
      url = "github:nix-community/fenix/monthly";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      self,
      fenix,
      naersk,
      nixpkgs,
    }:
    let
      inherit (nixpkgs) lib;

      systems = [
        "aarch64-darwin"
      ];

      perSystem =
        fn:
        lib.genAttrs systems (
          system:
          let
            pkgs = import nixpkgs {
              inherit system;
            };

            target = "aarch64-unknown-none";

            toolchain =
              with fenix.packages.${system};
              combine [
                minimal.cargo
                minimal.rustc
                latest.rust-analyzer
                latest.rust-src
                targets.${target}.latest.rust-std
              ];

            pkgsCross = pkgs.pkgsCross.aarch64-embedded;
            llvmCross = pkgsCross.llvmPackages;
            stdenv = pkgs.overrideCC llvmCross.stdenv (
              llvmCross.stdenv.cc.override (_: {
                extraPackages = [ ];
                extraBuildCommands = "";
              })
            );
            mkShell = pkgsCross.mkShell.override { inherit stdenv; };
          in
          fn rec {
            inherit
              system
              mkShell
              pkgs
              pkgsCross
              stdenv
              toolchain
              ;

            naersk' = pkgs.callPackage naersk {
              cargo = toolchain;
              rustc = toolchain;
            };
          }
        );
    in
    {
      packages = perSystem (
        {
          toolchain,
          stdenv,
          naersk',
          ...
        }:
        rec {
          kernel = naersk'.buildPackage {
            src = ./.;
          };
          inherit toolchain stdenv;
          default = kernel;
        }
      );

      devShell = perSystem (
        {
          pkgs,
          mkShell,
          toolchain,
          ...
        }:
        mkShell {
          nativeBuildInputs = [
            toolchain
            pkgs.gdb
            pkgs.dtc
          ];

          buildInputs = [
          ];
        }
      );
    };
}
