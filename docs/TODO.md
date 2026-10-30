# TODO

- [ ] **Tauri 迁移收尾（2026-10-29）**：界面已实现并通过渲染截图验证，但交互项尚未
      逐一点击验证（开发机 Wayland 会话当时所有新窗口被 WM 置为 Iconic，无法自动化点击）。
      待办：保存/清除凭据、检测浏览器、关闭到托盘与菜单启用的实机点击验证；
      Windows NSIS 安装包实机验证；可见窗口状态下的内存复测（当前 PSS 数据测于
      WM 异常最小化时）。
      - [x] 2026-10-29 `docs/screenshot.png` 已用真实前端 + headless Chromium 重新渲染
        （880×1440），release 构建内存已实测并写入 README「资源占用」。
      - [x] 2026-10-29 目录迁移（workspace + `src-tauri/` + `ui/`）、命令层/事件桥、
        Tauri 托盘与单实例、关闭到托盘、前端控制/设置/日志/对话框、mock e2e、
        clippy、26 个单元测试。
- [ ] **实网长稳验证**：断线自动重连的长时间稳定性、验证码识别率与 Portal 改版监控
      仍需持续观察。
      - [x] 2026-10-29 单次认证全流程实网验证通过（Windows 11 + 系统 Edge + 瑞数挑战 +
        验证码 OCR + JS 提交 + BRAS 放行）；瑞数挑战失败（stealth 补丁）已实网复现并修复。
- [ ] **Windows 打包与其余功能验证**：运行 lite/bundled 两个包，验证 Chrome/Chromium
      定位、凭据管理器、注册表自启、托盘（系统 Edge 路径已实网验证）。
      - [x] 2026-10-29 Windows 11 + MSVC：`cargo build --release`、单元测试、GUI 运行、
        mock Portal 端到端均通过。
- [ ] 可选：迁移旧 Python 版配置（学号、检测间隔）与 DPAPI 密码（Windows `windows` crate）。
- [ ] 可选：GitHub Actions 双平台构建产物。
- [ ] 可选：托盘菜单（Tauri / libayatana）动态启用/禁用状态的实机验证。
- [ ] 可选：为随包 Chromium 提供 AppArmor profile，避免 Ubuntu 23.10+ 降级为 --no-sandbox。
- [ ] 可选：浏览器启动/goto/fetch 等长等待用 select 响应取消（当前取消最多延迟数秒到 30 秒）。
