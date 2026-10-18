//! macOS 合盖切换时阻止睡眠的特权来源：一条 sudoers 规则，只允许当前用户免密码执行
//! `pmset -a disablesleep 1` 和 `pmset -a disablesleep 0`。

pub use imp::{install, installed, uninstall};
#[cfg(target_os = "macos")]
pub(crate) use imp::{set_sleep_disabled, sleep_disabled};

#[cfg(any(target_os = "macos", test))]
use baton_core::{Error, Result};

#[cfg(any(target_os = "macos", test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SleepPrivileges {
    Helper,
    Sudoers,
}

#[cfg(any(target_os = "macos", test))]
pub(crate) fn privileges_for(bundled: bool) -> SleepPrivileges {
    if bundled {
        SleepPrivileges::Helper
    } else {
        SleepPrivileges::Sudoers
    }
}

pub const SUDOERS_PATH: &str = "/etc/sudoers.d/deskbaton";
#[cfg(any(target_os = "macos", test))]
const PMSET: &str = "/usr/bin/pmset";

#[cfg(any(target_os = "macos", test))]
fn pmset_args(disabled: bool) -> [&'static str; 3] {
    ["-a", "disablesleep", if disabled { "1" } else { "0" }]
}

/// 用户名直接写进 sudoers，只接受 macOS 短名的字符集，排除 `ALL`、`%组`、逗号、空白等语法字符。
#[cfg(any(target_os = "macos", test))]
fn sudoers_rule(user: &str) -> Result<String> {
    let mut chars = user.chars();
    let valid = chars
        .next()
        .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "_-.".contains(c));
    if !valid {
        return Err(Error::Config(format!("用户名不能写进 sudoers：{user:?}")));
    }
    let command = |disabled| format!("{PMSET} {}", pmset_args(disabled).join(" "));
    Ok(format!(
        "# Managed by deskbaton-cli sleep-helper: keeps a lid-closed Mac awake during handoff.\n\
         {user} ALL=(root) NOPASSWD: {}, {}\n",
        command(true),
        command(false)
    ))
}

/// 解析 `pmset -g` 输出中的 `SleepDisabled` 行。
#[cfg(any(target_os = "macos", test))]
fn parse_sleep_disabled(pmset_g: &str) -> Option<bool> {
    pmset_g.lines().find_map(|line| {
        let mut fields = line.split_whitespace();
        (fields.next()? == "SleepDisabled").then_some(())?;
        match fields.next()? {
            "1" => Some(true),
            "0" => Some(false),
            _ => None,
        }
    })
}

#[cfg(target_os = "macos")]
mod imp {
    use std::fs;
    use std::process::{Command, Stdio};

    use baton_core::{Error, Result};
    use baton_macos_sys as sys;

    use super::{PMSET, SUDOERS_PATH, parse_sleep_disabled, pmset_args, sudoers_rule};

    const SUDO: &str = "/usr/bin/sudo";
    /// sudo 跳过文件名含 `.` 的文件，规则先以 root 身份放在这里校验，通过后再改名生效。
    const STAGED_PATH: &str = "/etc/sudoers.d/deskbaton.new";

    /// 规则是否已生效（不会提示输入密码）。
    pub fn installed() -> bool {
        Command::new(SUDO)
            .args(["-n", "-l", PMSET])
            .args(pmset_args(true))
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    }

    /// 为当前用户写入规则；会在终端提示输入管理员密码。
    pub fn install() -> Result<()> {
        let user = std::env::var("USER").map_err(|_| Error::Config("读不到当前用户名".into()))?;
        if user == "root" {
            return Err(Error::Config("请以平时登录的用户运行，不要加 sudo".into()));
        }
        let rule = sudoers_rule(&user)?;
        let draft = std::env::temp_dir().join(format!("deskbaton-sudoers-{}", std::process::id()));
        fs::write(&draft, rule).map_err(|e| Error::Power(format!("写入临时规则失败：{e}")))?;
        let draft = draft.to_string_lossy().into_owned();
        let result = sudo(
            &[
                "/usr/bin/install",
                "-m",
                "0440",
                "-o",
                "root",
                "-g",
                "wheel",
                &draft,
                STAGED_PATH,
            ],
            "复制规则",
        )
        .and_then(|()| {
            sudo(&["/usr/sbin/visudo", "-cf", STAGED_PATH], "校验规则").inspect_err(|_| {
                let _ = sudo(&["/bin/rm", "-f", STAGED_PATH], "删除未通过校验的规则");
            })
        })
        .and_then(|()| sudo(&["/bin/mv", "-f", STAGED_PATH, SUDOERS_PATH], "启用规则"));
        let _ = fs::remove_file(&draft);
        result
    }

