fn main() {
    // 前端资源在编译期嵌入，改动后需要重新编译本 crate。
    println!("cargo:rerun-if-changed=ui");
    ensure_sleep_helper_sidecar();
    tauri_build::build();
}

/// Tauri 在编桌面目标时要求 `externalBin` 对应的 triple 文件已存在。
/// 真正的助手由 pack 任务写入；本地测试放一个空占位即可。
fn ensure_sleep_helper_sidecar() {
    let triple = std::env::var("TARGET").unwrap_or_else(|_| "unknown".into());
    let file = if triple.contains("windows") {
        format!("deskbaton-sleep-helper-{triple}.exe")
    } else {
        format!("deskbaton-sleep-helper-{triple}")
    };
    let path = std::path::Path::new("binaries").join(file);
    if path.exists() {
        return;
    }
    let _ = std::fs::create_dir_all("binaries");
    let _ = std::fs::write(path, []);
}
