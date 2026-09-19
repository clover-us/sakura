//! 图标生成工具（**开发期工具，不属于应用运行路径**）。
//!
//! 用法：
//!
//! ```powershell
//! cd src-tauri
//! cargo run --example make-icon                    # 用 design/app-icon.svg 生成 icons/ 全套
//! cargo run --example make-icon -- --svg design/candidates/b-face.svg
//! cargo run --example make-icon -- --sheet         # 只出候选对比图，不动正式图标
//! ```
//!
//! ## 为什么图标源改成 SVG
//!
//! 第一版图标是**用 Rust 画 SDF 图元**拼出来的（`icon_art.rs`）。好处是"一份代码两个用途"
//! （exe 的 .ico 与运行时托盘图标），但观感上限很低：圆润造型、渐变、柔和投影、笔画粗细
//! 这些"让它不难看"的东西，用椭圆和圆角矩形拼不出来——用户看完的评价就是"图标丑"。
//!
//! 现在：**矢量 SVG 是唯一源**（`icons/design/`），本工具用 `resvg` 光栅化成
//! PNG / 多尺寸 ICO / 托盘用的 RGBA 块。手改 SVG 比调 SDF 常量直观得多。
//!
//! 代价与取舍：
//!   - 托盘图标需要运行时 RGBA，而**应用不该为了画图标背上 SVG 渲染器**（resvg 及其依赖
//!     在二进制里是好几 MB）。所以托盘那份由本工具预先栅格化成一个 32×32 的 `.rgba`
//!     裸数据文件（4KB，随仓库提交），应用 `include_bytes!` 直接吃；
//!   - 因此改成 SVG 之后**必须记得跑一次本工具**：改了 `design/app-icon.svg` 却不重新生成，
//!     图标不会变（`icons/tray-32.rgba` 是生成物，不手改）。
//!
//! ## 产物
//!
//!   - `icons/icon.ico`：16/24/32/48/64/128/256 七个尺寸（exe、任务栏、安装包都用它）
//!   - `icons/icon.png`（1024）、`icons/32x32.png`、`icons/128x128.png`、`icons/128x128@2x.png`
//!   - `icons/tray-32.png`（给人看）与 `icons/tray-32.rgba`（给应用用，裸 RGBA8）
//!   - `icons/candidates.png`：候选方案对比图（256 放大 + 64/32/16 真实像素）
//!
//! 依赖只在 `[dev-dependencies]`（resvg / png / ico），**应用二进制不会多出它们**。

use std::fs::File;
use std::io::BufWriter;
use std::path::{Path, PathBuf};

/// ico 里包含的尺寸（覆盖 Windows 任务栏/资源管理器/Alt-Tab 全部场景）
const ICO_SIZES: [u32; 7] = [16, 24, 32, 48, 64, 128, 256];

/// 托盘图标边长（应用按这个尺寸读取 `tray-32.rgba`）
const TRAY_SIZE: u32 = 32;

struct Raster {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let icons_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("icons");
    let mut source = icons_dir.join("design/app-icon.svg");
    let mut sheet_only = false;

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--svg" => {
                if let Some(path) = args.next() {
                    let candidate = PathBuf::from(&path);
                    source = if candidate.is_absolute() { candidate } else { icons_dir.join(candidate) };
                }
            }
            "--sheet" => sheet_only = true,
            other => return Err(format!("未知参数：{other}（可用 --svg <path> / --sheet）").into()),
        }
    }

    std::fs::create_dir_all(&icons_dir)?;

    // ---- 候选对比图（挑样式用）----
    write_png(&icons_dir.join("candidates.png"), &candidate_sheet(&icons_dir, &source)?)?;
    if sheet_only {
        println!("候选对比图已更新：{}", icons_dir.join("candidates.png").display());
        return Ok(());
    }

    // ---- 正式图标 ----
    if !source.is_file() {
        return Err(format!("找不到图标源文件：{}", source.display()).into());
    }
    let svg = std::fs::read_to_string(&source)?;
    println!("图标源：{}", source.display());

    let master = rasterize(&svg, 1024)?;
    write_png(&icons_dir.join("icon.png"), &master)?;
    for (name, size) in [("32x32.png", 32u32), ("128x128.png", 128), ("128x128@2x.png", 256)] {
        write_png(&icons_dir.join(name), &rasterize(&svg, size)?)?;
    }

    // 多尺寸 ico：`encode` 会按尺寸自动挑 PNG 还是 BMP 存储
    let mut ico = ico::IconDir::new(ico::ResourceType::Icon);
    for size in ICO_SIZES {
        let raster = rasterize(&svg, size)?;
        let image = ico::IconImage::from_rgba_data(raster.width, raster.height, raster.rgba);
        ico.add_entry(ico::IconDirEntry::encode(&image)?);
    }
    ico.write(&mut BufWriter::new(File::create(icons_dir.join("icon.ico"))?))?;

    // 托盘：PNG 给人看，RGBA 裸数据给应用读（应用不背 SVG 渲染器）
    let tray = rasterize(&svg, TRAY_SIZE)?;
    write_png(&icons_dir.join("tray-32.png"), &tray)?;
    std::fs::write(icons_dir.join("tray-32.rgba"), &tray.rgba)?;
    println!(
        "托盘图标：tray-32.png + tray-32.rgba（{}×{}，{} 字节裸 RGBA）",
        tray.width,
        tray.height,
        tray.rgba.len()
    );

    println!("图标已写入 {}", icons_dir.display());
    for entry in std::fs::read_dir(&icons_dir)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            continue;
        }
        println!("  - {} ({} 字节)", entry.file_name().to_string_lossy(), entry.metadata()?.len());
    }
    Ok(())
}

