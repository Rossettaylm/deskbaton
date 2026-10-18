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
        let cmd = v["build"]["beforeBundleCommand"].as_str().unwrap();
        assert!(
            cmd.contains("git rev-parse --show-toplevel"),
            "Tauri 钩子的 cwd 不是 crates/baton-gui，相对路径会 127"
        );
        assert!(cmd.contains("scripts/embed-sleep-daemon.sh"));
        let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../scripts/embed-sleep-daemon.sh");
        assert!(script.is_file(), "{}", script.display());
        assert_eq!(
            v["bundle"]["macOS"]["signingIdentity"], "-",
            "Apple Silicon 上 linker-signed 半截签名会被 Gatekeeper 报已损坏"
        );
        assert_eq!(v["bundle"]["createUpdaterArtifacts"], false);
        let endpoints = v["plugins"]["updater"]["endpoints"].as_array().unwrap();
        assert!(
            endpoints.iter().any(|e| e.as_str()
                == Some(
                    "https://github.com/Rossettaylm/deskbaton/releases/latest/download/latest.json"
                )),
            "更新清单走 GitHub latest.json"
        );
        let pubkey = v["plugins"]["updater"]["pubkey"].as_str().unwrap_or("");
        assert!(!pubkey.is_empty(), "更新器公钥必须写入 tauri.conf.json");
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

    #[test]
    fn pack_workflow_builds_named_installers() {
        let yml = include_str!("../../../.github/workflows/pack.yml");
        assert!(yml.contains("workflow_dispatch"));
        assert!(yml.contains("macos-latest"));
        assert!(yml.contains("windows-latest"));
        assert!(!yml.contains("\n  pull_request:"), "PR 不打安装包");
        assert!(yml.contains("tags:"));
        assert!(yml.contains("TAURI_SIGNING_PRIVATE_KEY"));
        assert!(yml.contains("createUpdaterArtifacts"));
        assert!(yml.contains("includeUpdaterJson"));
        assert!(yml.contains("updaterJsonPreferNsis"));
        assert!(yml.contains("contents: write"));
        assert!(
            yml.contains("steps.ver.outputs.version") && yml.contains("macos-aarch64.dmg"),
            "安装包文件名随 workspace 版本"
        );
        assert!(yml.contains("windows-x64-setup.exe"));
        assert!(
            !yml.contains("DeskBaton-0.0.1-macos-aarch64.dmg"),
            "不得写死 0.0.1 文件名"
        );
        assert!(yml.contains("tauri-apps/tauri-action"));
        assert!(yml.contains("crates/baton-gui"));
        assert!(yml.contains("beforeBundleCommand"));
        assert!(
            yml.contains("target/release/bundle/dmg") && yml.contains("|| true"),
            "产物在 workspace target 下；find 缺目录时不能让 pipefail 失败"
        );
    }

    fn pref_container_tag(html: &str, id: &str) -> &'static str {
        let pos = html
            .find(&format!("id=\"{id}\""))
            .unwrap_or_else(|| panic!("missing #{id}"));
        let before = &html[..pos];
        let pref = before
            .rfind("class=\"pref\"")
            .unwrap_or_else(|| panic!("#{id} not in .pref"));
        let tag_start = before[..pref]
            .rfind('<')
            .unwrap_or_else(|| panic!("#{id} .pref has no tag"));
        let tag = before[tag_start + 1..]
            .split(|c: char| c.is_whitespace() || c == '>')
            .next()
            .unwrap_or("");
        match tag {
            "label" => "label",
            "div" => "div",
            other => panic!("#{id} .pref is <{other}>"),
        }
    }

    #[test]
    fn pref_buttons_are_not_wrapped_in_labels() {
        let html = include_str!("../ui/index.html");
        assert_eq!(pref_container_tag(html, "pref-hotkey"), "div");
        assert_eq!(pref_container_tag(html, "pref-update"), "div");
        assert_eq!(pref_container_tag(html, "pref-name"), "label");
        assert_eq!(pref_container_tag(html, "pref-autostart"), "label");
        assert_eq!(pref_container_tag(html, "pref-notify"), "label");
        assert_eq!(pref_container_tag(html, "pref-clipboard"), "label");
    }
}
