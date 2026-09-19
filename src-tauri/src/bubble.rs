//! 气泡窗口：一个**独立、不可聚焦、整窗点击穿透**的小窗，用来显示宠物头顶的文本。
//!
//! ## 为什么要单独开一个窗口
//!
//! M0 把宠物窗口的尺寸压成"严格等于宠物包围盒"——因为窗口只要处于可交互态，
//! **整块窗口矩形都在吃鼠标**，留出透明余量就会挡住下层应用的点击（用户实测反馈过）。
//! 但气泡需要宠物**上方**的空间，而那正是窗口之外的地方。两个选择：
//!   - 显示气泡时临时扩大宠物窗口：那段时间又会挡住点击（把刚修好的问题换回来）；
//!   - **给气泡单独开一个小窗**（本模块）：宠物窗口保持紧致，气泡窗独立存在，
//!     它自己**永远是点击穿透的**，因此不挡任何东西。
//!
//! ## 关键设计
//!
//! - **不可聚焦 + 整窗穿透**：一个纯显示窗，用户永远点不到它，也不会抢焦点；
//! - **不参与命中判定**：与宠物窗相反，气泡窗没有任何"可交互"状态需要翻转；
//! - **按需创建、复用**：第一次显示时创建，之后只 `show` / `set_position`；
//! - **尺寸由文本长度预估**：宿主不测量文字（那要经过页面），按字数估一个够用的高度，
//!   页面里的气泡是 `inline-block`，实际渲染会自适应，四周留白是透明的。
//!
//! ## 定位
//!
//! 气泡窗的**底边中心**对准宠物包围盒的**顶边中心**（气泡底部的尖角指向宠物头顶）。
//! 位置由前端算出并传入（它掌握宠物包围盒的屏幕坐标），宿主只负责摆窗。

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, PhysicalPosition, PhysicalSize, Runtime, WebviewUrl, WebviewWindowBuilder};

use crate::model::Vec2;

/// 气泡窗标签前缀（与宠物窗 `pet-<id>-<n>` 区分开）
pub const BUBBLE_LABEL_PREFIX: &str = "bubble-";

/// 待注入文本表：气泡窗标签 → 最近一次要显示的文本。
///
/// ## 为什么必须留一份"待注入"
///
/// `eval` 与"页面导航完成"之间存在**竞态**：WebView2 的导航是异步的，
/// `build()` 返回时页面可能还停在 `about:blank`，此时脚本里的
/// `document.getElementById('bubble')` 拿到 `null`，文本就被**静默丢掉**——
/// 表现为"气泡窗建出来了、页面也加载了、日志一切正常，但屏幕上什么都没有"。
/// 实测两种时序都出现过（同一份代码，两次运行日志顺序不同）：
///
/// ```text
/// 正常：气泡窗已创建 → 页面加载事件 Started → 气泡窗显示完成（注入）→ Finished
/// 失败：气泡窗已创建 → 气泡窗显示完成（注入！）→ 页面加载事件 Started → Finished
/// ```
///
/// 因此 `show()` 在注入前先把文本记在这里，`on_page_load(Finished)` 再补注一次；
/// 谁先谁后都能落地。隐藏时清掉，避免热重载后凭空冒出一句旧话。
static RECORDS: OnceLock<Mutex<HashMap<String, BubbleRecord>>> = OnceLock::new();

/// 气泡窗的宿主侧记账（每个气泡窗一条）
#[derive(Debug, Clone)]
struct BubbleRecord {
    /// 最近一次要显示的文本（`None` = 已隐藏：页面若重载，不应再补注旧话）
    text: Option<String>,
    /// 气泡**底边中心**相对宠物包围盒原点的偏移。
    ///
    /// 由 `show()` 从"前端给的锚点 − 宿主已知的包围盒原点"算出来并记下，
    /// 之后宠物每移动一次就按它重摆气泡——**头距（气泡离头顶多远）只在前端算一次**，
    /// 宿主不做任何像素判断，两边不会各算一套。
    anchor_dx: f64,
    /// 同上（纵向）
    anchor_dy: f64,
    /// 当前窗口尺寸
    width: f64,
    height: f64,
    /// 最近一次摆到的窗口左上角（拖拽时每帧都会调用 follow，位置没变就别打扰系统）
    origin: Vec2,
}

