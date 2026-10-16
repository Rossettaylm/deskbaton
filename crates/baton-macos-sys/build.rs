fn main() {
    println!("cargo:rerun-if-changed=native");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }
    cc::Build::new()
        .file("native/app.m")
        .file("native/ddc.m")
        .file("native/power.m")
        .flag("-fobjc-arc")
        .compile("deskbaton_native");
    for framework in ["AppKit", "IOKit", "CoreFoundation", "Foundation"] {
        println!("cargo:rustc-link-lib=framework={framework}");
    }
}