/// 用 resvg 把 SVG 栅格化成指定边长的 RGBA
fn rasterize(svg: &str, size: u32) -> Result<Raster, Box<dyn std::error::Error>> {
    let options = usvg::Options::default();
    let tree = usvg::Tree::from_str(svg, &options)?;
    let mut pixmap = resvg::tiny_skia::Pixmap::new(size, size)
        .ok_or_else(|| format!("创建 {size}×{size} 画布失败"))?;
    // 等比缩放到目标边长（图标 SVG 都是正方形，长宽比一致）
    let scale = size as f32 / tree.size().width();
    resvg::render(&tree, resvg::tiny_skia::Transform::from_scale(scale, scale), &mut pixmap.as_mut());
    let mut rgba = Vec::with_capacity((size * size * 4) as usize);
    for pixel in pixmap.pixels() {
        let color = pixel.demultiply();
        rgba.extend_from_slice(&[color.red(), color.green(), color.blue(), color.alpha()]);
    }
    Ok(Raster { width: size, height: size, rgba })
}

fn write_png(path: &Path, raster: &Raster) -> Result<(), Box<dyn std::error::Error>> {
    let file = BufWriter::new(File::create(path)?);
    let mut encoder = png::Encoder::new(file, raster.width, raster.height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header()?;
    writer.write_image_data(&raster.rgba)?;
    Ok(())
}

/// 候选对比图：每个候选一列（256 放大 + 64/32/16 真实像素）
fn candidate_sheet(icons_dir: &Path, current: &Path) -> Result<Raster, Box<dyn std::error::Error>> {
    let mut files: Vec<PathBuf> = Vec::new();
    let dir = icons_dir.join("design/candidates");
    if dir.is_dir() {
        for entry in std::fs::read_dir(&dir)? {
            let path = entry?.path();
            if path.extension().map(|ext| ext == "svg").unwrap_or(false) {
                files.push(path);
            }
        }
    }
    files.sort();
    if files.is_empty() {
        return Err(format!("{} 下没有候选 SVG", dir.display()).into());
    }

    const CELL: u32 = 320;
    const BIG: u32 = 256;
    let sheet_w = CELL * files.len() as u32;
    let sheet_h = BIG + 170;
    let mut sheet = vec![0u8; (sheet_w * sheet_h * 4) as usize];
    for pixel in sheet.chunks_exact_mut(4) {
        pixel.copy_from_slice(&[246, 246, 248, 255]);
    }

    for (column, path) in files.iter().enumerate() {
        let svg = std::fs::read_to_string(path)?;
        let base_x = column as u32 * CELL;
        let big = rasterize(&svg, BIG)?;
        blit(&mut sheet, sheet_w, &big, base_x + (CELL - BIG) / 2, 12);
        let mut x = base_x + 34;
        for size in [64u32, 32, 16] {
            let small = rasterize(&svg, size)?;
            blit(&mut sheet, sheet_w, &small, x, BIG + 30 + (64 - size) / 2);
            x += size + 26;
        }
        // 当前正式图标的那一款画一条粉色下划线
        if path == current {
            let y = BIG + 120;
            for dy in 0..7 {
                for dx in 0..70 {
                    let px = base_x + (CELL - 70) / 2 + dx;
                    let py = y + dy;
                    let idx = ((py * sheet_w + px) * 4) as usize;
                    if idx + 4 <= sheet.len() {
                        sheet[idx..idx + 4].copy_from_slice(&[236, 111, 155, 255]);
                    }
                }
            }
        }
    }
    Ok(Raster { width: sheet_w, height: sheet_h, rgba: sheet })
}

/// 把一张 RGBA 图贴到另一张上（左上角 `(x0, y0)`，按 alpha 混合）
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
