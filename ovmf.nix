# HACK
{
  dpkg,
  fetchurl,
  stdenv,
}:

let
  version = "2025.11-3";
  url = "https://ftp.debian.org/debian/pool/main/e/edk2/qemu-efi-aarch64_${version}_all.deb";
in
stdenv.mkDerivation {
  name = "aarch64-uefi-fw";
  src = fetchurl {
    inherit url;
    hash = "sha256-scmv/fFCTp/AIs5TFJq9HfMjVAeas/HY23cwacxOMNY=";
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
