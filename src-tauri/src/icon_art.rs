//! 应用图标的**绘制代码**（不是位图素材）。
//!
//! ## 为什么图标是"代码画的"而不是一张 png
//!
//! 图标有两个消费方，尺寸要求完全不同：
//!   - **exe / 任务栏 / 安装包**：需要一个多尺寸 `.ico`（16/24/32/48/64/128/256），
//!     由 `examples/make-icon.rs` 调本模块渲染后写盘；
//!   - **系统托盘**：Tauri 需要运行时的一份 RGBA 缓冲（`tauri::image::Image`），
//!     同样调本模块现渲染（`Detail::Tray` 会省掉小尺寸看不清的细节）。
//!
//! 如果图标是一张位图，托盘那份就得要么再带一个 png 解码器、要么把二进制块塞进仓库；
//! 用绘制代码则**只有一个事实来源**：改一个常量，两处同时变。
//!
//! ## 画法
//!
//! 全部用**有符号距离场（SDF）+ 解析抗锯齿**：每个图元给出 `d(x, y)`（归一化坐标，
//! 负值在内部），覆盖率取 `clamp(0.5 - d*size, 0, 1)`——等于在像素中心做 1px 宽的
//! 线性过渡带，边缘因此是干净的抗锯齿，不需要超采样（1024×1024 超采样 4 倍要吃 268MB）。
//! 平滑并集（`smin`）用来把鲸鱼的躯干/尾鳍/胸鳍焊成一个整体，避免小尺寸下出现接缝阴影。

/// 图标画布的逻辑边长（`render` 的实际边长由调用方给）
pub const DESIGN_SIZE: f32 = 1.0;

/// 图标配色方案（用来对比挑选；产品里只用其中一种）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Style {
    /// 玫瑰粉渐变底 + 白色小鲸鱼（呼应参考图的粉色强调色）
    Rose,
    /// 深蓝紫底 + 白色小鲸鱼
    Midnight,
    /// 天蓝渐变底 + 白色小鲸鱼
    Aqua,
}

impl Style {
    /// 供文件名/日志使用的短标识
    pub fn id(self) -> &'static str {
        match self {
            Style::Rose => "rose",
            Style::Midnight => "midnight",
            Style::Aqua => "aqua",
        }
    }

    /// 全部候选（供生成工具批量输出）
    pub fn all() -> [Style; 3] {
        [Style::Rose, Style::Midnight, Style::Aqua]
    }
}

/// 细节层级
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Detail {
    /// 完整版（大尺寸：气泡、嘴、眼睛高光都在）
    Full,
    /// 托盘版（16~32px 用）：省掉气泡与嘴，眼睛放大——小尺寸下只留"能认出来的特征"
    Tray,
}

/// 一张 RGBA8 位图（直通 alpha）
pub struct Raster {
    pub width: u32,
    pub height: u32,
    /// `width * height * 4`，行优先
    pub rgba: Vec<u8>,
}

/// 颜色（0~1 直通 alpha）
type Color = [f32; 4];

fn rgb(value: u32, alpha: f32) -> Color {
    [
        ((value >> 16) & 0xff) as f32 / 255.0,
        ((value >> 8) & 0xff) as f32 / 255.0,
        (value & 0xff) as f32 / 255.0,
        alpha,
    ]
}

fn mix(a: Color, b: Color, t: f32) -> Color {
    let t = t.clamp(0.0, 1.0);
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
        a[3] + (b[3] - a[3]) * t,
    ]
}

/// 线性插值（用作梯度的 t）
fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t.clamp(0.0, 1.0)
}

// ---------------------------------------------------------------------------
//  SDF 图元（归一化坐标；返回值也是归一化长度，由调用方乘画布边长换算成像素）
// ---------------------------------------------------------------------------

fn sd_circle(cx: f32, cy: f32, r: f32) -> impl Fn(f32, f32) -> f32 {
    move |x, y| ((x - cx).powi(2) + (y - cy).powi(2)).sqrt() - r
}

/// 椭圆（用 `|p/r| - 1` 的常用近似：在边界附近足够准，且处处连续）
fn sd_ellipse(cx: f32, cy: f32, rx: f32, ry: f32) -> impl Fn(f32, f32) -> f32 {
    move |x, y| {
        let dx = (x - cx) / rx;
        let dy = (y - cy) / ry;
        ((dx * dx + dy * dy).sqrt() - 1.0) * rx.min(ry)
    }
}

