//! 开机自启在启动时的决策：系统登录项的读写由面板做，这里只决定要不要开、刷新还是保持关。

/// 面板启动时应对登录项做的事。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutostartOnLaunch {
    /// 从未成功种过：尝试开启，成功后标记已种。
    EnableAndSeed,
    /// 已种且系统侧开着：把路径更新为当前可执行文件。
    RefreshPath,
    /// 已种且系统侧关着：不要再打开。
    LeaveOff,
}

/// `seeded` 为面板是否已成功种过一次；`enabled` 为系统里是否已有我们写的那条登录项。
pub fn on_launch(seeded: bool, enabled: bool) -> AutostartOnLaunch {
    match (seeded, enabled) {
        (false, _) => AutostartOnLaunch::EnableAndSeed,
        (true, true) => AutostartOnLaunch::RefreshPath,
        (true, false) => AutostartOnLaunch::LeaveOff,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_launch_tries_to_enable() {
        assert_eq!(on_launch(false, false), AutostartOnLaunch::EnableAndSeed);
        assert_eq!(on_launch(false, true), AutostartOnLaunch::EnableAndSeed);
    }

    #[test]
    fn seeded_and_off_stays_off() {
        assert_eq!(on_launch(true, false), AutostartOnLaunch::LeaveOff);
    }

    #[test]
    fn seeded_and_on_refreshes_path() {
        assert_eq!(on_launch(true, true), AutostartOnLaunch::RefreshPath);
    }
}
