use std::env;
use std::path::Path;

fn main() {
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os == "windows" {
        let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
        let res_path = Path::new(&manifest_dir).join("assets").join("icon.res");
        println!("cargo:rustc-link-arg={}", res_path.display());
        println!("cargo:rerun-if-changed=assets/icon.res");
    }
}
