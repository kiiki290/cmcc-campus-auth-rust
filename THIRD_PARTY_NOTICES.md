# 第三方组件与资源

- **ddddocr 验证码模型**（`assets/models/common_old.onnx`）
  来源：<https://github.com/sml2h3/ddddocr>，MIT License。
- **ddddocr-rs**（`ddddocr-core` / `ddddocr-ort`）
  来源：<https://github.com/CNWeiWei/ddddocr-rs>，MIT OR Apache-2.0。
- **ONNX Runtime**：通过 `ort` crate 的 `download-binaries` 获取，MIT License。
- **Chrome for Testing / chrome-headless-shell**（bundled 分发包内）
  来源：<https://googlechromelabs.github.io/chrome-for-testing/>，
  随附的 `LICENSE.headless_shell` 位于 `chromium/` 目录。
- 其余 Rust 依赖的许可证见 `Cargo.lock` 对应 crate 的元数据。