impl Default for BubbleRecord {
    fn default() -> Self {
        Self {
            text: None,
            anchor_dx: 0.0,
            anchor_dy: 0.0,
            width: 0.0,
            height: 0.0,
            // NaN = "还没摆过"，保证第一次 follow 一定会摆
            origin: Vec2 { x: f64::NAN, y: f64::NAN },
        }
    }
}

/// 取记账表（首次访问时初始化）
fn records() -> &'static Mutex<HashMap<String, BubbleRecord>> {
    RECORDS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 气泡窗左上角：横向"尖角居中"、纵向"尖角正好落在锚点上"。
///
/// 纵向要减掉 [`estimate::ARROW_INSET`]：窗口底部留了投影余量，尖角并不在窗口最底边，
/// 而是在底边往上 `ARROW_INSET` 处——不减这一项，气泡会比预期低十来像素。
fn origin_for(record: &BubbleRecord, box_x: f64, box_y: f64) -> Vec2 {
    Vec2 {
        x: (box_x + record.anchor_dx - record.width / 2.0).round(),
        y: (box_y + record.anchor_dy - record.height + estimate::ARROW_INSET).round(),
    }
}

/// 取某只宠物当前的**包围盒原点**（屏幕坐标）；拿不到返回 `None`。
///
/// 为什么能从宿主侧拿到：宠物窗的几何（原点 + 偏移）本来就记在 `PetRuntime` 里，
/// `set_pet_bounds` 一直在维护它。
fn pet_box_origin<R: Runtime>(app: &AppHandle<R>, pet_label: &str) -> Option<Vec2> {
    let state = app.state::<crate::state::AppState>();
    let pets = crate::watchdog::timed_lock(&state.pets, "pets（气泡跟随）");
    pets.get(pet_label).map(|runtime| runtime.state.box_origin())
}

/// 让气泡**跟着宠物走**：宠物包围盒移动后，用记录里的偏移重摆气泡窗（幂等、便宜）。
///
/// 放在宿主侧而不是前端：宠物窗的每一次移动（拖拽、抛掷、漫游）都会经过
/// `set_pet_bounds`，宿主顺手挪一下气泡最省事；放前端就得每帧多发一条 IPC。
pub fn follow<R: Runtime>(app: &AppHandle<R>, pet_label: &str) {
    let label = format!("{BUBBLE_LABEL_PREFIX}{pet_label}");
    let Some(window) = app.get_webview_window(&label) else {
        return;
    };
    if !window.is_visible().unwrap_or(false) {
        return;
    }
    let Some(box_origin) = pet_box_origin(app, pet_label) else {
        return;
    };
    let Ok(mut map) = records().lock() else {
        return;
    };
    let Some(record) = map.get_mut(&label) else {
        return;
    };
    if record.width <= 0.0 || record.height <= 0.0 {
        return;
    }
    let origin = origin_for(record, box_origin.x, box_origin.y);
    if (origin.x - record.origin.x).abs() < 1.0 && (origin.y - record.origin.y).abs() < 1.0 {
        return;
    }
    record.origin = origin;
    drop(map);
    if let Err(err) = window.set_position(PhysicalPosition::new(origin.x, origin.y)) {
        eprintln!("[whale-pet] 气泡跟随失败 {label}：{err}");
    }
}

/// 组装"把文本写进气泡并淡入"的注入脚本。
///
/// 文本里的引号/反斜杠/换行都要转义，否则会破坏脚本本身（注入的是字符串字面量）。
fn bubble_script(text: &str) -> String {
    let escaped = text
        .replace('\\', "\\\\")
        .replace('\'', "\\'")
        .replace('\n', "\\n")
        .replace('\r', "");
    format!(
        "(function () {{ var b = document.getElementById('bubble'); var t = document.getElementById('bubble-text'); if (!b || !t) return 'no-dom'; t.textContent = '{escaped}'; b.classList.add('is-on'); return 'ok'; }})()"
    )
}

/// 把文本注入气泡页面（best-effort：页面尚未就绪时由 `on_page_load` 兜底补注）
fn inject_text<R: Runtime>(window: &tauri::WebviewWindow<R>, text: &str) {
    if let Err(err) = window.eval(bubble_script(text)) {
        eprintln!("[whale-pet] 气泡文本注入失败 {}：{err}", window.label());
    }
}