/// 旋转椭圆（`deg` 为角度，顺时针为正，因为屏幕坐标 y 向下）
fn sd_ellipse_rot(cx: f32, cy: f32, rx: f32, ry: f32, deg: f32) -> impl Fn(f32, f32) -> f32 {
    let rad = deg.to_radians();
    let (sin, cos) = rad.sin_cos();
    move |x, y| {
        let px = x - cx;
        let py = y - cy;
        // 反向旋转到椭圆本地坐标系
        let lx = px * cos + py * sin;
        let ly = -px * sin + py * cos;
        let dx = lx / rx;
        let dy = ly / ry;
        ((dx * dx + dy * dy).sqrt() - 1.0) * rx.min(ry)
    }
}

/// 圆角矩形（精确 SDF）
fn sd_rounded_rect(cx: f32, cy: f32, hw: f32, hh: f32, r: f32) -> impl Fn(f32, f32) -> f32 {
    move |x, y| {
        let qx = (x - cx).abs() - (hw - r);
        let qy = (y - cy).abs() - (hh - r);
        let ax = qx.max(0.0);
        let ay = qy.max(0.0);
        (ax * ax + ay * ay).sqrt() + qx.max(qy).min(0.0) - r
    }
}

/// 胶囊（线段 + 半径）：画嘴、水柱这类"细长条"用
fn sd_capsule(ax: f32, ay: f32, bx: f32, by: f32, r: f32) -> impl Fn(f32, f32) -> f32 {
    move |x, y| {
        let pax = x - ax;
        let pay = y - ay;
        let bax = bx - ax;
        let bay = by - ay;
        let denom = bax * bax + bay * bay;
        let h = if denom <= f32::EPSILON { 0.0 } else { ((pax * bax + pay * bay) / denom).clamp(0.0, 1.0) };
        let dx = pax - bax * h;
        let dy = pay - bay * h;
        (dx * dx + dy * dy).sqrt() - r
    }
}

/// 把某个 SDF 绕 `(cx, cy)` 旋转 `deg` 度（顺时针为正）。
///
/// 用途：让鲸鱼整体带一点上浮的倾斜姿态（纯水平摆放会显得呆）。
fn rotated<F: Fn(f32, f32) -> f32>(cx: f32, cy: f32, deg: f32, inner: F) -> impl Fn(f32, f32) -> f32 {
    let rad = deg.to_radians();
    let (sin, cos) = rad.sin_cos();
    move |x, y| {
        let px = x - cx;
        let py = y - cy;
        let rx = px * cos - py * sin;
        let ry = px * sin + py * cos;
        inner(cx + rx, cy + ry)
    }
}

/// 把某个 SDF 平移 `(dx, dy)`（用于整体微调构图重心）
fn shifted<F: Fn(f32, f32) -> f32>(dx: f32, dy: f32, inner: F) -> impl Fn(f32, f32) -> f32 {
    move |x, y| inner(x - dx, y - dy)
}

/// 月牙形（外圆减内圆，再夹到一个矩形里）：用来画"上翘的嘴"
fn sd_crescent(
    cx: f32,
    cy: f32,
    r_out: f32,
    r_in: f32,
    clip_cx: f32,
    clip_cy: f32,
    clip_hw: f32,
    clip_hh: f32,
) -> impl Fn(f32, f32) -> f32 {
    let outer = sd_circle(cx, cy, r_out);
    let inner = sd_circle(cx, cy, r_in);
    let clip = sd_rounded_rect(clip_cx, clip_cy, clip_hw, clip_hh, 0.004);
    move |x, y| outer(x, y).max(-inner(x, y)).max(clip(x, y))
}

/// 平滑并集：把两个形状"焊"在一起（k 越大焊得越圆滑）
fn smin(a: f32, b: f32, k: f32) -> f32 {
    let h = (0.5 + 0.5 * (b - a) / k).clamp(0.0, 1.0);
    lerp(b, a, h) - k * h * (1.0 - h)
}

// ---------------------------------------------------------------------------
//  画布
// ---------------------------------------------------------------------------

struct Canvas {
    size: u32,
    /// 直通 alpha 的累加缓冲（0~1）
    px: Vec<Color>,
}

impl Canvas {
    fn new(size: u32) -> Self {
        Canvas { size, px: vec![[0.0; 4]; (size * size) as usize] }
    }

