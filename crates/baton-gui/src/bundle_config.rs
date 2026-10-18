//! 锁住安装器规格要求的 Tauri bundle 字段。

#[cfg(test)]
mod tests {
    #[test]
    fn bundle_matches_installer_spec() {
        let raw = include_str!("../tauri.conf.json");
        let v: serde_json::Value = serde_json::from_str(raw).unwrap();
        assert_eq!(v["productName"], "DeskBaton");
        assert_eq!(v["identifier"], "io.github.rossettaylm.deskbaton.panel");
        assert_eq!(v["bundle"]["active"], true);
        let targets = v["bundle"]["targets"].as_array().unwrap();
        assert!(targets.iter().any(|t| t == "dmg"));
        assert!(targets.iter().any(|t| t == "nsis"));
        assert_eq!(targets.len(), 2);
        assert_eq!(v["bundle"]["windows"]["nsis"]["installMode"], "currentUser");
        assert_eq!(
            v["bundle"]["windows"]["nsis"]["installerHooks"],
            "windows/hooks.nsh"
        );
        let bins = v["bundle"]["externalBin"].as_array().unwrap();
        assert!(bins.iter().any(|b| b == "binaries/deskbaton-sleep-helper"));
    }

    #[test]
    fn nsis_hooks_write_current_user_run_key() {
        let hooks = include_str!("../windows/hooks.nsh");
        assert!(hooks.contains("NSIS_HOOK_POSTINSTALL"));
        assert!(hooks.contains("NSIS_HOOK_PREUNINSTALL"));
        assert!(hooks.contains("Software\\Microsoft\\Windows\\CurrentVersion\\Run"));
        assert!(hooks.contains("DeskBaton"));
        assert!(hooks.contains("$INSTDIR\\DeskBaton.exe"));
        assert!(hooks.contains("WriteRegStr HKCU"));
        assert!(hooks.contains("DeleteRegValue HKCU"));
    }
}