/// 气泡窗尺寸估算参数
///
/// 注意"**气泡本体**"与"**窗口**"是两回事：窗口比本体四周各大一圈，
/// 那一圈是留给投影（`box-shadow`）与尖角的**透明余量**。
/// 不留的话投影会被窗口边界切出一条硬边——用户看到的就是"气泡旁边有一块虚影"（实测反馈）。
mod estimate {
    /// 每行大约容纳的字符数（按 15px 字号、约 280px 文本宽估算）
    pub const CHARS_PER_LINE: usize = 16;
    /// 单个字符的平均宽度（px），用于估算本体宽度
    pub const CHAR_WIDTH: f64 = 15.5;
    /// 单行高度（15px 字号 × 1.55 行高 ≈ 23px，取整便于估算）
    pub const LINE_HEIGHT: f64 = 24.0;
    /// 本体的上下内边距 + 描边（px）
    pub const BODY_PAD_Y: f64 = 20.0;
    /// 本体的左右内边距 + 描边（px）
    pub const BODY_PAD_X: f64 = 32.0;
    /// 窗口在**本体左右**各留的透明余量（投影用，px）
    pub const PAD_SIDE: f64 = 14.0;
    /// 窗口在本体**上方**留的透明余量（投影用，px）
    pub const PAD_TOP: f64 = 14.0;
    /// 窗口在本体**下方**留的透明余量（尖角 9px + 投影，px）
    pub const PAD_BOTTOM: f64 = 18.0;
    /// 尖角相对**窗口底边**的位置（px）：窗口底边往上这么多，就是尖角尖端
    pub const ARROW_INSET: f64 = PAD_BOTTOM - 9.0;
    /// 气泡窗允许的最大尺寸（px）——超过就换行，避免超出屏幕
    pub const MAX_WIDTH: f64 = 420.0;
    pub const MAX_HEIGHT: f64 = 320.0;
    /// 本体最小宽度（px）：一行短句也要有个像样的泡
    pub const MIN_WIDTH: f64 = 120.0;
}

/// 气泡窗请求参数（前端 → 宿主）
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BubbleRequest {
    /// 气泡**尖角**要指向的屏幕坐标（x）——即宠物包围盒顶边中心（前端已按头距下压）
    pub anchor_x: f64,
    /// 同上（y）
    pub anchor_y: f64,
    /// 宠物包围盒原点的屏幕坐标（x）。
    ///
    /// **必须由前端一起给**：宿主虽然也记着包围盒原点，但在"菜单外扩/缩回"期间
    /// 它有一小段时间是不一致的（偏移已改、原点还没跟上）。若宿主在这段时间里自己推算
    /// "锚点相对包围盒的偏移"，算出来的就是错的——之后宠物一动，气泡就会整体错开一个外扩量
    /// （实测过：气泡跑到宠物右下角）。前端手里永远是同一份快照，交给它算最稳。
    pub box_x: f64,
    /// 同上（y）
    pub box_y: f64,
    /// 要显示的文本
    pub text: String,
}

/// 气泡窗状态（宿主侧记账的只读快照；诊断与将来的设置界面用）
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BubbleState {
    /// 是否正在显示
    pub visible: bool,
    /// 当前窗口左上角（屏幕坐标）
    pub origin: Vec2,
    /// 当前窗口尺寸
    pub size: crate::model::Size,
}

/// 按文本长度估算**窗口**尺寸（本体 + 四周透明余量；页面内是自适应布局，多给一点没关系）
fn estimate_size(text: &str) -> (f64, f64) {
    let chars = text.chars().count().max(1);
    let lines = chars.div_ceil(estimate::CHARS_PER_LINE).max(1);
    // 本体（文字 + 内边距 + 描边）
    let body_height = lines as f64 * estimate::LINE_HEIGHT + estimate::BODY_PAD_Y;
    let body_width =
        (chars.min(estimate::CHARS_PER_LINE) as f64 * estimate::CHAR_WIDTH + estimate::BODY_PAD_X)
            .clamp(estimate::MIN_WIDTH, estimate::MAX_WIDTH);
    // 窗口 = 本体 + 投影/尖角余量
    (
        body_width + estimate::PAD_SIDE * 2.0,
        (body_height + estimate::PAD_TOP + estimate::PAD_BOTTOM).min(estimate::MAX_HEIGHT),
    )
}