    /// 按 SDF 覆盖一层颜色。
    ///
    /// `color_at` 让同一个形状内部可以有渐变（底色就是靠它做的）。
    /// `softness` 是边缘过渡带宽度（像素）：普通图元给 1.0（抗锯齿），
    /// 阴影这类需要"虚边"的给几十像素。
    fn blend<F, C>(&mut self, sdf: F, color_at: C, softness: f32)
    where
        F: Fn(f32, f32) -> f32,
        C: Fn(f32, f32) -> Color,
    {
        let size = self.size as f32;
        for iy in 0..self.size {
            for ix in 0..self.size {
                let x = (ix as f32 + 0.5) / size;
                let y = (iy as f32 + 0.5) / size;
                let d_px = sdf(x, y) * size;
                let coverage = (0.5 - d_px / softness.max(0.0001)).clamp(0.0, 1.0);
                if coverage <= 0.0 {
                    continue;
                }
                let src = color_at(x, y);
                let a = coverage * src[3];
                if a <= 0.0 {
                    continue;
                }
                let dst = &mut self.px[(iy * self.size + ix) as usize];
                for c in 0..3 {
                    dst[c] = dst[c] * (1.0 - a) + src[c] * a;
                }
                dst[3] = dst[3] * (1.0 - a) + a;
            }
        }
    }

    fn into_raster(self) -> Raster {
        let mut rgba = Vec::with_capacity(self.px.len() * 4);
        for p in self.px {
            for c in 0..3 {
                rgba.push((p[c].clamp(0.0, 1.0) * 255.0).round() as u8);
            }
            rgba.push((p[3].clamp(0.0, 1.0) * 255.0).round() as u8);
        }
        Raster { width: self.size, height: self.size, rgba }
    }
}

// ---------------------------------------------------------------------------
//  图标本体
// ---------------------------------------------------------------------------

/// 渲染一张 `size × size` 的图标
pub fn render(size: u32, style: Style, detail: Detail) -> Raster {
    let mut canvas = Canvas::new(size);

    // ---- 底色方块 ----
    // 轻微内缩（0.955）让图标在任务栏/托盘里不贴边，视觉上更"站得住"
    let (top, bottom, whale, blush, ink) = palette(style);
    let tile = sd_rounded_rect(0.5, 0.5, 0.5, 0.5, 0.235);
    canvas.blend(
        tile,
        |x, y| {
            // 对角线渐变 + 左上角一点高光，避免大片纯色显得平
            let t = lerp(0.0, 1.0, (x * 0.55 + y * 0.75 - 0.12) / 0.95);
            let base = mix(top, bottom, t);
            let glow = (1.0 - ((x - 0.30).powi(2) + (y - 0.18).powi(2)).sqrt() / 0.62).clamp(0.0, 1.0);
            mix(base, [1.0, 1.0, 1.0, 1.0], glow * 0.16)
        },
        1.0,
    );

    // ---- 鲸鱼落影（虚边椭圆，给一点"浮着"的体积感）----
    canvas.blend(sd_ellipse(0.500, 0.812, 0.238, 0.038), |_, _| [0.0, 0.0, 0.0, 0.13], 18.0);

    // ---- 鲸鱼本体 ----
    //
    // 让它一眼是**鲸**而不是鱼，靠三件事（都是画了三版才补上的）：
    //   ① 尾鳍是**横向宽扁的两叶**（鱼尾是竖向高瘦的分叉，这是最要命的差别）；
    //      两叶之间留出缺口，正好形成鲸尾那个中缝；
    //   ② 背鳍要真的凸出躯干轮廓（第二版被并进身体里，白做了）；
    //   ③ 整体带一点上浮倾角，别像标本一样平放。
    const TILT: f32 = -7.0;
    let body = sd_ellipse(0.412, 0.558, 0.272, 0.206);
    // 尾柄（细而略上翘）+ 两片横向尾叶。
    // 尾柄 0.052 是调出来的：更细会让尾叶看着"掉"在身体外面，更粗则失去鲸尾的细腰。
    const SHIFT_X: f32 = 0.012;
    const SHIFT_Y: f32 = 0.020;
    let tail_stock = sd_capsule(0.575, 0.556, 0.672, 0.578, 0.052);
    // 托盘版（16~32px）把尾叶加粗：细尾叶在 16px 会被抗锯齿抹成一团，"鲸尾"这个特征就没了
    let fluke_ry = if detail == Detail::Tray { 0.066 } else { 0.048 };
    let fluke_upper = sd_ellipse_rot(0.796, 0.508, 0.142, fluke_ry, -19.0);
    let fluke_lower = sd_ellipse_rot(0.796, 0.630, 0.142, fluke_ry, 19.0);
    let dorsal = sd_ellipse_rot(0.474, 0.330, 0.072, 0.034, -22.0);
    let flipper = sd_ellipse_rot(0.452, 0.752, 0.098, 0.038, 24.0);
    canvas.blend(
        rotated(0.50, 0.56, TILT, shifted(SHIFT_X, SHIFT_Y, move |x, y| {
            let mut d = body(x, y);
            d = smin(d, tail_stock(x, y), 0.030);
            d = smin(d, fluke_upper(x, y), 0.024);
            d = smin(d, fluke_lower(x, y), 0.024);
            d = smin(d, dorsal(x, y), 0.020);
            d = smin(d, flipper(x, y), 0.020);
            d
        })),
        |_, _| whale,
        1.0,
    );

    // ---- 五官（脸部三件套：眼睛 + 上翘的嘴 + 腮红）----
    //
    // 脸部整体跟着躯干一起倾，否则倾斜的身体配一张正着的脸会显得歪。
    let (eye_x, eye_y, eye_r) = match detail {
        Detail::Full => (0.302, 0.512, 0.034),
        // 托盘版：16px 下五官只剩"一个点"，所以把眼睛放大、其余全省（见 candidate_sheet）
        Detail::Tray => (0.300, 0.516, 0.044),
    };
    canvas.blend(
        rotated(0.50, 0.56, TILT, shifted(SHIFT_X, SHIFT_Y, sd_circle(eye_x, eye_y, eye_r))),
        |_, _| ink,
        1.0,
    );
    if detail == Detail::Full {
        canvas.blend(
            rotated(
                0.50,
                0.56,
                TILT,
                shifted(SHIFT_X, SHIFT_Y, sd_circle(eye_x - 0.013, eye_y - 0.014, 0.011)),
            ),
            |_, _| [1.0, 1.0, 1.0, 0.95],
            1.0,
        );
        // 腮红：嘴的左下侧（先画，让嘴压在它上面）
        canvas.blend(
            rotated(
                0.50,
                0.56,
                TILT,
                shifted(SHIFT_X, SHIFT_Y, sd_ellipse(0.238, 0.652, 0.055, 0.036)),
            ),
            |_, _| blush,
            5.0,
        );
        // 嘴：外圆减内圆再夹出下缘 → 一段上翘的弧（比直线有表情）
        canvas.blend(
            rotated(
                0.50,
                0.56,
                TILT,
                shifted(
                    SHIFT_X,
                    SHIFT_Y,
                    sd_crescent(0.286, 0.470, 0.150, 0.136, 0.286, 0.610, 0.060, 0.018),
                ),
            ),
            |_, _| [ink[0], ink[1], ink[2], 0.88],
            1.0,
        );
        // 气泡：从头顶往左上飘（放右边会和加大的尾鳍打架）
        canvas.blend(sd_circle(0.190, 0.222, 0.031), |_, _| [1.0, 1.0, 1.0, 0.92], 1.0);
        canvas.blend(sd_circle(0.120, 0.146, 0.018), |_, _| [1.0, 1.0, 1.0, 0.76], 1.0);
    } else {
        canvas.blend(
            rotated(
                0.50,
                0.56,
                TILT,
                shifted(SHIFT_X, SHIFT_Y, sd_ellipse(0.238, 0.652, 0.055, 0.036)),
            ),
            |_, _| blush,
            4.0,
        );
    }

    canvas.into_raster()
}

