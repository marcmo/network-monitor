fn main() {
    println!("cargo:rerun-if-changed=native/location.m");
    println!("cargo:rerun-if-changed=native/Info.plist");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }

    cc::Build::new()
        .file("native/location.m")
        .flag("-fobjc-arc")
        .warnings(true)
        .extra_warnings(true)
        .compile("network_monitor_location");
    println!("cargo:rustc-link-lib=framework=CoreLocation");
    println!("cargo:rustc-link-lib=framework=Foundation");
    println!("cargo:rustc-link-lib=framework=CoreFoundation");

    if let Some(manifest_dir) = std::env::var_os("CARGO_MANIFEST_DIR") {
        let plist = std::path::PathBuf::from(manifest_dir).join("native/Info.plist");
        println!(
            "cargo:rustc-link-arg=-Wl,-sectcreate,__TEXT,__info_plist,{}",
            plist.display()
        );
    }
}
