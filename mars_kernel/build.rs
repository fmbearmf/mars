fn main() {
    println!("cargo:rustc-link-arg=-Tlinker.lds");
    println!("cargo:rerun-if-changed=linker.lds");
}
