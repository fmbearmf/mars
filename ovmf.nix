# AAVMF in Nixpkgs is bugged currently.
{
  dpkg,
  fetchurl,
  stdenv,
}:

let
  version = "2025.11-4";
  url = "https://ftp.debian.org/debian/pool/main/e/edk2/qemu-efi-aarch64_${version}_all.deb";
in
stdenv.mkDerivation {
  name = "aarch64-uefi-fw";
  src = fetchurl {
    inherit url;
    hash = "sha256-725DvoCrSM+204KIbDbhBWKdICi5PgQ7o6hmwEa/AoM=";
  };

  nativeBuildInputs = [ dpkg ];
  unpackPhase = ''
    dpkg-deb -x $src .
  '';

  installPhase = ''
    mkdir -p $out/share/qemu
    cp usr/share/AAVMF/*.fd $out/share/qemu/
  '';
}
