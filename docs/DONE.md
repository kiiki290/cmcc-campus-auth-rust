# DONE

- [2026-10-28] 初始化 Rust GUI 工程：egui/eframe 0.35 + tokio + chromiumoxide + ddddocr-rs；
  模块骨架、图标、.gitignore。
- [2026-10-28] 核心模块：黑洞探测/放行确认（原始 HTTP 解析、三态）、配置读写、系统密钥环、
  Portal 根地址推导、浏览器定位（随包/系统/自定义）。
- [2026-10-28] 认证核心：chromiumoxide 启动、frame 定位、验证码获取与 OCR、JS 填表与
  `form.submit()`、结果解析、致命错误、BRAS 放行确认、3 轮重试、断线监控。
- [2026-10-28] GUI：状态/控制/设置/日志、托盘常驻、单实例唤起、开机自启、关闭到托盘、
  中文字体加载、调试截图模式、任务异常状态复位。
- [2026-10-28] 测试：23 个单元测试（另有 4 个需真机环境的 ignored）；OCR 冒烟（6971 识别正确）；mock Portal + 真实
  chrome-headless-shell 端到端通过；密钥环、开机自启、单实例实测；UI 截图验证；
  最终 bundled 发布包在无环境变量下自动降级沙箱并完成认证。
- [2026-10-28] 提交前独立审查修复：--minimized 首帧隐藏、监控 Busy 状态、macOS 自启签名、
  自启路径引号、托盘宿主探测、zh-CN 语言/Accept-Language、隐藏时致命错误弹窗、
  扩大致命错误识别、监控 3 轮失败后 30 秒继续、单次认证可取消、Portal 主机白名单、
  监听线程读超时、CJK 字体兜底。
- [2026-10-28] 第二轮审查修复：成功页不再被隐藏文案误判致命、认证 frame 自身与
  表单 action 同源+端口校验、白名单错误按 Fatal 停止重试、guard 状态复位顺序、
  清除凭据同时取消认证、托盘宿主探测 gdbus 兜底/误报时不禁用托盘、探测测试超时与文档。
- [2026-10-28] 第三轮审查修复：标题“登录失败”时仍检查整页 FATAL_PATTERNS，
  避免错误信息只在正文/脚本里时被当成普通失败重试。
- [2026-10-28] 发布包复验：白名单拒绝时 Fatal、显示窗口且不启动浏览器；
  正常路径 --minimized 隐藏（IsUnMapped）、沙箱自动降级、连续 3 次认证成功。
- [2026-10-28] 打包与文档：Linux/Windows 双分发脚本、README、CLAUDE.md、MIT License、
  ddddocr 模型与测试验证码资源；文件日志（app.log）、版本一致的 UA、显式跨平台配置路径；Ubuntu 23.10+ 沙箱不可用时自动降级并记忆。
- [2026-10-29] 实网定位并修复“找不到认证 Frame”：Portal 的瑞数动态防护会识别
  chromiumoxide stealth 补丁并返回 400 空页面。改为只覆盖 UA，保留
  `--disable-blink-features=AutomationControlled`。修复后单次认证全流程实网验证通过
  （系统 Edge：探测→frame→OCR→JS 提交→结果→BRAS 放行），mock 端到端回归通过。
