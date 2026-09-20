//! 图标生成工具（**开发期工具，不属于应用运行路径**）。
//!
//! 用法：
//!
//! ```powershell
//! cd src-tauri
//! cargo run --example make-icon                       # 用 design/app-icon.png 生成 icons/ 全套 + 前端 logo
//! cargo run --example make-icon -- --png <256px.png>  # 换一张位图源
//! cargo run --example make-icon -- --svg design/candidates/b-face.svg
//! cargo run --example make-icon -- --sheet            # 只出候选对比图，不动正式图标
//! ```
//!
//! ## 图形源：现在是**位图**（用户给的素材）
//!
//! 第一版图标是**用 Rust 画 SDF 图元**拼出来的（`icon_art.rs`），观感上限很低——
//! 用户看完的评价就是"图标丑"，于是改成"矢量 SVG 是唯一源"（`design/*.svg` + resvg）。
//!
//! 2026-09-21 用户又给了新的应用图标（原子轨道，**两种样式各 4 个尺寸的 PNG**，没有矢量源），
//! 要求换掉全套。因此本工具多了一条位图通道：`--png` 吃一张 PNG，自己做重采样。
//! 取舍写在明处——**位图源改不动形状**，好处是"与用户给的素材逐像素一致"。
//!
//! 小尺寸清晰靠两件事：
//!
//!   1. **能用原图就用原图**：用户给的就是 16/32/64/256 四个尺寸各一张，
//!      所以同目录下的 `app-icon-<尺寸>.png` 一旦存在就直接吃进来（托盘用到的 32px
//!      正是其中一张），只有缺的尺寸才自己算。重采样出来的 16px 会比原图淡一截——
//!      细笔画的 alpha 被平均掉了，实测对比见 `docs/VERIFICATION.md` 第 22 节；
//!   2. 自己算的那些：缩小走**面积平均**（256 → 24 这种非整数倍也一样），
//!      颜色在**线性光**里平均（sRGB 直接平均会把淡蓝轨道压暗、彩色电子变脏），
//!      放大走双线性（只用于 1024 的 `icon.png`，Windows 用不到它）。
//!
//! ## 产物
//!
//!   - `icons/icon.ico`：16/24/32/48/64/128/256 七个尺寸（exe、任务栏、安装包都用它）
//!   - `icons/icon.png`（1024）、`icons/32x32.png`、`icons/128x128.png`、`icons/128x128@2x.png`
//!   - `icons/tray-32.png`（给人看）与 `icons/tray-32.rgba`（给应用用，裸 RGBA8）
//!   - `../src/assets/app-logo.png`：**前端两处页头 logo** 用的 128px 位图
//!     （设置侧栏与托盘菜单页头；页面里显示 22~26px，留足 HiDPI 余量。
//!     以前这里是手写的内联 SVG，与 `design/app-icon.svg` 手工对齐——换成位图源之后
//!     手工对齐不再可靠，改成直接引用本工具产出的同一张图）
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

/// 前端页头 logo 的产物边长（页面里显示 22~26px）
const LOGO_SIZE: u32 = 128;

struct Raster {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

/// 图形源：SVG（矢量，手写）或 PNG（位图，用户给的素材）
enum Art {
    Svg(String),
    Png(Raster),
}

impl Art {
    /// 按目标边长产出 RGBA
    fn render(&self, size: u32) -> Result<Raster, Box<dyn std::error::Error>> {
        match self {
            Art::Svg(svg) => render_svg(svg, size),
            Art::Png(raster) => Ok(resample(raster, size)),
        }
    }
}

/// 图标源：主图 + 若干"尺寸精确"的帧（见文件头的说明）
struct IconSource {
    main: Art,
    exact: Vec<(u32, Art)>,
}

impl IconSource {
    /// `path` 是主图（最大尺寸那张）；同目录下的 `<stem>-<尺寸>.png` 视为该尺寸的原图
    fn load(path: &Path, sizes: &[u32]) -> Result<Self, Box<dyn std::error::Error>> {
        let main = load_art(path)?;
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("app-icon").to_string();
        let mut exact = Vec::new();
        for size in sizes {
            let candidate = path.with_file_name(format!("{stem}-{size}.png"));
            if !candidate.is_file() {
                continue;
            }
            let art = load_art(&candidate)?;
            let ok = match &art {
                Art::Png(raster) => raster.width == *size && raster.height == *size,
                Art::Svg(_) => false,
            };
            if ok {
                println!("  原图帧：{} → {size}px", candidate.file_name().unwrap_or_default().to_string_lossy());
                exact.push((*size, art));
            } else {
                println!("  跳过 {}：不是 {size}×{size} 的位图", candidate.display());
            }
        }
        Ok(IconSource { main, exact })
    }

