//! 键鼠路由：控制权移动时让键鼠跟随。
//!
//! - [`MonitorKvmRouter`]：键鼠插在显示器上，由显示器内置 KVM 跟随输入源，软件不介入。
//! - [`software::SoftwareRouter`]：键鼠插在某台电脑上，由软件转发输入（Deskflow 或自研后端）。
//! - [`PlatformActivity`]：本机键鼠空闲时长，用于自动学习本机输入源编码。

mod activity;
mod monitor_kvm;
pub mod software;

pub use activity::PlatformActivity;
pub use monitor_kvm::MonitorKvmRouter;

use baton_core::config::InputRouting;
use baton_core::ports::InputRouter;

pub fn router_for(routing: &InputRouting) -> Box<dyn InputRouter> {
    match routing {
        InputRouting::MonitorKvm => Box::new(MonitorKvmRouter),
        InputRouting::Software { backend } => Box::new(software::SoftwareRouter::new(*backend)),
    }
}
