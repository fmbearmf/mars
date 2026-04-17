{
  description = "Mars kernel";

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
            pkgsOVMF = import nixpkgs {
              system = "aarch64-linux";
            };

            pkgs = import nixpkgs {
              inherit system;
              overlays = [
                (self: super: {
                  inherit (pkgsOVMF) OVMF;
                })
              ];
            };

            targets = [
              "aarch64-apple-darwin"
              "aarch64-unknown-none"
              "aarch64-unknown-uefi"
            ];

            stds = builtins.map (t: fenix.packages.${system}.targets.${t}.latest.rust-std) targets;

            toolchain =
              with fenix.packages.${system};
              combine (
                [
                  minimal.cargo
                  latest.rustc
                  latest.rust-analyzer
                  latest.rust-src
                ]
                ++ stds
              );

            pkgsCross = pkgs.pkgsCross.aarch64-embedded;
            llvmCross = pkgsCross.llvmPackages;
            stdenv = pkgs.overrideCC llvmCross.stdenv (
              llvmCross.stdenv.cc.override (_: {
                extraPackages = [ ];
                extraBuildCommands = "";
              })
            );
            mkShell = pkgsCross.mkShell.override { inherit stdenv; };

            #OVMF = pkgs.callPackage ./ovmf.nix { };
            OVMF = pkgs.OVMF;
          in
          fn rec {
            inherit
              system
              mkShell
              OVMF
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
          OVMF,
          toolchain,
          ...
        }:
        mkShell {
          nativeBuildInputs = [
            toolchain
            pkgs.dtc
            pkgs.cargo-bloat
            (pkgs.callPackage ./gdb/package.nix { })
          ];

          shellHook = ''
            export OVMF_DIR="${OVMF.fd}/FV"
            export OVMF_CODE_PATH="$OVMF_DIR/AAVMF_CODE.fd"
          '';

          buildInputs = [
          ];
        }
      );
    };
}
