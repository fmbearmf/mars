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
    verus-src = {
      url = "github:verus-lang/verus";
      flake = false;
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
      verus-src,
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

            stds = map (t: fenix.packages.${system}.targets.${t}.latest.rust-std) targets;

            toolchain =
              with fenix.packages.${system};
              combine (
                [
                  latest.cargo
                  latest.rustc
                  latest.rust-analyzer
                  latest.rust-src
                  latest.rustc-dev
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

            verus'toolchain = (
              fenix.packages.${system}.fromToolchainFile {
                file = "${verus-src}/rust-toolchain.toml";
                sha256 = "sha256-p8h3Sl/YRByZfZTAKXdsvF6xEenXKrXSVvpphmZENH4=";
              }
            );

            verus'toml = (fromTOML (builtins.readFile "${verus-src}/rust-toolchain.toml"));
            verus'toolchain'version = verus'toml.toolchain.channel;
            verus'toolchain'triple = "${verus'toolchain'version}-${pkgs.stdenv.hostPlatform.rust.rustcTargetSpec}";

            verus'env = {
              VERUS_Z3_PATH = "${pkgs.z3}/bin/z3";
              VERUS_USE_RUSTUP = "0";
              VERUS_TOOLCHAIN = verus'toolchain'triple;
            };

            verus =
              let

                rustPlatform = pkgs.makeRustPlatform {
                  cargo = verus'toolchain;
                  rustc = verus'toolchain;
                };

                kosherRustup = pkgs.writeShellScriptBin "rustup" ''
                  case "$*" in
                    *"show active-toolchain"*)
                      echo "${verus'toolchain'triple} (env override)"
                      exit 0
                      ;;
                    *)
                      echo "bad invocation: rustup $*" >&2
                      exit 1
                      ;;
                  esac
                '';
              in

              rustPlatform.buildRustPackage (
                {
                  pname = "verus"; # wow
                  version = "0-unstable";
                  src = verus-src;
                  cargoRoot = "source";

                  cargoLock = {
                    lockFile = "${verus-src}/source/Cargo.lock";
                    allowBuiltinFetchGit = true;
                  };

                  nativeBuildInputs = [
                    pkgs.makeWrapper
                    pkgs.pkg-config
                    pkgs.gitMinimal
                    kosherRustup
                  ];
                  buildInputs = [
                    pkgs.z3
                    pkgs.libz
                  ];

                  preBuildPhases = [
                    "initGitRepoPhase"
                  ];

                  initGitRepoPhase = ''
                    git init -q .
                    git config user.email "nix@build.local"
                    git config user.name "nix"
                    git add -A
                    git commit -q -m "nix build" --allow-empty
                  '';

                  buildPhase = ''
                    runHook preBuild

                    cd source
                    cargo build --release --offline
                    cargo run --release --offline -p cargo-verus -- \
                      build --release --manifest-path vstd/Cargo.toml
                    cd ..

                    runHook postBuild
                  '';

                  postPatch = ''
                    for crate in rust_verify verus; do
                        if [ -f "source/$crate/Cargo.toml" ]; then
                            substituteInPlace "source/$crate/Cargo.toml" \
                              --replace-warn 'build = "build.rs"' 'build = false' || true
                        fi
                        rm -f "source/$crate/build.rs"
                    done
                  '';

                  installPhase = ''
                    runHook preInstall

                    mkdir -p $out/opt/verus
                    cp -r source/target-verus/release/. $out/opt/verus/
                    mkdir -p $out/bin

                    runHook postInstall
                  '';

                  postInstall = ''
                    makeWrapper $out/opt/verus/verus $out/bin/verus \
                      --set VERUS_Z3_PATH "${pkgs.z3}/bin/z3" \
                      --set VERUS_USE_RUSTUP "0" \
                      --prefix DYLD_FALLBACK_LIBRARY_PATH : "${verus'toolchain}/lib" \
                      --prefix LD_LIBRARY_PATH : "${verus'toolchain}/lib"

                    makeWrapper $out/opt/verus/cargo-verus $out/bin/cargo-verus \
                      --set VERUS_Z3_PATH "${pkgs.z3}/bin/z3" \
                      --set VERUS_USE_RUSTUP "0" \
                      --prefix DYLD_FALLBACK_LIBRARY_PATH : "${verus'toolchain}/lib" \
                      --prefix LD_LIBRARY_PATH : "${verus'toolchain}/lib"
                  '';

                  doCheck = false;
                  auditable = false;
                }
                // verus'env
              );

          in
          fn rec {
            inherit
              system
              mkShell
              OVMF
              pkgs
              pkgsHax
              verus
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
          pkgs,
          pkgsHax,
          verus,
          ...
        }:
        rec {
          kernel =
            (pkgs.makeRustPlatform {
              cargo = toolchain;
              rustc = toolchain;
            }).buildRustPackage
              {
                pname = "mars-kernel";
                version = "0.0.1";

                src = ./.;

                cargoLock = {
                  lockFile = ./Cargo.lock;
                  allowBuiltinFetchGit = true;
                };

                cargoBuildFlags = [
                  "-p"
                  "kernel"
                  "-Z"
                  "build-std=core,compiler_builtins,alloc"
                  "--target"
                  "aarch64-mars-none"
                ];

                RUST_TARGET_PATH = ./target-specs;
                RUSTFLAGS = "-Z unstable-options -Z emit-stack-sizes";

                preBuildPhases = [ "vendorPhase" ];

                vendorPhase = ''
                  if [ -d "$NIX_BUILD_TOP/cargo-vendor-dir" ]; then
                      vendor_dir="$NIX_BUILD_TOP/cargo-vendor-dir"
                      if [ -L "$vendor_dir" ]; then
                          target_dir=$(readlink -f "$vendor_dir")
                          rm "$vendor_dir"
                          mkdir -p "$vendor_dir"
                          ln -sv "$target_dir"/* "$vendor_dir/"
                      fi
                      rust_sysroot="$(rustc --print sysroot)"
                      for v in "$rust_sysroot"/lib/rustlib/src/rust/library/vendor \
                        "$rust_sysroot"/lib/rustlib/src/rust/vendor; do
                            if [ -d "$v" ]; then
                                ln -sv "$v"/* "$vendor_dir/" 2>/dev/null || true
                            fi
                      done
                  fi
                '';

                installPhase = ''
                  runHook preInstall

                  mkdir -p $out/bin
                  cp target/aarch64-mars-none/release/kernel $out/bin/kernel

                  runHook postInstall
                '';

                doCheck = false;
                auditable = false;
              };

          inherit toolchain stdenv;
          inherit (pkgsHax) hax;
          inherit verus;
          default = kernel;
        }
      );

      devShell = perSystem (
        {
          pkgs,
          pkgsHax,
          verus,
          mkShell,
          OVMF,
          toolchain,
          ...
        }:
        let
          OVMF_DIR = "${OVMF.fd}/FV";
          OVMF_CODE_PATH = "${OVMF_DIR}/AAVMF_CODE.fd";

          lib_path = pkgs.lib.makeLibraryPath [
            pkgs.libz
            pkgsHax.rustc
            "${toolchain}/lib"
          ];

          RUST_TARGET_PATH = ./target-specs;

          FSTAR_HOME = "${pkgsHax.fstar}";
          HAX_HOME = ./.;
          VERUS_Z3_PATH = "${pkgs.z3}/bin/z3";
        in
        mkShell {
          inherit
            OVMF_DIR
            OVMF_CODE_PATH
            FSTAR_HOME
            RUST_TARGET_PATH
            HAX_HOME
            VERUS_Z3_PATH
            ;

          DYLD_LIBRARY_PATH = lib_path;
          LD_LIBRARY_PATH = lib_path;

          packages =
            (with pkgs; [
              dtc
              acpica-tools
              qemu
              cargo-expand
              cargo-bloat
              #(callPackage ./gdb/package.nix { })
              gdb
              z3
            ])
            ++ [
              toolchain
              pkgsHax.hax
              pkgsHax.fstar
              pkgsHax.hax-env
              verus
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
