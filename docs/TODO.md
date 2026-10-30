# TODO

- [ ] **Windows 安装包与功能验证**：运行 lite/bundled 两个包，验证 Chrome/Chromium
      定位、凭据管理器、注册表自启、托盘；NSIS 安装包尚未实机安装验证。
      - [x] 2026-10-29 Windows 11 + MSVC：`cargo build --release`、单元测试、GUI 运行、
        mock Portal 端到端通过；实网单次认证通过（系统 Edge）。
- [ ] **实网长稳验证**：断线自动重连的长时间稳定性、验证码识别率与 Portal 改版监控
      仍需持续观察。
      - [x] 2026-10-29 单次认证全流程实网验证通过（Windows 11 + 系统 Edge + 瑞数挑战 +
        验证码 OCR + JS 提交 + BRAS 放行）；瑞数挑战失败（stealth 补丁）已实网复现并修复。
- [ ] 可选：在正常桌面会话（真实 GPU 合成）中人工确认窗口渲染；本机会话会把新建
      XWayland 窗口置为 Iconic，交互已在 Xvfb 下验证，原生 Wayland 路径仅确认进程与
      日志正常。
- [ ] 可选：迁移旧 Python 版配置（学号、检测间隔）与 DPAPI 密码（Windows `windows` crate）。
- [ ] 可选：GitHub Actions 双平台构建产物。
- [ ] 可选：Linux 开机自启改为手写 `.desktop`（`auto-launch` 写死 `~/.config/autostart`，
      不遵循 `XDG_CONFIG_HOME`，测试隔离不友好）。
- [ ] 可选：为随包 Chromium 提供 AppArmor profile，避免 Ubuntu 23.10+ 降级为 --no-sandbox。
- [ ] 可选：浏览器启动/goto/fetch 等长等待用 select 响应取消（当前取消最多延迟数秒到 30 秒）。