    fn render(&self, size: u32) -> Result<Raster, Box<dyn std::error::Error>> {
        for (exact_size, art) in &self.exact {
            if *exact_size == size {
                return art.render(size);
            }
        }
        self.main.render(size)
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let icons_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("icons");
    let mut source = icons_dir.join("design/app-icon.png");
    let mut sheet_only = false;

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            // 两个开关都只是"换一个源"，吃 SVG 还是 PNG 由**扩展名**决定
            "--svg" | "--png" => {
                if let Some(path) = args.next() {
                    let candidate = PathBuf::from(&path);
                    source = if candidate.is_absolute() { candidate } else { icons_dir.join(candidate) };
                }
            }
            "--sheet" => sheet_only = true,
            other => return Err(format!("未知参数：{other}（可用 --svg/--png <path> / --sheet）").into()),
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
    let art = IconSource::load(&source, &ICO_SIZES)?;
    println!("图标源：{}", source.display());

    let master = art.render(1024)?;
    write_png(&icons_dir.join("icon.png"), &master)?;
    for (name, size) in [("32x32.png", 32u32), ("128x128.png", 128), ("128x128@2x.png", 256)] {
        write_png(&icons_dir.join(name), &art.render(size)?)?;
    }

    // 多尺寸 ico：`encode` 会按尺寸自动挑 PNG 还是 BMP 存储
    let mut ico = ico::IconDir::new(ico::ResourceType::Icon);
    for size in ICO_SIZES {
        let raster = art.render(size)?;
        let image = ico::IconImage::from_rgba_data(raster.width, raster.height, raster.rgba);
        ico.add_entry(ico::IconDirEntry::encode(&image)?);
    }
    ico.write(&mut BufWriter::new(File::create(icons_dir.join("icon.ico"))?))?;

    // 托盘：PNG 给人看，RGBA 裸数据给应用读（应用不背图像解码器）
    let tray = art.render(TRAY_SIZE)?;
    write_png(&icons_dir.join("tray-32.png"), &tray)?;
    std::fs::write(icons_dir.join("tray-32.rgba"), &tray.rgba)?;
    println!(
        "托盘图标：tray-32.png + tray-32.rgba（{}×{}，{} 字节裸 RGBA）",
        tray.width,
        tray.height,
        tray.rgba.len()
    );

    // 前端两处页头 logo（Vite 会把它当作 src/assets 下的普通资源处理）
    let frontend_logo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../src/assets/app-logo.png");
    if let Some(parent) = frontend_logo.parent() {
        std::fs::create_dir_all(parent)?;
    }
    write_png(&frontend_logo, &art.render(LOGO_SIZE)?)?;
    println!("前端 logo：{}（{LOGO_SIZE}px）", frontend_logo.display());

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

/// 按扩展名读一个图形源（SVG 读文本，PNG 解码成 RGBA）
fn load_art(path: &Path) -> Result<Art, Box<dyn std::error::Error>> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "svg" => Ok(Art::Svg(std::fs::read_to_string(path)?)),
        "png" => Ok(Art::Png(load_png(path)?)),
        other => Err(format!("不认识的图形源扩展名：.{other}（只认 .svg / .png）").into()),
    }
}

/// 解码 PNG 成 RGBA8
fn load_png(path: &Path) -> Result<Raster, Box<dyn std::error::Error>> {
    let decoder = png::Decoder::new(File::open(path)?);
    let mut reader = decoder.read_info()?;
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf)?;
    if info.bit_depth != png::BitDepth::Eight {
        return Err(format!("只支持 8 位 PNG（{} 是 {:?}）", path.display(), info.bit_depth).into());
    }
    let pixels = &buf[..info.buffer_size()];
    let mut rgba = Vec::with_capacity((info.width * info.height * 4) as usize);
    match info.color_type {
        png::ColorType::Rgba => rgba.extend_from_slice(pixels),
        png::ColorType::Rgb => {
            for px in pixels.chunks_exact(3) {
                rgba.extend_from_slice(&[px[0], px[1], px[2], 255]);
            }
        }
        png::ColorType::Grayscale => {
            for px in pixels.chunks_exact(1) {
                rgba.extend_from_slice(&[px[0], px[0], px[0], 255]);
            }
        }
        png::ColorType::GrayscaleAlpha => {
            for px in pixels.chunks_exact(2) {
                rgba.extend_from_slice(&[px[0], px[0], px[0], px[1]]);
            }
        }
        png::ColorType::Indexed => {
            return Err(format!("索引色 PNG 不支持（{}）：先转成 RGBA", path.display()).into())
        }
    }
    Ok(Raster { width: info.width, height: info.height, rgba })
}

