use std::process::ExitCode;
use std::sync::mpsc;
use std::time::Duration;

use baton_core::ids::{DisplayId, InputCode, MacAddr};
use baton_core::ports::{DisplayControl, DisplayInfo};
use baton_core::{Error, Result};
use baton_power::sleep_helper;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "deskbaton-cli", version, about = "DeskBaton 调试工具")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// 列出支持 DDC/CI 的显示器
    Displays,
    /// 读取当前输入源
    GetInput {
        /// 显示器型号（子串匹配），默认要求只有一台
        #[arg(long, default_value = "auto")]
        display: String,
    },
    /// 切换输入源，编码支持十进制或 0x 开头的十六进制
    SetInput {
        code: InputCode,
        #[arg(long, default_value = "auto")]
        display: String,
    },
    /// 发送 Wake-on-LAN 魔术包
    Wol { mac: MacAddr },
    /// 在指定秒数内阻止本机睡眠（含合盖睡眠）
    StayAwake { secs: u64 },
    /// 恢复上次异常退出残留的睡眠设置
    RestoreSleep,
    /// 管理免密码执行 pmset 的 sudoers 规则（macOS 合盖切换时阻止睡眠需要它）
    SleepHelper {
        #[command(subcommand)]
        action: HelperAction,
    },
}

#[derive(Subcommand)]
enum HelperAction {
    /// 为当前用户安装规则，会提示输入管理员密码
    Install,
    /// 删除规则
    Uninstall,
    /// 查看规则是否生效
    Status,
}

fn resolve(display: &dyn DisplayControl, matcher: &str) -> Result<DisplayId> {
    Ok(DisplayInfo::select(display.list()?, matcher)?.id)
}

fn run(command: Command) -> Result<()> {
    match command {
        Command::Displays => {
            for d in baton_display::platform_display().list()? {
                let mode = match baton_display::screen_mode(&d.id) {
                    Ok(m) => format!(
                        "{}×{}（渲染 {}×{}）{}",
                        m.width,
                        m.height,
                        m.pixel_width,
                        m.pixel_height,
                        m.refresh_hz
                            .map(|hz| format!(" {hz:.0}Hz"))
                            .unwrap_or_default()
                    ),
                    Err(e) => e.to_string(),
                };
                println!(
                    "{}\t{} {}\t{}\t{mode}",
                    d.id.0,
                    d.vendor,
                    d.model,
                    d.serial.unwrap_or_default()
                );
            }
        }
        Command::GetInput { display } => {
            let ddc = baton_display::platform_display();
            let id = resolve(ddc.as_ref(), &display)?;
            println!("{}", ddc.get_input(&id)?);
        }
        Command::SetInput { code, display } => {
            let ddc = baton_display::platform_display();
            let id = resolve(ddc.as_ref(), &display)?;
            ddc.set_input(&id, code)?;
            println!("已切换到 {code}");
        }
        Command::Wol { mac } => {
            baton_power::wake::WolWaker::default()
                .send(mac)
                .map_err(|e| baton_core::Error::Wake(e.to_string()))?;
            println!("已向 {mac} 发送魔术包");
        }
        Command::StayAwake { secs } => {
            if cfg!(target_os = "macos") && !sleep_helper::installed() {
                eprintln!(
                    "警告：未安装 sleep-helper，只能阻止空闲睡眠，合盖的 Mac 切回时仍会睡眠。先运行 deskbaton-cli sleep-helper install"
                );
            }
            let (stop, stopped) = mpsc::channel();
            ctrlc::set_handler(move || {
                let _ = stop.send(());
            })
            .map_err(|e| Error::Config(format!("无法注册退出信号处理：{e}")))?;
            let duration = Duration::from_secs(secs);
            let guard = baton_power::platform_power().stay_awake(duration)?;
            println!("保持唤醒 {secs} 秒，期间可合盖测试；按 Ctrl+C 提前结束");
            if stopped.recv_timeout(duration).is_ok() {
                println!("收到退出信号，恢复睡眠设置");
            }
            drop(guard);
        }
        Command::RestoreSleep => {
            baton_power::platform_power().restore_sleep()?;
            println!("睡眠设置已恢复");
        }
        Command::SleepHelper { action } => match action {
            HelperAction::Install => {
                sleep_helper::install()?;
                println!("已写入 {}", sleep_helper::SUDOERS_PATH);
            }
            HelperAction::Uninstall => {
                sleep_helper::uninstall()?;
                println!("已删除 {}", sleep_helper::SUDOERS_PATH);
            }
            HelperAction::Status => {
                if sleep_helper::installed() {
                    println!("已生效：{}", sleep_helper::SUDOERS_PATH);
                } else {
                    println!("未生效，运行 deskbaton-cli sleep-helper install 安装");
                }
            }
        },
    }
    Ok(())
}

fn main() -> ExitCode {
    match run(Cli::parse().command) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("错误：{e}");
            ExitCode::FAILURE
        }
    }
}
