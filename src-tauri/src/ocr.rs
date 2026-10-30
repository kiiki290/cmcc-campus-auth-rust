//! 验证码识别：ddddocr 旧版模型（与 Python ddddocr 默认模型一致）。
//!
//! 模型直接编译进二进制（13 MB），保证首次运行无需联网下载——校园网未认证
//! 状态下本来也下载不了。字符集与归一化参数使用 ddddocr-core 内置的旧版配置。

use std::sync::{Arc, OnceLock};

use anyhow::{Context, Result};
use ddddocr_core::traits::Loader;
use ddddocr_core::{ModelMetadata, Normalization, Ocr, Resize};

static MODEL: &[u8] = include_bytes!("../../assets/models/common_old.onnx");

pub struct OcrEngine {
    runtime: Arc<ddddocr_ort::OcrRuntime>,
}

impl OcrEngine {
    pub fn load() -> Result<Self> {
        let session = ddddocr_ort::loader::ModelLoader::default()
            .build_from_bytes(MODEL)
            .map_err(|err| anyhow::anyhow!("OCR 模型加载失败：{err}"))?;
        let metadata = ModelMetadata::from_builtin_old(
            false,
            Resize::DynamicWidth(64),
            1,
            Normalization::ZeroToOne,
        );
        Ok(Self {
            runtime: Arc::new(ddddocr_ort::OcrRuntime::new(session, metadata)),
        })
    }

    pub fn recognize(&self, image_bytes: &[u8]) -> Result<String> {
        let image = image::load_from_memory(image_bytes).context("验证码图片解码失败")?;
        let result = Ocr::builder()
            .build_with(self.runtime.as_ref())
            .predict(&image)
            .map_err(|err| anyhow::anyhow!("OCR 推理失败：{err}"))?;
        let text = result.into_text().trim().replace(' ', "");
        if text.starts_with("Error:") {
            anyhow::bail!("OCR 无法识别该图片：{text}");
        }
        Ok(text)
    }
}

static CACHE: OnceLock<Result<Arc<OcrEngine>, String>> = OnceLock::new();

/// 全局只加载一次模型（首次约 0.2–1s），后续复用会话。
pub fn load_cached() -> Result<Arc<OcrEngine>> {
    match CACHE.get_or_init(|| {
        OcrEngine::load()
            .map(Arc::new)
            .map_err(|err| format!("{err:#}"))
    }) {
        Ok(engine) => Ok(engine.clone()),
        Err(message) => Err(anyhow::anyhow!("{message}")),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    #[ignore = "需要 CMCC_TEST_CAPTCHA 指向一张验证码图片"]
    fn ocr_smoke() {
        let path = std::env::var("CMCC_TEST_CAPTCHA")
            .expect("请设置 CMCC_TEST_CAPTCHA 环境变量指向验证码图片");
        let bytes = std::fs::read(&path).expect("读取验证码图片失败");
        let engine = super::load_cached().expect("加载 OCR 模型失败");
        let text = engine.recognize(&bytes).expect("OCR 识别失败");
        println!("OCR 识别结果：{text}");
        assert!(text.chars().count() >= 4, "识别结果过短：{text:?}");
    }
}