/// 预备气泡窗：**启动时**就把它建出来并隐藏。
///
/// 为什么不是"第一次显示时再建"：WebView2 建窗 + 页面加载要几百毫秒，
/// 用户点了「工具 → 显示一句气泡」之后得愣一下才看到气泡（实测反馈"首次出现很慢"）。
/// 预先建好、保持隐藏，显示时只改尺寸与位置，做到"点了就出现"。
pub fn prepare<R: Runtime>(app: &AppHandle<R>, pet_label: &str) -> Result<(), String> {
    let label = format!("{BUBBLE_LABEL_PREFIX}{pet_label}");
    if app.get_webview_window(&label).is_some() {
        return Ok(());
    }
    let (width, height) = estimate_size("预备");
    let window = create_window(app, &label, width, height, Vec2 { x: 0.0, y: 0.0 })?;
    // 建窗过程（WebView2 初始化）可能把窗口显示出来，这里**显式再藏一次**：
    // 进程外探针实测过"预备窗"会以 (0,0) 148×76 的可见窗口留在桌面上
    // （它是透明的，看不见，但没必要让一个可见窗口挂在那儿）。
    window.hide().map_err(|e| format!("预备气泡窗隐藏失败 {label}：{e}"))?;
    Ok(())
}

/// 建气泡窗（**隐藏**创建；显示由 `show()` 负责）
fn create_window<R: Runtime>(
    app: &AppHandle<R>,
    label: &str,
    width: f64,
    height: f64,
    origin: Vec2,
) -> Result<tauri::WebviewWindow<R>, String> {
    let window = WebviewWindowBuilder::new(app, label, WebviewUrl::App("bubble.html".into()))
        .inner_size(width, height)
        .position(origin.x, origin.y)
        .transparent(true)
        .decorations(false)
        .shadow(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .resizable(false)
        .maximizable(false)
        .minimizable(false)
        // 预先建好但不显示：等 `show()` 改完尺寸/位置再亮出来
        .visible(false)
        // 与宠物窗同样的理由：**绝不抢焦点**
        .focusable(false)
        // 页面就绪后**补注一次**：`show()` 里的注入可能赶在导航之前（见 `RECORDS` 的说明），
        // 而"页面加载完成"是子窗口齐备、DOM 可用的确定时机。
        .on_page_load(|window, payload| {
            eprintln!(
                "[whale-pet] 气泡页面加载事件 {:?} url={}",
                payload.event(),
                payload.url()
            );
            if matches!(payload.event(), tauri::webview::PageLoadEvent::Finished) {
                if let Err(err) = set_click_through(&window) {
                    eprintln!("[whale-pet] {err}");
                }
                // 只有当这个窗口**确实**有待显示文本时才补注（隐藏时会清空）
                let pending = records()
                    .lock()
                    .ok()
                    .and_then(|map| map.get(window.label()).and_then(|r| r.text.clone()));
                if let Some(text) = pending {
                    inject_text(&window, &text);
                }
            }
        })
        .build()
        .map_err(|e| format!("创建气泡窗口失败 {label}：{e}"))?;
    eprintln!("[whale-pet] 气泡窗已创建 {label}：{width}×{height} {}", describe(&window));
    Ok(window)
}

/// 显示气泡：不存在就创建（正常情况下已在启动时预备好），存在就复用。
///
/// 文本经 `eval` 注入页面（页面是纯显示壳，没有自己的逻辑，因此注入是最直接的方式）。
pub fn show<R: Runtime>(
    app: &AppHandle<R>,
    pet_label: &str,
    request: &BubbleRequest,
) -> Result<(), String> {
    let label = format!("{BUBBLE_LABEL_PREFIX}{pet_label}");
    let (width, height) = estimate_size(&request.text);
    // 记账：把锚点换算成"相对宠物包围盒的偏移"存下来，之后宠物每次移动都用它重摆气泡。
    // 包围盒原点**用请求里带来的那一份**（前端快照），理由见 BubbleRequest::box_x 的注释。
    let record = BubbleRecord {
        text: Some(request.text.clone()),
        anchor_dx: request.anchor_x - request.box_x,
        anchor_dy: request.anchor_y - request.box_y,
        width,
        height,
        origin: Vec2 { x: f64::NAN, y: f64::NAN },
    };
    let origin = origin_for(&record, request.box_x, request.box_y);
    if let Ok(mut map) = records().lock() {
        map.insert(label.clone(), record);
    }

    let window = match app.get_webview_window(&label) {
        Some(existing) => existing,
        None => create_window(app, &label, width, height, origin)?,
    };

    window
        .set_size(PhysicalSize::new(width, height))
        .map_err(|e| format!("设置气泡尺寸失败 {label}：{e}"))?;
    window
        .set_position(PhysicalPosition::new(origin.x, origin.y))
        .map_err(|e| format!("设置气泡位置失败 {label}：{e}"))?;

    // 注入时机是**竞态**：导航已经完成就能直接成功，还停在 about:blank 就只能等
    // `on_page_load`。把当时的 url 打出来，下次再遇到"气泡不显示"一眼就能判断是哪一种。
    let page_url = window.url().map(|u| u.to_string()).unwrap_or_else(|_| "（读取失败）".to_string());
    eprintln!("[whale-pet] 气泡窗显示 {label}：{width}×{height} 页面 url={page_url}");
    inject_text(&window, &request.text);
    window.show().map_err(|e| format!("显示气泡窗口失败：{e}"))?;
    // 建窗窗口期（WebView2 的子窗口是异步建出来的）由守护线程补位
    spawn_click_through_guard(window.clone());

    // **在显示之后再设一次点击穿透**。两层机制缺一不可：
    //
    //   1. `set_ignore_cursor_events(true)`（宿主 API，守护线程里调用）——tao 会把
    //      `WS_EX_TRANSPARENT | WS_EX_LAYERED` 加到**顶层窗口**上
    //      （`tao-0.35.3/src/platform_impl/windows/window_state.rs:286`），
    //      宠物窗的点击穿透走的就是同一条路；
    //   2. 递归把 `WS_EX_TRANSPARENT` 加到**全部子窗口**——该位是"逐窗口"的命中测试标志，
    //      而鼠标实际命中的是 WebView2 最内层的 `Chrome_RenderWidgetHostHWND`
    //      （进程外实测：只设顶层时点击仍然落在子窗口上）。
    //
    // 位置有讲究：`set_size` / `set_position` / `show` 都会触发 tao 按它自己的标志位
    // **整体重写**窗口样式（`apply_diff` 里是一次 `SetWindowLongW(GWL_EXSTYLE, …)`），
    // 所以补设必须放在这些调用**之后**。
    //
    // 注：早期版本的注释曾断言"宿主 API 在这个窗口上不生效"——那是误判，
    // 真正的原因是在**同步命令**里建窗导致的死锁（webview 从未初始化完成），
    // 详见 `commands::show_bubble` 的文档注释与 `docs/VERIFICATION.md` 的 ⑥。
    set_click_through(&window)?;
    eprintln!("[whale-pet] 气泡窗显示完成 {label}：{}", describe(&window));

    Ok(())
}

/// 读窗口及其直接子窗口的扩展样式，拼成一行可读文本（诊断用）。
#[cfg(windows)]
fn describe<R: Runtime>(window: &tauri::WebviewWindow<R>) -> String {
    let Ok(hwnd) = window.hwnd() else {
        return "（取句柄失败）".to_string();
    };
    let top = ex_flags(hwnd.0 as isize);
    let mut children = Vec::new();
    visit_children(hwnd.0 as isize, &mut children);
    let child_text: Vec<String> = children
        .iter()
        .map(|child| format!("{}[{:#x}]", flags_text(*child), child))
        .collect();
    format!(
        "顶层{}({:#x}) 子窗{}",
        flags_text(top),
        top,
        if child_text.is_empty() { "无".to_string() } else { child_text.join(" ") }
    )
}

/// 非 Windows：没有 Win32 样式可读
#[cfg(not(windows))]
fn describe<R: Runtime>(_window: &tauri::WebviewWindow<R>) -> String {
    String::new()
}

/// 在气泡可见期间**高频重试**设置点击穿透（覆盖 WebView2 异步建窗的窗口期）。
///
/// ## 为什么必须重试而不是设一次
///
/// WebView2 的子窗口是**异步创建**的：`build()` 返回时窗口树里还只有
/// `Tauri Window → WRY_WEBVIEW`，真正接收鼠标的 `Chrome_RenderWidgetHostHWND`
/// 要过一会儿才出现。因此"设一次位"会漏掉后建的子窗口——进程外探针实测：
/// 气泡区域的点命中的正是 `WRY_WEBVIEW`，而它的 `WS_EX_TRANSPARENT` 为 false。
///
/// 这里在显示后的前 ~3 秒里每 50ms 重设一次（覆盖建窗窗口期），之后转入 300ms 的低频守护
/// （防止后续还有样式被重置）。气泡一次只显示几秒，这点开销可以忽略。
#[cfg(windows)]
fn spawn_click_through_guard<R: Runtime>(window: tauri::WebviewWindow<R>) {
    std::thread::spawn(move || {
        let started = std::time::Instant::now();
        loop {
            // 窗口已隐藏：停止守护（下次显示由 `show` 自己补设一次）
            if !window.is_visible().unwrap_or(false) {
                return;
            }
            // 宿主 API + 递归设位，两条路一起上
            if let Err(err) = window.set_ignore_cursor_events(true) {
                eprintln!("[whale-pet] 气泡穿透守护（宿主 API）：{err}");
                return;
            }
            if let Err(err) = set_click_through(&window) {
                eprintln!("[whale-pet] 气泡穿透守护：{err}");
                return;
            }
            let elapsed = started.elapsed();
            if elapsed > std::time::Duration::from_secs(3) {
                // 建窗窗口期已过（WebView2 的子窗口都建出来了）：低频守护即可
                std::thread::sleep(std::time::Duration::from_millis(300));
            } else {
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            // 上限保护：最多守护 40 秒（正常一次只显示几秒）
            if elapsed > std::time::Duration::from_secs(40) {
                return;
            }
        }
    });
}

/// 非 Windows：没有该位可守护（`set_ignore_cursor_events` 已由调用方设置）
#[cfg(not(windows))]
fn spawn_click_through_guard<R: Runtime>(_window: tauri::WebviewWindow<R>) {}

/// 读一个窗口的 `GWL_EXSTYLE`
#[cfg(windows)]
fn ex_flags(hwnd: isize) -> isize {
    #[link(name = "user32")]
    extern "system" {
        fn GetWindowLongPtrW(hwnd: isize, index: i32) -> isize;
    }
    /// `GWL_EXSTYLE`
    const GWL_EXSTYLE: i32 = -20;
    // SAFETY: 只读窗口样式位
    unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) }
}

