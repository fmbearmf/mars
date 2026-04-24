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
    creusot = {
      url = "github:creusot-rs/creusot";
      inputs = {
        nixpkgs.follows = "nixpkgs";
      };
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
              config = {
                allowUnfreePredicate =
                  pkg:
                  builtins.elem (nixpkgs.lib.getName pkg) [
                    "alt-ergo"
                    "ocaml5.4.1-alt-ergo-2.6.3"
                  ];
              };
            };

            pkgsCreusot = creusot.packages.${system};

            mkWhy3 =
              {
              }:
              let
                solvers =
                  let
                    alt-ergo = (
                      pkgs.alt-ergo.overrideAttrs rec {
                        version = "2.6.2";
                        src = pkgs.fetchurl {
                          url = "https://github.com/OCamlPro/alt-ergo/releases/download/v${version}/alt-ergo-${version}.tbz";
                          hash = "sha256-OeLJEop9HonzMuMaJxbzWfO54akl/oHxH6SnSbXSTYI=";
                        };
                      }
                    );
                    free-solvers =
                      let
                        why3 = (
                          pkgs.stdenv.mkDerivation rec {
                            inherit (pkgs.why3)
                              installTargets
                              meta
                              pname
                              postInstall
                              outputs
                              ;

                            version = "2c0f2992af85f82f3eda0f158dcf10e62e0db875";
                            src = pkgs.fetchFromGitLab {
                              domain = "gitlab.inria.fr";
                              owner = "why3";
                              repo = "why3";
                              rev = version;
                              hash = "sha256-mUHT6QOIhHyxqyYaEAXec15Bh0LpLXs2aJc1AldPaxo=";
                            };

                            nativeBuildInputs =
                              with pkgs;
                              with ocamlPackages;
                              [
                                findlib
                                menhir
                                ocaml
                                wrapGAppsHook3
                                autoreconfHook
                              ];

                            buildInputs =
                              with pkgs;
                              with ocamlPackages;
                              [
                                autoreconfHook
                                lablgtk3-sourceview3
                                ocamlgraph
                              ];

                            propagatedBuildInputs =
                              with pkgs;
                              with ocamlPackages;
                              [
                                camlzip
                                menhirLib
                                re
                                sexplib
                                zarith
                              ];

                            configureFlags = [
                              "--enable-ide"
                              "--enable-verbose-make"

                              "--disable-js-of-ocaml"
                              "--disable-web-ide"
                              "--disable-coq-libs"
                              "--disable-isabelle-libs"
                              "--disable-pvs-libs"

                              "--disable-doc"
                              "--disable-pdf-doc"

                              "--disable-emacs-compilation"
                              "--disable-java"
                            ];
                          }
                        );
                        why3find = (
                          pkgs.ocamlPackages.buildDunePackage rec {
                            pname = "why3find";

                            version = "eab37557d3e24e1913a3c4f44bc5528ef497c6c9";
                            src = pkgs.fetchFromGitHub {
                              owner = "creusot-rs";
                              repo = "why3find";
                              rev = version;
                              hash = "sha256-Z0By+s+aFtm+fzeec7GadgJdAMxRTUd1fsQSYr3i4ME=";
                            };

                            nativeBuildInputs =
                              with pkgs;
                              (lib.optionals stdenv.buildPlatform.isDarwin [
                                darwin.sigtool
                              ]);

                            buildInputs =
                              (
                                with pkgs;
                                with ocamlPackages;
                                [
                                  dune-site
                                  terminal_size
                                  yojson
                                  zeromq
                                  zmq
                                ]
                              )
                              ++ [
                                why3
                              ];

                          }
                        );
                      in
                      [
                        pkgs.cvc5
                        pkgs.cvc4
                        why3
                        why3find
                        pkgs.z3
                      ];
                  in
                  [ alt-ergo ] ++ free-solvers;

                why3json = pkgs.writeTextFile {
                  destination = "/why3find.json";
                  name = "why3find.json";
                  text = builtins.readFile (creusot + /why3find.json);
                };
              in
              pkgs.symlinkJoin {
                name = "creusot-why3";
                paths = solvers ++ [ why3json ];
                postBuild = "ln -s $out $out/creusot";

                passthru = builtins.listToAttrs (
                  map (drv: {
                    name = drv.pname;
                    value = drv;
                  }) solvers
                );
              };

            why3Framework = mkWhy3 { };

            crossTargets = [
              #"aarch64-apple-darwin"
              "aarch64-unknown-none"
              "aarch64-unknown-uefi"
            ];

            stds = map (t: fenix.packages.${system}.targets.${t}.latest.rust-std) crossTargets;

            toolchain =
              with fenix.packages.${system};
              combine (
                [
                  (fenix.packages.${system}.latest.withComponents [
                    "cargo"
                    "rustc"
                    "rust-analyzer"
                    "rust-src"
                  ])
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
              pkgsCreusot
              stdenv
              toolchain
              why3Framework
              ;

            naersk' = pkgs.callPackage naersk {
              cargo = toolchain;
              rustc = toolchain;
            };
          }
        );
    in
    rec {
      packages = perSystem (
        {
          toolchain,
          pkgs,
          pkgsCreusot,
          why3Framework,
          stdenv,
          naersk',
          ...
        }:
        rec {
          kernel = naersk'.buildPackage {
            src = ./.;
          };
          creusotEnv = pkgs.buildEnv {
            name = "creusot-env";
            paths = [
              toolchain
              why3Framework
            ]
            ++ [
              pkgsCreusot.creusot
              pkgsCreusot.prelude
            ];

            nativeBuildInputs = [ pkgs.makeWrapper ];

            postBuild = ''
              ls -lah $out/bin/
              wrapProgram $out/bin/cargo-creusot \
                --set CREUSOT_DATA_HOME "$out"
            '';
          };
          inherit toolchain stdenv pkgsCreusot;
          default = kernel;
        }
      );

      devShells = perSystem (
        {
          pkgs,
          pkgsCreusot,
          mkShell,
          why3Framework,
          OVMF,
          toolchain,
          ...
        }:
        {
          legacy = mkShell {
            nativeBuildInputs = [
              toolchain
              pkgs.dtc
              pkgs.cargo-bloat
              (pkgs.callPackage ./nix/gdb/package.nix { })
            ];

            shellHook = ''
              export OVMF_DIR="${OVMF.fd}/FV"
              export OVMF_CODE_PATH="$OVMF_DIR/AAVMF_CODE.fd"
            '';
          };

          default =
            let
              ovmf_dir = "${OVMF.fd}/FV";
              ovmf_code = "${ovmf_dir}/AAVMF_CODE.fd";
            in
            mkShell {
              inputsFrom = [ pkgsCreusot.creusot ];
              packages = [
                toolchain
                packages.${pkgs.stdenv.buildPlatform.system}.creusotEnv
                (pkgs.callPackage ./nix/gdb/package.nix { })
              ];

              CREUSOT_DATA_HOME = why3Framework;
              LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath [ toolchain ];
              DYLD_FALLBACK_LIBRARY_PATH = pkgs.lib.makeLibraryPath [ toolchain ];
              OVMF_DIR = ovmf_dir;
              OVMF_CODE_PATH = ovmf_code;

              #shellHook = ''
              #  export OVMF_DIR="${OVMF.fd}/FV"
              #  export OVMF_CODE_PATH="$OVMF_DIR/AAVMF_CODE.fd"
              #'';
            };
        }
      );
    };
}
