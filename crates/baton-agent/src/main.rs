fn main() {
    if let Err(e) = baton_power::platform_power().restore_sleep() {
        eprintln!("恢复残留的睡眠设置失败：{e}");
    }
    println!(
        "DeskBaton {}（agent 组装尚未完成）",
        env!("CARGO_PKG_VERSION")
    );
}
