# TODO

- [ ] **实网长稳验证**：断线自动重连的长时间稳定性、验证码识别率与 Portal 改版监控
      仍需持续观察。
      - [x] 2026-10-29 单次认证全流程实网验证通过（系统 Edge + 瑞数挑战 + 验证码 OCR +
        JS 提交 + BRAS 放行）；瑞数挑战失败（stealth 补丁）已实网复现并修复。
- [ ] **Windows 构建与打包验证**：在 Windows 上 `cargo build --release`、运行 lite/bundled
      两个包，验证 Edge/Chrome 定位、凭据管理器、注册表自启、托盘。
- [ ] 可选：迁移旧 Python 版配置（学号、检测间隔）与 DPAPI 密码（Windows `windows` crate）。
- [ ] 可选：GitHub Actions 双平台构建产物。
- [ ] 可选：Windows exe 文件图标（资源嵌入）与应用清单。
- [ ] 可选：托盘菜单在 KSNI 下动态启用/禁用状态的实机验证。
- [ ] 可选：为随包 Chromium 提供 AppArmor profile，避免 Ubuntu 23.10+ 降级为 --no-sandbox。
- [ ] 可选：浏览器启动/goto/fetch 等长等待用 select 响应取消（当前取消最多延迟数秒到 30 秒）。
