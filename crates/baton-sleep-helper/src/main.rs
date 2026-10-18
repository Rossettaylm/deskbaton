use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        return serve();
    }
    match baton_sleep_helper::allowed_argv(&args) {
        Some(_) => exec_pmset(&args),
        None => ExitCode::from(2),
    }
}

#[cfg(target_os = "macos")]
fn serve() -> ExitCode {
    // SAFETY: 阻塞在 dispatch_main，无其它线程同时调。
    unsafe {
        deskbaton_sleep_helper_serve();
    }
    ExitCode::from(1)
}

#[cfg(not(target_os = "macos"))]
fn serve() -> ExitCode {
    ExitCode::from(2)
}

#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn deskbaton_sleep_helper_serve();
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
    eprintln!("当前操作系统不支持 sleep-helper");
    ExitCode::from(2)
}
