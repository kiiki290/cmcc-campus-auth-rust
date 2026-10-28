# ddddocr 验证码模型

- 文件：`common_old.onnx`（约 13 MB）
- 来源：<https://github.com/sml2h3/ddddocr/blob/master/ddddocr/common_old.onnx>
- 下载日期：2026-10-28
- SHA-256：`b8f2ad9cbc1f2e3922a6cb9459e30824e7e2467f3fb4fd61420640e34ea0bf68`
- 许可证：ddddocr 项目为 MIT License

本仓库将该模型通过 `include_bytes!` 编译进二进制，运行时解压/加载都不需要联网。
识别参数与 Python ddddocr 默认配置保持一致（旧版字符集、
`Resize::DynamicWidth(64)`、灰度单通道、`Normalization::ZeroToOne`）。