/// 把扩展样式位翻译成短标签（诊断日志用）
#[cfg(windows)]
fn flags_text(ex: isize) -> String {
    const WS_EX_TRANSPARENT: isize = 0x0000_0020;
    const WS_EX_LAYERED: isize = 0x0008_0000;
    const WS_EX_NOACTIVATE: isize = 0x0800_0000;
    let mut parts = Vec::new();
    if ex & WS_EX_TRANSPARENT != 0 {
        parts.push("T");
    }
    if ex & WS_EX_LAYERED != 0 {
        parts.push("L");
    }
    if ex & WS_EX_NOACTIVATE != 0 {
        parts.push("NA");
    }
    if parts.is_empty() {
        parts.push("-");
    }
    parts.join("|")
}

/// 收集全部子窗口句柄（递归）
#[cfg(windows)]
fn visit_children(hwnd: isize, out: &mut Vec<isize>) {
    #[link(name = "user32")]
    extern "system" {
        fn EnumChildWindows(parent: isize, callback: extern "system" fn(isize, isize) -> i32, lparam: isize) -> i32;
    }
    extern "system" fn on_child(child: isize, lparam: isize) -> i32 {
        // SAFETY: 回调收到的是 `&mut Vec<isize>`
        let out = unsafe { &mut *(lparam as *mut Vec<isize>) };
        out.push(child);
        1
    }
    // SAFETY: 回调只往 Vec 里塞句柄；lparam 指向栈上变量，调用期间有效
    unsafe {
        EnumChildWindows(hwnd, on_child, out as *mut Vec<isize> as isize);
    }
}