/// 各方案的颜色表：`(底色上, 底色下, 鲸鱼, 腮红, 五官)`
fn palette(style: Style) -> (Color, Color, Color, Color, Color) {
    match style {
        Style::Rose => (
            rgb(0xFFC2DB, 1.0),
            rgb(0xF8749F, 1.0),
            rgb(0xFFFDFE, 1.0),
            rgb(0xFF8CB4, 0.85),
            rgb(0x23304A, 1.0),
        ),
        Style::Midnight => (
            rgb(0x4C5CC4, 1.0),
            rgb(0x2A2360, 1.0),
            rgb(0xFFFDFE, 1.0),
            rgb(0xFF9EC4, 0.85),
            rgb(0x1B2140, 1.0),
        ),
        Style::Aqua => (
            rgb(0x8FE3FF, 1.0),
            rgb(0x3E8BF0, 1.0),
            rgb(0xFFFDFE, 1.0),
            rgb(0xFF9EC4, 0.80),
            rgb(0x1B2A4A, 1.0),
        ),
    }
}

/// 托盘用的 RGBA（32×32 托盘版）：给 `tauri::image::Image` 直接用
///
/// 返回 `(像素, 宽, 高)`；像素是行优先的 RGBA8，Tauri 的 `Image::new_owned` 直接吃。
pub fn tray_rgba(size: u32) -> (Vec<u8>, u32, u32) {
    let raster = render(size, Style::Rose, Detail::Tray);
    (raster.rgba, raster.width, raster.height)
}
