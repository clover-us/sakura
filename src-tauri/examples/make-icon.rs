//! 图标生成工具（**开发期工具，不属于应用运行路径**）。
//!
//! 用法：
//!
//! ```powershell
//! cd src-tauri
//! cargo run --example make-icon                 # 写出 icons/ 全套 + 候选对比图
//! cargo run --example make-icon -- --style aqua # 换一种配色写 icons/
//! ```
//!
//! 产出：
//!   - `icons/icon.ico`：16/24/32/48/64/128/256 七个尺寸（exe、任务栏、安装包都用它）
//!   - `icons/icon.png`（1024）、`icons/32x32.png`、`icons/128x128.png`、`icons/128x128@2x.png`
//!     （Tauri 打包与 macOS/移动端将来会用到）
//!   - `icons/tray-32.png`：托盘版细节（省掉小尺寸看不清的气泡/嘴）
//!   - `icons/candidates.png`：三种配色的对比图（64/32/16 真实像素 + 256 放大），用来挑样式
//!
//! 为什么用 `examples/` 而不是 `src/bin/`：png 编码器只有这个工具需要，
//! 放 examples 可以走 `[dev-dependencies]`，**应用二进制里不会多出这个依赖**。

use std::fs::File;
use std::io::BufWriter;
use std::path::PathBuf;

use whale_pet_desktop_lib::icon_art::{self, Detail, Raster, Style};

/// ico 里包含的尺寸（覆盖 Windows 任务栏/资源管理器/Alt-Tab 全部场景）
const ICO_SIZES: [u32; 7] = [16, 24, 32, 48, 64, 128, 256];

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut out_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("icons");
    let mut style = Style::Rose;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--out" => {
                if let Some(dir) = args.next() {
                    out_dir = PathBuf::from(dir);
                }
            }
            "--style" => {
                if let Some(name) = args.next() {
                    style = match name.as_str() {
                        "rose" => Style::Rose,
                        "midnight" => Style::Midnight,
                        "aqua" => Style::Aqua,
                        other => return Err(format!("未知配色：{other}（可选 rose / midnight / aqua）").into()),
                    };
                }
            }
            other => return Err(format!("未知参数：{other}").into()),
        }
    }
    std::fs::create_dir_all(&out_dir)?;

    // ---- 1. 主图标（1024 与 Tauri 约定的几个尺寸）----
    let master = icon_art::render(1024, style, Detail::Full);
    write_png(&out_dir.join("icon.png"), &master)?;
    for (name, size) in [("32x32.png", 32u32), ("128x128.png", 128), ("128x128@2x.png", 256)] {
        write_png(&out_dir.join(name), &icon_art::render(size, style, Detail::Full))?;
    }

    // ---- 2. 多尺寸 ico ----
    // `encode` 会按尺寸自动挑 PNG 还是 BMP 存储（大尺寸走 PNG，小尺寸走 BMP 兼容性最好）
    let mut ico = ico::IconDir::new(ico::ResourceType::Icon);
    for size in ICO_SIZES {
        let raster = icon_art::render(size, style, Detail::Full);
        let image = ico::IconImage::from_rgba_data(raster.width, raster.height, raster.rgba);
        ico.add_entry(ico::IconDirEntry::encode(&image)?);
    }
    let ico_path = out_dir.join("icon.ico");
    ico.write(&mut BufWriter::new(File::create(&ico_path)?))?;

    // ---- 3. 托盘版（32px：应用运行时也是这套细节）----
    write_png(&out_dir.join("tray-32.png"), &icon_art::render(32, style, Detail::Tray))?;
    write_png(&out_dir.join("tray-16.png"), &icon_art::render(16, style, Detail::Tray))?;

    // ---- 4. 候选对比图（挑样式用）----
    write_png(&out_dir.join("candidates.png"), &candidate_sheet(style))?;

    println!("图标已写入 {}", out_dir.display());
    println!("  当前配色：{}", style.id());
    for entry in std::fs::read_dir(&out_dir)? {
        let entry = entry?;
        let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
        println!("  - {} ({} 字节)", entry.file_name().to_string_lossy(), size);
    }
    Ok(())
}

fn write_png(path: &PathBuf, raster: &Raster) -> Result<(), Box<dyn std::error::Error>> {
    let file = BufWriter::new(File::create(path)?);
    let mut encoder = png::Encoder::new(file, raster.width, raster.height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header()?;
    writer.write_image_data(&raster.rgba)?;
    Ok(())
}

/// 三种配色的对比图：每种一列（256 放大 + 64/32/16 真实像素）
fn candidate_sheet(current: Style) -> Raster {
    const CELL: u32 = 300;
    const ROW_BIG: u32 = 260;
    const SHEET_W: u32 = CELL * 3;
    const SHEET_H: u32 = ROW_BIG + 150;
    let mut sheet = vec![0u8; (SHEET_W * SHEET_H * 4) as usize];
    // 浅灰底，方便看透明边缘与深色方案
    for pixel in sheet.chunks_exact_mut(4) {
        pixel.copy_from_slice(&[246, 246, 248, 255]);
    }
    for (column, style) in Style::all().into_iter().enumerate() {
        let x0 = column as u32 * CELL + (CELL - 256) / 2;
        blit(&mut sheet, SHEET_W, &icon_art::render(256, style, Detail::Full), x0, 10);
        let mut x = column as u32 * CELL + 40;
        for size in [64u32, 32, 16] {
            let y = ROW_BIG + 20 + (64 - size) / 2;
            blit(&mut sheet, SHEET_W, &icon_art::render(size, style, Detail::Tray), x, y);
            x += size + 24;
        }
        // 标出当前选中的方案（在标题位置画一条短线）
        if style == current {
            let y = ROW_BIG + 110;
            for dy in 0..6 {
                for dx in 0..60 {
                    let px = column as u32 * CELL + 120 + dx;
                    let py = y + dy;
                    let idx = ((py * SHEET_W + px) * 4) as usize;
                    sheet[idx..idx + 4].copy_from_slice(&[232, 116, 160, 255]);
                }
            }
        }
    }
    Raster { width: SHEET_W, height: SHEET_H, rgba: sheet }
}

/// 把一张 RGBA 图贴到另一张上（左上角为 `(x0, y0)`，直接覆盖）
fn blit(dst: &mut [u8], dst_w: u32, src: &Raster, x0: u32, y0: u32) {
    for y in 0..src.height {
        for x in 0..src.width {
            let sx = x0 + x;
            let sy = y0 + y;
            if sx >= dst_w {
                continue;
            }
            let sidx = ((y * src.width + x) * 4) as usize;
            let didx = ((sy * dst_w + sx) * 4) as usize;
            if didx + 4 > dst.len() {
                continue;
            }
            let a = src.rgba[sidx + 3] as f32 / 255.0;
            for c in 0..3 {
                let s = src.rgba[sidx + c] as f32;
                let d = dst[didx + c] as f32;
                dst[didx + c] = (s * a + d * (1.0 - a)).round() as u8;
            }
            dst[didx + 3] = 255;
        }
    }
}
