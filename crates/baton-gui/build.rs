fn main() {
    // 前端资源在编译期嵌入，改动后需要重新编译本 crate。
    println!("cargo:rerun-if-changed=ui");
    tauri_build::build();
}
