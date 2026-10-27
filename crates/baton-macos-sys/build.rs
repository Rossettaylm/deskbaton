fn main() {
    println!("cargo:rerun-if-changed=native");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }
    cc::Build::new()
        .file("native/app.m")
        .file("native/ddc.m")
        .file("native/gamma.m")
        .file("native/power.m")
        .file("native/screen.m")
        .file("native/notify.m")
        .file("native/service.m")
        .file("native/send_service.m")
        .flag("-fobjc-arc")
        .flag("-Wno-deprecated-declarations")
        .compile("deskbaton_native");
    for framework in [
        "AppKit",
        "IOKit",
        "CoreFoundation",
        "CoreGraphics",
        "Foundation",
        "ServiceManagement",
    ] {
        println!("cargo:rustc-link-lib=framework={framework}");
    }
}
