//! 开机自启（Windows 当前用户注册表 / Linux XDG Autostart）。

use auto_launch::AutoLaunch;

pub const APP_NAME: &str = "cmcc-campus-auth";
pub const ARG_MINIMIZED: &str = "--minimized";

fn launcher() -> Result<AutoLaunch, String> {
    if !AutoLaunch::is_support() {
        return Err("当前平台不支持开机自启".to_string());
    }
    let exe = std::env::current_exe().map_err(|err| format!("获取程序路径失败：{err}"))?;
    let exe = exe.to_string_lossy().to_string();
    let launch_path = launch_path(&exe);
    let args = [ARG_MINIMIZED];

    #[cfg(target_os = "windows")]
    let launcher = AutoLaunch::new(
        APP_NAME,
        &launch_path,
        auto_launch::WindowsEnableMode::CurrentUser,
        &args,
    );

    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    let launcher = AutoLaunch::new(
        APP_NAME,
        &launch_path,
        auto_launch::LinuxLaunchMode::XdgAutostart,
        &args,
    );

    #[cfg(target_os = "macos")]
    let launcher = {
        let bundle_ids: &[&str] = &[];
        AutoLaunch::new(
            APP_NAME,
            &launch_path,
            auto_launch::MacOSLaunchMode::LaunchAgent,
            &args,
            bundle_ids,
            "",
        )
    };

    Ok(launcher)
}

/// auto-launch 0.6 在 Windows Run 键与 Linux `.desktop` 的 Exec 中直接拼接路径，
/// 不处理空格；这里显式加引号。macOS 的 LaunchAgent 使用 ProgramArguments 数组，
/// 不能带引号。
fn launch_path(exe: &str) -> String {
    #[cfg(any(target_os = "windows", target_os = "linux", target_os = "freebsd"))]
    {
        format!("\"{exe}\"")
    }
    #[cfg(target_os = "macos")]
    {
        exe.to_string()
    }
}

pub fn is_enabled() -> bool {
    launcher()
        .and_then(|launcher| launcher.is_enabled().map_err(|err| err.to_string()))
        .unwrap_or(false)
}

pub fn set_enabled(enabled: bool) -> Result<(), String> {
    let launcher = launcher()?;
    let result = if enabled {
        launcher.enable()
    } else {
        launcher.disable()
    };
    result.map_err(|err| {
        format!(
            "{}开机自启失败：{err}",
            if enabled { "启用" } else { "关闭" }
        )
    })
}

#[cfg(test)]
mod tests {
    #[test]
    #[ignore = "会修改 ~/.config/autostart，需在隔离的 HOME 下运行"]
    fn autostart_roundtrip() {
        let before = super::is_enabled();
        super::set_enabled(true).unwrap();
        assert!(super::is_enabled());

        #[cfg(target_os = "linux")]
        {
            // auto-launch 0.6 在 Linux 上硬编码 $HOME/.config，不读 XDG_CONFIG_HOME
            let home = std::env::var_os("HOME").expect("请设置 HOME 指向临时目录");
            let entry = std::path::PathBuf::from(home)
                .join(".config")
                .join("autostart")
                .join(format!("{}.desktop", super::APP_NAME));
            let data = std::fs::read_to_string(&entry).expect("读取自启 .desktop 失败");
            assert!(
                data.contains("Exec=\""),
                ".desktop Exec 未给路径加引号：{data}"
            );
            assert!(data.contains(super::ARG_MINIMIZED));
        }

        super::set_enabled(false).unwrap();
        assert!(!super::is_enabled());
        super::set_enabled(before).unwrap();
        assert_eq!(super::is_enabled(), before);
    }

    #[test]
    fn launch_path_is_quoted() {
        #[cfg(not(target_os = "macos"))]
        assert_eq!(super::launch_path("/tmp/a b/app"), "\"/tmp/a b/app\"");
    }
}
