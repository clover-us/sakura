//! 对话输入窗（M3）：宠物右上角的一行输入框。
//!
//! ## 为什么是独立小窗，而不是画在宠物窗里
//!
//! 上游把输入框画在宠物窗内（它给宠物窗留了 0.5 的外扩余量）。本项目**没有**这个空间：
//! 宠物窗的尺寸严格等于宠物包围盒（`WINDOW_MARGIN_RATIO = 0`，这是 M1 的刻意决定），
//! 而且在命中区之外整窗是**点击穿透**的——放在那里的输入框根本点不动。
//!
//! ## 它是本项目唯一**可聚焦**的辅助窗
//!
//! 宠物窗 / 气泡窗 / 右键菜单 / 托盘菜单统统 `focusable(false)`（绝不抢焦点，见各处注释）。
//! 输入框必须能收到键盘，所以这里是唯一的例外：`focusable(true)` + 打开时主动 `set_focus()`。
//! 这条差别是有意的，不是漏改；关掉窗口就交还焦点。
//!
//! ## 交互（照抄上游）
//!
//! 回车发送、Esc 关闭、失败**红字留在框内**（不弹气泡、不静默）；**回复不在这里显示**，
//! 而是交给宠物页的气泡——与碎碎念同一条链路（`whisper::emit`）。

use tauri::{AppHandle, Manager, PhysicalPosition, PhysicalSize, WebviewUrl, WebviewWindowBuilder};

use crate::model::Vec2;
use crate::watchdog;

/// 窗口标签前缀（与宠物窗 `pet-*`、气泡 `bubble-*`、菜单 `menu-*`、托盘菜单区分开）
pub const CHAT_LABEL_PREFIX: &str = "chat-";
/// 输入窗尺寸（够放一行中文字 + 发送提示；输入框内部自增高，窗口不变）
const WIDTH: f64 = 380.0;
const HEIGHT: f64 = 54.0;

/// 输入窗标签
pub fn label_for(pet_label: &str) -> String {
    format!("{CHAT_LABEL_PREFIX}{pet_label}")
}