/// 位图重采样：缩小走面积平均，放大走双线性（都在线性光里算）
fn resample(src: &Raster, size: u32) -> Raster {
    if size >= src.width {
        upscale_bilinear(src, size)
    } else {
        area_average(src, size)
    }
}

fn srgb_to_linear(v: u8) -> f64 {
    let c = v as f64 / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_srgb(v: f64) -> u8 {
    let c = v.clamp(0.0, 1.0);
    let s = if c <= 0.0031308 { c * 12.92 } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 };
    (s * 255.0).round().clamp(0.0, 255.0) as u8
}

/// 面积平均降采样：每个目标像素 = 覆盖到的源像素按**重叠面积**加权平均。
///
/// 这就是"16 倍降采样仍然清晰"的关键：双线性只会采到源图里的 4 个点，
/// 细笔画（本图标在 256px 下轨道只有 6px 宽）会被整条漏掉或时隐时现。
fn area_average(src: &Raster, size: u32) -> Raster {
    let scale = src.width as f64 / size as f64;
    let mut out = vec![0u8; (size * size * 4) as usize];
    for ty in 0..size {
        let y0 = ty as f64 * scale;
        let y1 = y0 + scale;
        for tx in 0..size {
            let x0 = tx as f64 * scale;
            let x1 = x0 + scale;
            let (mut pr, mut pg, mut pb, mut pa, mut wsum) = (0.0f64, 0.0f64, 0.0f64, 0.0f64, 0.0f64);
            for sy in (y0.floor() as u32)..((y1.ceil() as u32).min(src.height)) {
                let wy = (y1.min(f64::from(sy + 1)) - y0.max(f64::from(sy))).max(0.0);
                if wy <= 0.0 {
                    continue;
                }
                for sx in (x0.floor() as u32)..((x1.ceil() as u32).min(src.width)) {
                    let wx = (x1.min(f64::from(sx + 1)) - x0.max(f64::from(sx))).max(0.0);
                    if wx <= 0.0 {
                        continue;
                    }
                    let w = wx * wy;
                    let idx = ((sy * src.width + sx) * 4) as usize;
                    let alpha = f64::from(src.rgba[idx + 3]) / 255.0;
                    pr += srgb_to_linear(src.rgba[idx]) * alpha * w;
                    pg += srgb_to_linear(src.rgba[idx + 1]) * alpha * w;
                    pb += srgb_to_linear(src.rgba[idx + 2]) * alpha * w;
                    pa += alpha * w;
                    wsum += w;
                }
            }
            let idx = ((ty * size + tx) * 4) as usize;
            if wsum <= 0.0 || pa <= 0.0 {
                continue; // 全透明：保持 0
            }
            out[idx] = linear_to_srgb(pr / pa);
            out[idx + 1] = linear_to_srgb(pg / pa);
            out[idx + 2] = linear_to_srgb(pb / pa);
            out[idx + 3] = (pa / wsum * 255.0).round().clamp(0.0, 255.0) as u8;
        }
    }
    Raster { width: size, height: size, rgba: out }
}

