use baton_core::config::Config;
use baton_core::coordinator::{Coordinator, Ports};
use baton_core::wake::WakeChain;

fn build_coordinator(config: Config) -> Coordinator {
    let ports = Ports {
        display: baton_display::platform_display(),
        power: baton_power::platform_power(),
        input: baton_input::router_for(&config.input),
        wake: WakeChain::from_config(&config.power.wake_chain, baton_power::available_wakers()),
        link: Box::new(baton_net::TcpPeerLink),
    };
    Coordinator::new(config, ports)
}

fn main() {
    if let Err(e) = baton_power::platform_power().restore_sleep() {
        eprintln!("恢复残留的睡眠设置失败：{e}");
    }
    let mut coordinator = build_coordinator(Config::default());
    println!(
        "DeskBaton {}（骨架版本，交棒流程尚未实现）",
        env!("CARGO_PKG_VERSION")
    );
    match coordinator.refresh_state() {
        Ok(state) => println!("当前状态：{state:?}"),
        Err(e) => println!("无法判定持棒状态：{e}"),
    }
}
