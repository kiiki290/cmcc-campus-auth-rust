//! 程序/托盘图标：运行时绘制，避免引入二进制美术资源。

/// 生成 RGBA 图标：蓝色圆底 + 白色 Wi-Fi 信号。
pub fn icon_rgba(size: u32) -> Vec<u8> {
    let mut pixels = vec![0u8; (size * size * 4) as usize];
    let s = size as f32;
    let sub = 3u32;
    for y in 0..size {
        for x in 0..size {
            let mut acc = [0f32; 4];
            for sy in 0..sub {
                for sx in 0..sub {
                    let px = (x as f32 + (sx as f32 + 0.5) / sub as f32) / s * 2.0 - 1.0;
                    let py = (y as f32 + (sy as f32 + 0.5) / sub as f32) / s * 2.0 - 1.0;
                    let c = sample(px, py);
                    for i in 0..4 {
                        acc[i] += c[i];
                    }
                }
            }
            let n = (sub * sub) as f32;
            let idx = ((y * size + x) * 4) as usize;
            for i in 0..4 {
                pixels[idx + i] = (acc[i] / n * 255.0).clamp(0.0, 255.0).round() as u8;
            }
        }
    }
    pixels
}

fn sample(x: f32, y: f32) -> [f32; 4] {
    let radius = (x * x + y * y).sqrt();
    if radius > 1.0 {
        return [0.0, 0.0, 0.0, 0.0];
    }

    // 蓝色圆底（边缘做一圈深色描边）
    let t = (y + 1.0) * 0.5;
    let mut color = if radius > 0.94 {
        [0.02, 0.20, 0.45, 1.0]
    } else {
        [0.05 + 0.04 * t, 0.40 + 0.10 * t, 0.82 + 0.08 * t, 1.0]
    };

    // Wi-Fi：圆心偏下，向上三段弧 + 圆点
    let cx = 0.0;
    let cy = 0.42;
    let dx = x - cx;
    let dy = y - cy;
    let dist = (dx * dx + dy * dy).sqrt();
    let angle = dy.atan2(dx);
    let in_arc_angle = angle > -2.45 && angle < -0.69;
    if in_arc_angle {
        for (arc_radius, half_width) in [(0.30, 0.065), (0.56, 0.075), (0.82, 0.085)] {
            if (dist - arc_radius).abs() < half_width {
                color = [1.0, 1.0, 1.0, 1.0];
            }
        }
    }
    if dist < 0.13 {
        color = [1.0, 1.0, 1.0, 1.0];
    }

    // 边缘抗锯齿
    if radius > 0.96 {
        let alpha = ((1.0 - radius) / 0.04).clamp(0.0, 1.0);
        color[3] *= alpha;
    }
    color
}