/// 给窗口**及其全部子窗口**加上 `WS_EX_TRANSPARENT`（点击穿透）。
///
/// ## 为什么要递归设到子窗口
///
/// WebView2 的窗口是**多层嵌套**的（实测窗口树）：
///
/// ```text
/// Tauri Window            ← window.hwnd() 返回这一层
///   └─ WRY_WEBVIEW        ← Tauri 的 webview 容器
///        ├─ Chrome_WidgetWin_0
///        ├─ Chrome_WidgetWin_1
///        └─ Chrome_RenderWidgetHostHWND   ← **真正接收鼠标的是这一层**
/// ```
///
/// `WS_EX_TRANSPARENT` 是**逐窗口**的命中测试标志：设在父窗口上不会自动作用于子窗口，
/// 而鼠标落点实际命中的是 `Chrome_RenderWidgetHostHWND`。这就是"父窗口设了穿透、
/// 进程外探针却仍然命中本进程"的原因（实测反复验证过）。
/// 因此这里把该位**递归**加到整棵子窗口树上——pet 窗之所以能穿透，正是因为它那层的
/// `Chrome_RenderWidgetHostHWND` 已经带了这个位（窗口树实测：`T=True`）。
#[cfg(windows)]
fn set_click_through<R: Runtime>(window: &tauri::WebviewWindow<R>) -> Result<(), String> {
    /// 取/设窗口扩展样式
    const GWL_EXSTYLE: i32 = -20;
    /// 命中测试时跳过本窗口（点击穿透）
    const WS_EX_TRANSPARENT: isize = 0x0000_0020;
    /// 分层窗：**顶层窗口**只有带上它，穿透位才会对命中测试生效
    const WS_EX_LAYERED: isize = 0x0008_0000;

    #[link(name = "user32")]
    extern "system" {
        fn GetWindowLongPtrW(hwnd: isize, index: i32) -> isize;
        fn SetWindowLongPtrW(hwnd: isize, index: i32, value: isize) -> isize;
        fn EnumChildWindows(parent: isize, callback: extern "system" fn(isize, isize) -> i32, lparam: isize) -> i32;
    }

    /// 给单个窗口设位（顶层连 `WS_EX_LAYERED` 一起设）；返回设置后的样式
    fn apply_one(hwnd: isize, layered: bool) -> isize {
        // SAFETY: 只读写窗口样式位
        unsafe {
            let current = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
            let mut want = current | WS_EX_TRANSPARENT;
            if layered {
                want |= WS_EX_LAYERED;
            }
            if want != current {
                SetWindowLongPtrW(hwnd, GWL_EXSTYLE, want);
            }
            GetWindowLongPtrW(hwnd, GWL_EXSTYLE)
        }
    }

    /// `EnumChildWindows` 的回调：把收到的每个子窗口都设上穿透位
    extern "system" fn on_child(hwnd: isize, _lparam: isize) -> i32 {
        apply_one(hwnd, false);
        1 // 继续枚举
    }

    let hwnd = window
        .hwnd()
        .map_err(|e| format!("取气泡窗句柄失败（{}）：{e}", window.label()))?
        .0 as isize;

    // 递归给整棵子窗口树设位是**跨进程**调用（子窗口有几个归 WebView2 进程），
    // 而跨进程 SendMessage 会等对方线程派发——计时告警，见 watchdog 的复盘。
    let after = crate::watchdog::timed("气泡/菜单窗点击穿透（含子窗口）", || {
        let after = apply_one(hwnd, true);
        // SAFETY: 回调只做样式位读写；lparam 未使用
        unsafe {
            EnumChildWindows(hwnd, on_child, 0);
        }
        after
    });
    if after & WS_EX_TRANSPARENT == 0 {
        return Err(format!(
            "气泡窗点击穿透设置失败（{}）：设置后顶层窗口的 WS_EX_TRANSPARENT 仍为 0（ex={after:#x}）",
            window.label()
        ));
    }
    Ok(())
}

/// 隐藏气泡（窗口保留以便复用；页面内先淡出再隐藏由调用方决定时机）
pub fn hide<R: Runtime>(app: &AppHandle<R>, pet_label: &str) -> Result<(), String> {
    let label = format!("{BUBBLE_LABEL_PREFIX}{pet_label}");
    // 只清"待注入文本"，尺寸/偏移留着（下次显示直接复用）：
    // 否则页面若因为热重载等原因重载，`on_page_load` 会把上一句已经过期的文本又画出来
    if let Ok(mut map) = records().lock() {
        if let Some(record) = map.get_mut(&label) {
            record.text = None;
        }
    }
    if let Some(window) = app.get_webview_window(&label) {
        // 先让页面把气泡淡出（视觉上不突兀），再隐藏窗口
        let _ = window.eval(
            "(function () { var b = document.getElementById('bubble'); if (b) b.classList.remove('is-on'); return 'ok'; })()",
        );
        window.hide().map_err(|e| format!("隐藏气泡窗口失败 {label}：{e}"))?;
    }
    Ok(())
}
