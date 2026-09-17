//! 托盘图标与菜单状态图标的加载 / 生成。

use tray_icon::Icon;

/// 生成一个 32x32 的实心圆形 RGBA 像素数据，用于菜单状态指示。
pub fn circle_rgba(r: u8, g: u8, b: u8) -> Vec<u8> {
    let mut pixels = vec![0u8; 32 * 32 * 4];
    for y in 0..32 {
        for x in 0..32 {
            let dx = x as f32 - 16.0;
            let dy = y as f32 - 16.0;
            let dist = (dx * dx + dy * dy).sqrt();

            let idx = (y * 32 + x) * 4;

            if dist < 14.0 {
                pixels[idx] = r;
                pixels[idx + 1] = g;
                pixels[idx + 2] = b;
                pixels[idx + 3] = 255;
            }
        }
    }
    pixels
}

/// 根据运行状态返回菜单项前缀图标（绿色=运行，红色=停止）。
pub fn get_menu_icon(running: bool) -> tray_icon::menu::Icon {
    let rgba = if running {
        circle_rgba(0, 200, 0)
    } else {
        circle_rgba(200, 0, 0)
    };

    tray_icon::menu::Icon::from_rgba(rgba, 32, 32).unwrap()
}

/// 加载内嵌的默认托盘图标。
pub fn load_default() -> Icon {
    let img = image::load_from_memory(include_bytes!("../icons/tray.png"))
        .unwrap()
        .into_rgba8();

    let (w, h) = img.dimensions();
    Icon::from_rgba(img.into_raw(), w, h).unwrap()
}

/// 从指定路径加载图标；加载失败则回退到默认内嵌图标。
pub fn load_from_path(path: &str) -> Icon {
    if let Ok(reader) = image::ImageReader::open(path) {
        if let Ok(img) = reader.decode() {
            let rgba = img.to_rgba8();
            let (width, height) = rgba.dimensions();

            if let Ok(icon) = Icon::from_rgba(rgba.into_raw(), width, height) {
                return icon;
            }
        }
    }

    load_default()
}