/// 启动时预建并隐藏（与气泡/菜单同一考虑：点了就出现，不用等 WebView2 建窗）
pub fn prepare(app: &AppHandle, pet_label: &str) -> Result<(), String> {
    let label = label_for(pet_label);
    if app.get_webview_window(&label).is_some() {
        return Ok(());
    }
    let window = WebviewWindowBuilder::new(app, &label, WebviewUrl::App(page_url(pet_label).into()))
        // 标题在无边框窗口上看不见，但窗口枚举/截图探针要靠它认人
        .title("whale-pet 对话")
        .inner_size(WIDTH, HEIGHT)
        .position(0.0, 0.0)
        .transparent(true)
        .decorations(false)
        .shadow(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .resizable(false)
        .maximizable(false)
        .minimizable(false)
        // **唯一的可聚焦辅助窗**：要打字（见模块注释）
        .focusable(true)
        .visible(false)
        .on_page_load(|_window, payload| {
            eprintln!("[whale-pet] 对话窗页面加载事件 {:?} url={}", payload.event(), payload.url());
        })
        .build()
        .map_err(|e| format!("创建对话窗失败 {label}：{e}"))?;
    window.hide().map_err(|e| format!("预备对话窗隐藏失败 {label}：{e}"))?;
    eprintln!("[whale-pet] 对话窗已创建（隐藏）{label}：{WIDTH}×{HEIGHT}");
    Ok(())
}

/// 打开某只宠物的输入框：摆到它右上角、显示、聚焦、清空上一次的输入
pub fn open(app: &AppHandle, pet_label: &str) -> Result<(), String> {
    let label = label_for(pet_label);
    let window = app
        .get_webview_window(&label)
        .ok_or_else(|| format!("对话窗不存在（{label}），prepare 没跑到？"))?;

    if let Some(origin) = anchor_origin(app, pet_label) {
        window
            .set_size(PhysicalSize::new(WIDTH, HEIGHT))
            .map_err(|e| format!("设置对话窗尺寸失败：{e}"))?;
        window
            .set_position(PhysicalPosition::new(origin.x, origin.y))
            .map_err(|e| format!("设置对话窗位置失败：{e}"))?;
        // 摆位这件事必须"能对上账"：把输入量（宠物包围盒 / 命中区 / 工作区）与结果一起打出来，
        // 否则"输入框出现在奇怪的地方"只能靠猜（截图核对时就遇到过一次）。
        eprintln!(
            "[whale-pet] 对话窗摆位 {label}：原点=({:.0},{:.0}) 尺寸={WIDTH}×{HEIGHT}",
            origin.x, origin.y
        );
    }
    window.show().map_err(|e| format!("显示对话窗失败：{e}"))?;
    // 抢焦点是这里**特意**做的：用户点了"说两句"，接下来就要打字
    if let Err(err) = window.set_focus() {
        eprintln!("[whale-pet] 对话窗聚焦失败（仍可点击输入框）：{err}");
    }
    let script = "(function () { var c = window.__whalePetChat; return c ? c.focusInput() : 'not-ready'; })()";
    if let Err(err) = window.eval(script) {
        eprintln!("[whale-pet] 对话窗聚焦注入失败：{err}");
    }
    Ok(())
}

/// 关闭（只隐藏窗口，页面状态由页面自己在下次 `focusInput` 时重置）
pub fn close(app: &AppHandle, pet_label: &str) -> Result<(), String> {
    let label = label_for(pet_label);
    if let Some(window) = app.get_webview_window(&label) {
        window.hide().map_err(|e| format!("隐藏对话窗失败 {label}：{e}"))?;
    }
    Ok(())
}

/// 页面 URL（把宠物标签与显示名带进去；宠物 id / 名字变了就重建窗口，见 reload）
fn page_url(pet_label: &str) -> String {
    format!("chat.html?label={pet_label}")
}

/// 摆位：贴在宠物**命中区**的右上角，再夹进它所在那一块屏的工作区。
///
/// 用命中区而不是包围盒：命中区才是"看得见的身体"（包围盒里那一圈是透明像素）。
fn anchor_origin(app: &AppHandle, pet_label: &str) -> Option<Vec2> {
    let state = app.state::<crate::state::AppState>();
    let (box_origin, hit_box) = {
        let pets = watchdog::timed_lock(&state.pets, "pets（对话窗摆位）");
        let runtime = pets.get(pet_label)?;
        (runtime.state.box_origin(), runtime.config.hit_box)
    };

    // 命中区右上角再往右 8px；纵向让输入框底边压在命中区顶边上方 8px（像挂在宠物头顶）
    let right = box_origin.x + hit_box.x + hit_box.width;
    let top = box_origin.y + hit_box.y;
    let mut origin = Vec2 { x: (right + 8.0).round(), y: (top - HEIGHT - 8.0).round() };

    let center = Vec2 { x: right, y: top };
    let area = crate::display::work_area_containing(app, center)
        .or_else(|| crate::display::primary_work_area(app))?;
    let max_x = (area.x + area.width - WIDTH).max(area.x);
    let max_y = (area.y + area.height - HEIGHT).max(area.y);
    origin.x = origin.x.clamp(area.x, max_x);
    origin.y = origin.y.clamp(area.y, max_y);
    eprintln!(
        "[whale-pet] 对话窗摆位输入：包围盒=({:.0},{:.0}) 命中区=({:.0},{:.0} {:.0}×{:.0}) 期望=({:.0},{:.0}) 夹取后=({:.0},{:.0}) 工作区=({:.0},{:.0} {:.0}×{:.0})",
        box_origin.x,
        box_origin.y,
        hit_box.x,
        hit_box.y,
        hit_box.width,
        hit_box.height,
        right + 8.0,
        top - HEIGHT - 8.0,
        origin.x,
        origin.y,
        area.x,
        area.y,
        area.width,
        area.height
    );
    Some(origin)
}
