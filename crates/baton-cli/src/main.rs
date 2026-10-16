use std::process::ExitCode;
use std::thread;
use std::time::Duration;

use baton_core::Result;
use baton_core::ids::{DisplayId, InputCode, MacAddr};
use baton_core::ports::{DisplayControl, DisplayInfo};
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
}

fn resolve(display: &dyn DisplayControl, matcher: &str) -> Result<DisplayId> {
    Ok(DisplayInfo::select(display.list()?, matcher)?.id)
}

fn run(command: Command) -> Result<()> {
    match command {
        Command::Displays => {
            for d in baton_display::platform_display().list()? {
                println!(
                    "{}\t{} {}\t{}",
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
            let duration = Duration::from_secs(secs);
            let _guard = baton_power::platform_power().stay_awake(duration)?;
            println!("保持唤醒 {secs} 秒，期间可合盖测试");
            thread::sleep(duration);
        }
        Command::RestoreSleep => {
            baton_power::platform_power().restore_sleep()?;
            println!("睡眠设置已恢复");
        }
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
