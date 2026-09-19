//! 托盘菜单窗：**自绘**的托盘弹出菜单。
//!
//! ## 为什么不用系统原生菜单
//!
//! Windows 的原生菜单由系统绘制，外观**完全不可控**（圆角、间距、图标、强调色、
//! 深浅色都改不了）。托盘是桌宠唯一的常驻入口，而设置窗口是自绘的——
//! 两处视觉语言不一致会显得"拼出来的"。代价是弹出菜单的三件麻烦事要自己扛：
//!
//!   1. **摆位**：锚在托盘图标（= 点击时的光标位置）附近，并按"点击那一块屏的工作区"夹取，
//!      任务栏在上边还是下边都能正确翻面；
//!   2. **外点关闭**：窗口是 `focusable(false)`（与宠物窗同一纪律：绝不抢焦点），
//!      因此拿不到"失焦"事件——由宿主的光标轮询判定"在菜单矩形外按下"（与右键菜单同一套）；
//!   3. **尺寸随内容变**：主菜单是短列表，"动作点播"展开后要高得多。
//!      页面调 [`resize`] 报上新尺寸，宿主用**记住的锚点**重算位置（否则会跳走）。
//!
//! ## 与右键菜单的关系
//!
//! 两者是**不同的窗口**（`tray-menu` vs `menu-<宠物>`）：右键菜单是"给这只宠物点播动作"，
//! 托盘菜单是"整个应用的开关面板"。它们互斥显示——弹一个就收掉另一个，
//! 否则屏幕上会同时挂两个弹出层。

use std::sync::Mutex;

use tauri::{AppHandle, Manager, PhysicalPosition, PhysicalSize, WebviewUrl, WebviewWindowBuilder};

use crate::model::Vec2;

/// 窗口标签
pub const LABEL: &str = "tray-menu";

/// 主菜单面板尺寸（页面按它排版）
pub const PANEL_W: f64 = 272.0;
pub const PANEL_H: f64 = 286.0;
/// 面板四周留给 CSS 阴影的透明边距（窗口比面板大一圈，阴影才有地方画）
pub const SHADOW_PAD: f64 = 18.0;
/// "动作点播"展开后的面板高度上限（再高就交给页面内部滚动）
pub const PICKER_MAX_H: f64 = 520.0;

/// 上一次弹出时的锚点（光标位置）：`resize` 要用它重算位置，否则展开动作列表时窗口会跳
static LAST_ANCHOR: Mutex<Option<Vec2>> = Mutex::new(None);

