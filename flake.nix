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
    hax = {
      url = "github:cryspen/hax/release-0.3.6";
      inputs.hacl-star.follows = "hacl-star";
    };
    hacl-star = {
      url = "github:hacl-star/hacl-star";
      flake = false;
    };
  };

  outputs =
    {
      self,
      fenix,
      hacl-star,
      hax,
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

            pkgsHax = hax.packages.${system};

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
                  latest.cargo
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
              pkgsHax
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
          pkgsHax,
          naersk',
          ...
        }:
        rec {
          kernel = naersk'.buildPackage {
            src = ./.;
          };
          inherit toolchain stdenv;
          inherit (pkgsHax) hax;
          default = kernel;
        }
      );

      devShell = perSystem (
        {
          pkgs,
          pkgsHax,
          mkShell,
          OVMF,
          toolchain,
          ...
        }:
        let
          OVMF_DIR = "${OVMF.fd}/FV";
          OVMF_CODE_PATH = "${OVMF_DIR}/AAVMF_CODE.fd";

          DYLD_LIBRARY_PATH = pkgs.lib.makeLibraryPath [
            pkgs.libz
            pkgsHax.rustc
          ];

          FSTAR_HOME = "${pkgsHax.fstar}";
          HAX_HOME = ./.;
        in
        mkShell {
          inherit
            OVMF_DIR
            OVMF_CODE_PATH
            DYLD_LIBRARY_PATH
            FSTAR_HOME
            HAX_HOME
            ;

          packages =
            (with pkgs; [
              dtc
              cargo-expand
              cargo-bloat
              (callPackage ./gdb/package.nix { })
            ])
            ++ [
              toolchain
              pkgsHax.hax
              pkgsHax.fstar
              pkgsHax.hax-env
            ];

          shellHook = ''
            eval $(hax-env)
          '';
        }
      );

      nixConfig = {
        extra-substituters = [
          "https://hax.cachix.org"
        ];
        extra-trusted-public-keys = [
          "hax.cachix.org-1:Oe3CtQr+8tJqpb+QNErHccOgkoA11sMm4/D4KHxOkY8="
        ];
      };
    };
}
