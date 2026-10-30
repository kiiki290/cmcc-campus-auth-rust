//! 一次性生成 1024×1024 源图标，供 `npx @tauri-apps/cli icon` 生成全套平台图标。
//!
//! 用法：`cargo run --example gen_icon -- 1024 src-tauri/app-icon.png`

fn main() {
    let mut args = std::env::args().skip(1);
    let size: u32 = args
        .next()
        .and_then(|value| value.parse().ok())
        .unwrap_or(1024);
    let path = args.next().unwrap_or_else(|| "app-icon.png".to_string());

    let rgba = cmcc_campus_auth::icon::icon_rgba(size);
    let image = image::RgbaImage::from_raw(size, size, rgba).expect("图标尺寸不匹配");
    image.save(&path).expect("保存图标失败");
    println!("已生成 {path}（{size}×{size}）");
}
