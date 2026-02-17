fn main() {
    let target = std::env::var("TARGET").expect("target");
    println!("cargo:warning=Building for target: {}", target);
    println!("cargo:rustc-link-arg=-Taarch64.lds");
    println!("cargo:rerun-if-changed=aarch64.lds");
}
