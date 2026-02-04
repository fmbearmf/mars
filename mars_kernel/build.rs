fn main() {
    println!("cargo:rustc-link-arg=-Taarch64.lds");
    println!("cargo:rerun-if-changed=aarch64.lds");
}