/// 双线性放大（只给 1024 的 `icon.png` 用；预乘 alpha + 线性光）
fn upscale_bilinear(src: &Raster, size: u32) -> Raster {
    let mut out = vec![0u8; (size * size * 4) as usize];
    let step = src.width as f64 / size as f64;
    let max = |v: f64, hi: u32| v.clamp(0.0, f64::from(hi.saturating_sub(1)));
    for ty in 0..size {
        let fy = max((ty as f64 + 0.5) * step - 0.5, src.height);
        let y0 = fy.floor() as u32;
        let y1 = (y0 + 1).min(src.height - 1);
        let wy = fy - f64::from(y0);
        for tx in 0..size {
            let fx = max((tx as f64 + 0.5) * step - 0.5, src.width);
            let x0 = fx.floor() as u32;
            let x1 = (x0 + 1).min(src.width - 1);
            let wx = fx - f64::from(x0);
            let (mut pr, mut pg, mut pb, mut pa) = (0.0f64, 0.0f64, 0.0f64, 0.0f64);
            for (sy, wyy) in [(y0, 1.0 - wy), (y1, wy)] {
                for (sx, wxx) in [(x0, 1.0 - wx), (x1, wx)] {
                    let w = wyy * wxx;
                    let idx = ((sy * src.width + sx) * 4) as usize;
                    let alpha = f64::from(src.rgba[idx + 3]) / 255.0;
                    pr += srgb_to_linear(src.rgba[idx]) * alpha * w;
                    pg += srgb_to_linear(src.rgba[idx + 1]) * alpha * w;
                    pb += srgb_to_linear(src.rgba[idx + 2]) * alpha * w;
                    pa += alpha * w;
                }
            }
            let idx = ((ty * size + tx) * 4) as usize;
            if pa <= 0.0 {
                continue;
            }
            out[idx] = linear_to_srgb(pr / pa);
            out[idx + 1] = linear_to_srgb(pg / pa);
            out[idx + 2] = linear_to_srgb(pb / pa);
            out[idx + 3] = (pa * 255.0).round().clamp(0.0, 255.0) as u8;
        }
    }
    Raster { width: size, height: size, rgba: out }
}

/// 用 resvg 把 SVG 栅格化成指定边长的 RGBA
fn render_svg(svg: &str, size: u32) -> Result<Raster, Box<dyn std::error::Error>> {
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

/// 候选对比图：每个候选一列（256 放大 + 64/32/16 真实像素），
/// "当前采用的那一款"下面画一条粉色标记线。
///
/// 候选既可以是 SVG（手写矢量）也可以是 PNG（用户给的位图素材），按扩展名分派。
/// 标记的判据是**文件内容**而不是路径：正式图标是从某个候选复制过来的，
/// 按路径比永远标记不上（踩过：换成 app-icon 之后标记线消失了）。
fn candidate_sheet(icons_dir: &Path, current: &Path) -> Result<Raster, Box<dyn std::error::Error>> {
    let current_bytes = std::fs::read(current).unwrap_or_default();
    let mut files: Vec<PathBuf> = Vec::new();
    let dir = icons_dir.join("design/candidates");
    if dir.is_dir() {
        for entry in std::fs::read_dir(&dir)? {
            let path = entry?.path();
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
            if ext == "svg" || ext == "png" {
                files.push(path);
            }
        }
    }
    files.sort();
    if files.is_empty() {
        return Err(format!("{} 下没有候选（.svg / .png）", dir.display()).into());
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
        let art = load_art(path)?;
        let base_x = column as u32 * CELL;
        let big = art.render(BIG)?;
        blit(&mut sheet, sheet_w, &big, base_x + (CELL - BIG) / 2, 12);
        let mut x = base_x + 34;
        for size in [64u32, 32, 16] {
            let small = art.render(size)?;
            blit(&mut sheet, sheet_w, &small, x, BIG + 30 + (64 - size) / 2);
            x += size + 26;
        }
        // 当前正式图标的那一款画一条粉色下划线
        if std::fs::read(path).unwrap_or_default() == current_bytes {
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