    /// 删除规则；会在终端提示输入管理员密码。
    pub fn uninstall() -> Result<()> {
        sudo(&["/bin/rm", "-f", SUDOERS_PATH, STAGED_PATH], "删除规则")
    }

    /// 安装版经特权助手，开发版经 sudoers；都失败时立即返回，不会等待输入密码。
    pub(crate) fn set_sleep_disabled(disabled: bool) -> Result<()> {
        match super::privileges_for(bundled()) {
            super::SleepPrivileges::Helper => set_via_helper(disabled),
            super::SleepPrivileges::Sudoers => set_via_sudo(disabled),
        }
    }

    fn bundled() -> bool {
        std::env::current_exe().ok().is_some_and(|p| {
            p.ancestors()
                .any(|a| a.extension().is_some_and(|e| e.eq_ignore_ascii_case("app")))
        })
    }

    fn set_via_helper(disabled: bool) -> Result<()> {
        // SAFETY: 只传 0/1。
        let rc = unsafe { sys::deskbaton_helper_set_sleep_disabled(i32::from(disabled)) };
        if rc == sys::OK {
            Ok(())
        } else {
            Err(Error::Power(
                "执行 pmset 失败，请在系统设置 → 登录项与扩展里允许 DeskBaton".into(),
            ))
        }
    }

    fn set_via_sudo(disabled: bool) -> Result<()> {
        let output = Command::new(SUDO)
            .arg("-n")
            .arg(PMSET)
            .args(pmset_args(disabled))
            .output()
            .map_err(|e| Error::Power(format!("无法运行 sudo：{e}")))?;
        if output.status.success() {
            return Ok(());
        }
        Err(Error::Power(format!(
            "执行 pmset 失败（{}），先运行 deskbaton-cli sleep-helper install",
            String::from_utf8_lossy(&output.stderr).trim()
        )))
    }

    pub(crate) fn sleep_disabled() -> Result<bool> {
        let output = Command::new(PMSET)
            .arg("-g")
            .output()
            .map_err(|e| Error::Power(format!("无法运行 pmset：{e}")))?;
        parse_sleep_disabled(&String::from_utf8_lossy(&output.stdout))
            .ok_or_else(|| Error::Power("pmset -g 的输出里没有 SleepDisabled".into()))
    }

    fn sudo(args: &[&str], what: &str) -> Result<()> {
        let status = Command::new(SUDO)
            .args(args)
            .status()
            .map_err(|e| Error::Power(format!("{what}：无法运行 sudo：{e}")))?;
        if status.success() {
            Ok(())
        } else {
            Err(Error::Power(format!("{what}失败")))
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod imp {
    use baton_core::{Error, Result};

    const WHAT: &str = "当前操作系统的 sleep-helper";

    pub fn installed() -> bool {
        false
    }

    pub fn install() -> Result<()> {
        Err(Error::Unsupported(WHAT))
    }

    pub fn uninstall() -> Result<()> {
        Err(Error::Unsupported(WHAT))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rule_allows_only_the_two_pmset_commands_for_one_user() {
        let rule = sudoers_rule("lymanyang").unwrap();
        let grants: Vec<&str> = rule.lines().filter(|l| !l.starts_with('#')).collect();
        assert_eq!(
            grants,
            [
                "lymanyang ALL=(root) NOPASSWD: /usr/bin/pmset -a disablesleep 1, /usr/bin/pmset -a disablesleep 0"
            ]
        );
        assert!(rule.is_ascii());
        assert!(rule.ends_with('\n'));
    }

    #[test]
    fn rule_rejects_unsafe_user_names() {
        for user in ["", "-root", "a b", "ALL", "x,y", "x:y", "%admin", "x\nroot"] {
            assert!(sudoers_rule(user).is_err(), "{user:?}");
        }
        assert!(sudoers_rule("first.last_2-x").is_ok());
    }

    #[test]
    fn parses_sleep_disabled_from_pmset_output() {
        let on = " sleep                0 (sleep prevented by powerd)\n SleepDisabled\t\t1\n";
        let off = " SleepDisabled\t\t0\n displaysleep 60\n";
        assert_eq!(parse_sleep_disabled(on), Some(true));
        assert_eq!(parse_sleep_disabled(off), Some(false));
        assert_eq!(parse_sleep_disabled(" displaysleep 60\n"), None);
    }

    #[test]
    fn pmset_args_toggle_disablesleep() {
        assert_eq!(pmset_args(true), ["-a", "disablesleep", "1"]);
        assert_eq!(pmset_args(false), ["-a", "disablesleep", "0"]);
    }

    #[test]
    fn installed_app_uses_helper_not_sudoers() {
        assert_eq!(privileges_for(true), SleepPrivileges::Helper);
        assert_eq!(privileges_for(false), SleepPrivileges::Sudoers);
    }
}