/// 启动时把窗口建好并隐藏（"点了就出现"，与右键菜单/气泡同一考虑）
pub fn prepare(app: &AppHandle) -> Result<(), String> {
    if app.get_webview_window(LABEL).is_some() {
        return Ok(());
    }
    let window = WebviewWindowBuilder::new(app, LABEL, WebviewUrl::App("tray-menu.html".into()))
        // 标题在无边框窗口上看不见，但**窗口枚举/截图探针要靠它认人**（脚本按标题匹配）
        .title("whale-pet 托盘菜单")
        .inner_size(PANEL_W + SHADOW_PAD * 2.0, PANEL_H + SHADOW_PAD * 2.0)
        .position(0.0, 0.0)
        .transparent(true)
        .decorations(false)
        .shadow(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .resizable(false)
        .maximizable(false)
        .minimizable(false)
        // 与宠物窗/菜单窗同一纪律：**绝不抢焦点**（否则用户正在打字的窗口会失焦）
        .focusable(false)
        .visible(false)
        .on_page_load(|_window, payload| {
            eprintln!("[whale-pet] 托盘菜单页面加载事件 {:?} url={}", payload.event(), payload.url());
        })
        .build()
        .map_err(|e| format!("创建托盘菜单窗失败：{e}"))?;
    // 建窗过程可能把窗口显示出来，显式再藏一次（气泡/菜单窗都实测过同样的问题）
    window.hide().map_err(|e| format!("预备托盘菜单窗隐藏失败：{e}"))?;
    eprintln!(
        "[whale-pet] 托盘菜单窗已创建（隐藏）：面板 {PANEL_W}×{PANEL_H}，阴影留边 {SHADOW_PAD}"
    );
    Ok(())
}

/// 在光标处弹出托盘菜单（托盘图标被点击时调用）
pub fn show(app: &AppHandle) -> Result<(), String> {
    // 弹出前先收掉宠物右键菜单：两个弹出层同时挂着会互相压住
    crate::menu_window::hide_all(app);

    let physical = crate::display::cursor_position(app)
        .ok_or_else(|| "无法读取光标位置（托盘菜单要靠它定位）".to_string())?;
    let cursor = Vec2 { x: physical.x, y: physical.y };
    {
        let mut anchor = match LAST_ANCHOR.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        *anchor = Some(cursor);
    }

    let window = app
        .get_webview_window(LABEL)
        .ok_or_else(|| "托盘菜单窗不存在，prepare 没跑到？".to_string())?;

    place(&window, cursor, PANEL_W, PANEL_H)?;
    window.show().map_err(|e| format!("显示托盘菜单失败：{e}"))?;
    // 让页面把上一次的展开状态收掉（回到主菜单），并刷新宠物可见性
    if let Err(err) = window.eval(
        "(function () { var m = window.__whalePetTrayMenu; return m ? m.show() : 'not-ready'; })()",
    ) {
        eprintln!("[whale-pet] 托盘菜单落位注入失败：{err}");
    }
    eprintln!(
        "[whale-pet] 托盘菜单已弹出：锚点=({:.0},{:.0})",
        cursor.x, cursor.y
    );
    Ok(())
}

/// 隐藏托盘菜单（**只动窗口**，DOM 交给页面在下次 `show()` 时自己收——避免两边互相调用的回声）
pub fn hide(app: &AppHandle) -> Result<(), String> {
    if let Some(window) = app.get_webview_window(LABEL) {
        window.hide().map_err(|e| format!("隐藏托盘菜单失败：{e}"))?;
    }
    Ok(())
}

/// 页面请求换尺寸（主菜单 ↔ 动作点播展开）；位置按上次锚点重算
pub fn resize(app: &AppHandle, panel_w: f64, panel_h: f64) -> Result<(), String> {
    let panel_w = panel_w.clamp(200.0, 460.0);
    let panel_h = panel_h.clamp(120.0, PICKER_MAX_H);
    let anchor = match LAST_ANCHOR.lock() {
        Ok(guard) => *guard,
        Err(poisoned) => *poisoned.into_inner(),
    };
    let window = app
        .get_webview_window(LABEL)
        .ok_or_else(|| "托盘菜单窗不存在".to_string())?;
    match anchor {
        Some(cursor) => place(&window, cursor, panel_w, panel_h)?,
        None => {
            window
                .set_size(PhysicalSize::new(
                    (panel_w + SHADOW_PAD * 2.0).round(),
                    (panel_h + SHADOW_PAD * 2.0).round(),
                ))
                .map_err(|e| format!("设置托盘菜单尺寸失败：{e}"))?;
        }
    }
    Ok(())
}

/// 托盘菜单当前是否可见
pub fn is_visible(app: &AppHandle) -> bool {
    app.get_webview_window(LABEL)
        .map(|window| window.is_visible().unwrap_or(false))
        .unwrap_or(false)
}

/// 光标轮询里调用：菜单开着时，**在菜单矩形外按下鼠标**就关掉它（窗口不可聚焦，拿不到失焦事件）
pub fn close_on_outside_press(app: &AppHandle, cursor: Vec2) {
    if !is_visible(app) {
        return;
    }
    let Some(window) = app.get_webview_window(LABEL) else {
        return;
    };
    let (Ok(origin), Ok(size)) = (window.outer_position(), window.inner_size()) else {
        return;
    };
    let inside = cursor.x >= f64::from(origin.x)
        && cursor.x < f64::from(origin.x) + f64::from(size.width)
        && cursor.y >= f64::from(origin.y)
        && cursor.y < f64::from(origin.y) + f64::from(size.height);
    if inside {
        return;
    }
    eprintln!("[whale-pet] 托盘菜单外部按下 → 关闭");
    let _ = hide(app);
}

/// 按锚点算窗口原点：优先出现在光标**上方**（任务栏默认在屏幕底部），
/// 上方不够就翻到下方；最后整体夹进"点击那一块屏"的工作区。
fn place(
    window: &tauri::WebviewWindow,
    cursor: Vec2,
    panel_w: f64,
    panel_h: f64,
) -> Result<(), String> {
    let window_w = panel_w + SHADOW_PAD * 2.0;
    let window_h = panel_h + SHADOW_PAD * 2.0;

    let area = crate::display::work_area_containing(window.app_handle(), cursor)
        .or_else(|| crate::display::primary_work_area(window.app_handle()))
        .ok_or_else(|| "无法读取显示器工作区".to_string())?;

    let gap = 10.0;
    let space_above = cursor.y - area.y;
    let space_below = area.bottom() - cursor.y;
    let raw_y = if space_above >= panel_h + gap || space_above >= space_below {
        cursor.y - gap - panel_h - SHADOW_PAD
    } else {
        cursor.y + gap + SHADOW_PAD
    };
    // 右缘大致对齐托盘图标（面板比窗口窄一圈，所以还要把阴影留边减回来）
    let raw_x = cursor.x + gap - panel_w - SHADOW_PAD;

    let max_x = (area.x + area.width - window_w).max(area.x);
    let max_y = (area.y + area.height - window_h).max(area.y);

    window
        .set_size(PhysicalSize::new(window_w.round(), window_h.round()))
        .map_err(|e| format!("设置托盘菜单尺寸失败：{e}"))?;
    window
        .set_position(PhysicalPosition::new(
            raw_x.clamp(area.x, max_x).round(),
            raw_y.clamp(area.y, max_y).round(),
        ))
        .map_err(|e| format!("设置托盘菜单位置失败：{e}"))?;
    Ok(())
}
