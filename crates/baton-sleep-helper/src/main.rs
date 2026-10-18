use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match baton_sleep_helper::allowed_argv(&args) {
        Some(_) if cfg!(target_os = "macos") => exec_pmset(&args),
        Some(_) => {
            eprintln!("当前操作系统不支持 sleep-helper");
            ExitCode::from(2)
        }
        None => ExitCode::from(2),
    }
}

#[cfg(target_os = "macos")]
fn exec_pmset(args: &[String]) -> ExitCode {
    use std::os::unix::process::CommandExt;
    let err = std::process::Command::new("/usr/bin/pmset")
        .args(args)
        .exec();
    eprintln!("无法执行 pmset：{err}");
    ExitCode::from(1)
}

#[cfg(not(target_os = "macos"))]
fn exec_pmset(_: &[String]) -> ExitCode {
    ExitCode::from(2)
}
