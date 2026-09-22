//! 显示器几何：把"桌面"建模为**工作区矩形列表**，并轮询它的变化。
//!
//! 两条关键设计（都来自上游 dsh-pet 踩过的坑）：
//!
//! 1. **边界用并集，不用外接矩形**：显示器摆放不规则时（副屏竖置、错位排列），
//!    外接矩形里会出现大片不属于任何显示器的"空洞"。若拿外接矩形当边界，
//!    被甩出的宠物会飞进空洞——用户完全看不到它。因此抛掷边界必须用逐屏 AABB（前端
//!    的 `throwStepRegion` 已按此实现），Rust 侧只负责把**逐屏工作区列表**送过去。
//!
//! 2. **工作区与完整面板要分开**：工作区（不含任务栏）用于落地/落点判定，
//!    完整面板（含任务栏）用于"屏缝处有没有邻屏"的探测——详见 `model.rs` 的字段注释。
//!
//! 3. **几何变化只能轮询**：Tauri / tao 没有暴露"显示器插拔/改分辨率"的事件，
//!    只能周期性取回显示器列表并与上次做**指纹比对**，变了才下发事件。
//!    指纹用几何数值拼字符串，避免为此引入哈希依赖。

use tauri::{AppHandle, PhysicalPosition, Runtime};

use crate::model::{DisplaysSample, Rect};

/// 显示器几何指纹：几何值拼成的稳定字符串，用于判断"是否真的变了"。
///
/// 为什么不直接比较结构体列表：位置/尺寸是 f64，且在部分平台上会出现
/// `1919.9999` 与 `1920.0` 这类表示差异；指纹里对它做四舍五入后再比较，
/// 避免因为浮点噪声每秒都误判成"几何变了"而反复重排宠物。
///
/// 面板也要进指纹：任务栏自动隐藏/移位会改变面板（而工作区可能不变），
/// 那正是跨屏放行判定依赖的量。
fn fingerprint(areas: &[Rect], panels: &[Rect], primary: &Rect) -> String {
    let mut parts: Vec<String> = areas
        .iter()
        .map(|a| format!("{:.0},{:.0},{:.0},{:.0}", a.x, a.y, a.width, a.height))
        .collect();
    parts.push(
        panels
            .iter()
            .map(|p| format!("P{:.0},{:.0},{:.0},{:.0}", p.x, p.y, p.width, p.height))
            .collect::<Vec<_>>()
            .join(";"),
    );
    parts.push(format!(
        "M{:.0},{:.0},{:.0},{:.0}",
        primary.x, primary.y, primary.width, primary.height
    ));
    parts.join("|")
}

/// 取当前显示器几何；失败时返回可读错误（不静默返回空列表，否则宠物会失去所有边界）
pub fn current_sample<R: Runtime>(app: &AppHandle<R>) -> Result<(DisplaysSample, String), String> {
    let monitors = app.available_monitors().map_err(|e| format!("读取显示器列表失败：{e}"))?;
    if monitors.is_empty() {
        return Err("系统未报告任何显示器".to_string());
    }

    // 工作区（不含任务栏）：漫游落点、角落定位、落地判定
    let areas: Vec<Rect> = monitors
        .iter()
        .map(|m| {
            let area = m.work_area();
            Rect {
                x: area.position.x as f64,
                y: area.position.y as f64,
                width: area.size.width as f64,
                height: area.size.height as f64,
            }
        })
        .collect();

    // 完整面板（含任务栏区域）：抛掷跨屏缝时的"邻屏探测"
    let panels: Vec<Rect> = monitors
        .iter()
        .map(|m| {
            let position = m.position();
            let size = m.size();
            Rect {
                x: position.x as f64,
                y: position.y as f64,
                width: size.width as f64,
                height: size.height as f64,
            }
        })
        .collect();

    // 主屏工作区：角落定位必须以它为准。外接矩形的角落可能压根不属于任何显示器
    // （实测右倒 T 型双屏：top-left 落在主屏上方的空洞里，配了该角落的宠物开机即隐身）
    let primary_monitor = app
        .primary_monitor()
        .map_err(|e| format!("读取主显示器失败：{e}"))?
        .ok_or_else(|| "系统未报告主显示器".to_string())?;
    let p_area = primary_monitor.work_area();
    let primary = Rect {
        x: p_area.position.x as f64,
        y: p_area.position.y as f64,
        width: p_area.size.width as f64,
        height: p_area.size.height as f64,
    };

    let fp = fingerprint(&areas, &panels, &primary);
    Ok((DisplaysSample { areas, panels, primary }, fp))
}

/// 当前光标在桌面坐标系中的位置（物理像素）。
///
/// 说明：Tauri 的 `AppHandle::cursor_position` 原点 = 桌面左上角，
/// 多显示器布局下可为负值——与窗口位置、前端物理层使用同一套坐标系，因此可直接相减比较。
pub fn cursor_position<R: Runtime>(app: &AppHandle<R>) -> Option<PhysicalPosition<f64>> {
    app.cursor_position().ok()
}

/// 主鼠标键当前是否按下（Windows：`GetAsyncKeyState(VK_LBUTTON)`）。
///
/// 为什么用它而不是等页面的 pointer 事件：
///   窗口处于**点击穿透**状态时收不到任何鼠标事件，而拖拽中宠物滞后于光标、
///   光标很容易滑出命中区并触发穿透翻转——那一刻的松手事件会丢，
///   前端状态机就永久停在"按下中"。全局按键位是唯一不依赖窗口消息的可靠来源。
///
/// 只在 Windows 上实现（本项目 M0 的目标平台）；其它平台返回 `false`
/// （语义 = "检测不到按键状态"，前端会退化为只依赖 DOM 事件）。
#[cfg(windows)]
pub fn primary_button_down() -> bool {
    // SAFETY: GetAsyncKeyState 是只读查询，无副作用、不涉及内存安全
    unsafe { (GetAsyncKeyState(VK_LBUTTON) as u16 & 0x8000) != 0 }
}

/// 非 Windows 平台：不支持全局按键查询
#[cfg(not(windows))]
pub fn primary_button_down() -> bool {
    false
}

/// 主键按下的同时，鼠标是否被**别的进程**的窗口捕获（Windows：`GetGUIThreadInfo` 的 `hwndCapture`）。
///
/// 用途（配合前端 `runtime.ts::evaluateInput` 的采样兜底起手）：
/// 全局采样只能看到"按键按着 + 光标在宠物身体上"，区分不了
///   - "用户抓住了宠物"（此时鼠标捕获属于宠物自己的窗口——WebView2 在按下时 `SetCapture`）；
///   - "用户正在桌面框选 / 在别的窗口里拖选，光标路过宠物"（捕获在对方进程手里）。
/// 后者如果被当成前者，宠物就会跟着别人的拖动一起走（用户实测的 bug）。
///
/// 只查**前台窗口所属线程**（`idThread = 0` 的语义就是"前台窗口的活动线程"）：
/// 这覆盖了绝大多数拖动场景，且**查询失败一律返回 `false`（放行）**——
/// 这道闸只用来少挡一次误抓，绝不能因为查不到而把正常点击吞掉。
#[cfg(windows)]
pub fn foreign_mouse_capture() -> bool {
    use win_key::{GetGUIThreadInfo, GetWindowThreadProcessId, GuiThreadInfo};
    let mut info = GuiThreadInfo::default();
    info.cb_size = std::mem::size_of::<GuiThreadInfo>() as u32;
    // SAFETY: 只写入本函数栈上的 GUITHREADINFO（cbSize 已按结构体实际大小填好）
    if unsafe { GetGUIThreadInfo(0, &mut info) } == 0 {
        return false;
    }
    if info.hwnd_capture == 0 {
        return false;
    }
    let mut pid: u32 = 0;
    // SAFETY: hwnd 来自系统返回的捕获窗口句柄，pid 是本函数栈上的 u32
    unsafe { GetWindowThreadProcessId(info.hwnd_capture, &mut pid) };
    pid != 0 && pid != std::process::id()
}

/// 非 Windows 平台：不支持捕获查询（返回"没有外来捕获"= 放行）
#[cfg(not(windows))]
pub fn foreign_mouse_capture() -> bool {
    false
}

#[cfg(windows)]
mod win_key {
    /// 虚拟键码：鼠标主键（左键）
    pub const VK_LBUTTON: i32 = 0x01;

    /// 屏幕坐标点（Win32 `POINT`）
    #[repr(C)]
    #[derive(Debug, Clone, Copy, Default)]
    pub struct Point {
        pub x: i32,
        pub y: i32,
    }

    /// 矩形（Win32 `RECT`；`GUITHREADINFO` 尾部需要它）
    #[repr(C)]
    #[derive(Debug, Clone, Copy, Default)]
    pub struct WinRect {
        pub left: i32,
        pub top: i32,
        pub right: i32,
        pub bottom: i32,
    }

    /// 线程 GUI 状态（Win32 `GUITHREADINFO`）
    ///
    /// 字段顺序/类型必须与 Win32 结构体逐字节一致：`cbSize` 由调用方填结构体大小，
    /// 系统按它判断版本。这里只为读 `hwnd_capture`，其余字段照抄以保持布局。
    #[repr(C)]
    #[derive(Debug, Clone, Copy, Default)]
    pub struct GuiThreadInfo {
        pub cb_size: u32,
        pub flags: u32,
        pub hwnd_active: isize,
        pub hwnd_focus: isize,
        pub hwnd_capture: isize,
        pub hwnd_menu_owner: isize,
        pub hwnd_move_size: isize,
        pub hwnd_caret: isize,
        pub rc_caret: WinRect,
    }

    #[link(name = "user32")]
    extern "system" {
        /// 查询虚拟键状态；最高位为 1 表示当前按下
        pub fn GetAsyncKeyState(v_key: i32) -> i16;
        /// 取光标在**虚拟桌面**中的位置（物理像素）
        pub fn GetCursorPos(point: *mut Point) -> i32;
        /// 取某个线程（传 0 = 前台窗口所属线程）的 GUI 状态，含"谁捕获了鼠标"
        pub fn GetGUIThreadInfo(id_thread: u32, info: *mut GuiThreadInfo) -> i32;
        /// 取窗口所属进程 id（用来判断捕获窗口是不是本进程的）
        pub fn GetWindowThreadProcessId(hwnd: isize, pid: *mut u32) -> u32;
    }
}

#[cfg(windows)]
use win_key::{GetAsyncKeyState, GetCursorPos, Point, VK_LBUTTON};

/// Win32 `GetCursorPos`：光标在虚拟桌面中的物理像素坐标（原点 = 主屏左上角，左侧副屏为负）。
///
/// 它与 Tauri 的 `AppHandle::cursor_position()` 是**两个不同的来源**，实测在 Windows 上
/// 可能给出不同的值（本项目的坐标错位问题就是这么来的）。排障时把两者放在一起比对，
/// 就能立刻判断是谁错了。
#[cfg(windows)]
pub fn win32_cursor_position() -> Option<PhysicalPosition<f64>> {
    let mut point = Point::default();
    // SAFETY: 只写入本函数栈上的 POINT 结构
    let ok = unsafe { GetCursorPos(&mut point) };
    if ok == 0 {
        return None;
    }
    Some(PhysicalPosition::new(point.x as f64, point.y as f64))
}

/// 非 Windows：没有 Win32 对照源
#[cfg(not(windows))]
pub fn win32_cursor_position() -> Option<PhysicalPosition<f64>> {
    None
}

/// 便捷函数：取主屏工作区（前端还没拿到几何事件时的角落定位兜底由前端负责）
pub fn primary_work_area<R: Runtime>(app: &AppHandle<R>) -> Option<Rect> {
    let monitor = app.primary_monitor().ok()??;
    let area = monitor.work_area();
    Some(Rect {
        x: area.position.x as f64,
        y: area.position.y as f64,
        width: area.size.width as f64,
        height: area.size.height as f64,
    })
}

/// 取**包含该点**的显示器工作区（找不到返回 `None`：点在空洞里或屏外）。
///
/// 用途：右键菜单窗要按"右键点所在的那块屏"夹取摆位——多屏时用主屏的工作区去夹，
/// 副屏上的菜单会被硬拽到主屏上去。
pub fn work_area_containing<R: Runtime>(app: &AppHandle<R>, point: crate::model::Vec2) -> Option<Rect> {
    let (sample, _) = current_sample(app).ok()?;
    sample.areas.into_iter().find(|a| {
        point.x >= a.x && point.x < a.x + a.width && point.y >= a.y && point.y < a.y + a.height
    })
}

/// 日志用：把几何列表压成一行（避免每帧刷屏）
pub fn describe(areas: &[Rect]) -> String {
    areas
        .iter()
        .map(|a| format!("({:.0},{:.0} {:.0}×{:.0})", a.x, a.y, a.width, a.height))
        .collect::<Vec<_>>()
        .join(" ")
}

/// 校验"工作区 / 完整面板"这一对几何的内在一致性。
///
/// 需要成立的三条（前端把两者**按序**配对使用，顺序错了就会拿错屏的边界）：
///   1. 列表非空且长度相等；
///   2. 逐项：工作区必须被对应的面板**包含**（工作区 = 面板扣掉任务栏等区域）；
///   3. 逐项：面板面积 ≥ 工作区面积。
///
/// 启动时会调用它：几何不自洽时**立刻报错**，而不是让宠物带着错误边界跑起来
/// （那种情况下的表现是抛掷穿屏或卡在屏缝，极难排查）。
/// 返回 `Err` 时带可读原因，便于直接打印。
pub fn validate_geometry(sample: &DisplaysSample) -> Result<(), String> {
    if sample.areas.is_empty() {
        return Err("工作区列表为空".to_string());
    }
    if sample.areas.len() != sample.panels.len() {
        return Err(format!(
            "工作区与面板数量不一致：{} vs {}（前端按序配对，长度必须相同）",
            sample.areas.len(),
            sample.panels.len()
        ));
    }
    for (index, (area, panel)) in sample.areas.iter().zip(sample.panels.iter()).enumerate() {
        if !(area.x >= panel.x
            && area.y >= panel.y
            && area.right() <= panel.right() + 0.5
            && area.bottom() <= panel.bottom() + 0.5)
        {
            return Err(format!(
                "第 {index} 块屏的工作区 {:?} 没有被面板 {:?} 包含",
                (area.x, area.y, area.width, area.height),
                (panel.x, panel.y, panel.width, panel.height)
            ));
        }
        if panel.width * panel.height < area.width * area.height - 0.5 {
            return Err(format!("第 {index} 块屏的面板面积小于工作区面积"));
        }
    }
    if !(sample.primary.width > 0.0 && sample.primary.height > 0.0) {
        return Err("主屏工作区尺寸异常".to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造一个与工作区一致的面板列表（单屏场景下两者相等）
    fn panels_of(areas: &[Rect]) -> Vec<Rect> {
        areas.to_vec()
    }

    #[test]
    fn fingerprint_changes_when_geometry_changes() {
        let areas = vec![Rect { x: 0.0, y: 0.0, width: 1920.0, height: 1040.0 }];
        let primary = areas[0];
        let base = fingerprint(&areas, &panels_of(&areas), &primary);
        let wider = vec![Rect { x: 0.0, y: 0.0, width: 2560.0, height: 1040.0 }];
        let moved = fingerprint(&wider, &panels_of(&wider), &primary);
        assert_ne!(base, moved, "分辨率变化必须反映到指纹");
    }

    #[test]
    fn fingerprint_changes_when_only_panels_change() {
        // 任务栏自动隐藏会导致面板不变、工作区变大（或反之）——两者都要被检出
        let areas = vec![Rect { x: 0.0, y: 0.0, width: 1920.0, height: 1040.0 }];
        let primary = areas[0];
        let base = fingerprint(&areas, &panels_of(&areas), &primary);
        let longer_panels = vec![Rect { x: 0.0, y: 0.0, width: 1920.0, height: 1080.0 }];
        assert_ne!(
            base,
            fingerprint(&areas, &longer_panels, &primary),
            "仅面板变化（例如任务栏隐藏）也必须反映到指纹"
        );
    }

    #[test]
    fn fingerprint_ignores_subpixel_noise() {
        let areas = vec![Rect { x: 0.4, y: 0.0, width: 1920.4, height: 1040.0 }];
        let primary = areas[0];
        let noisy = vec![Rect { x: 0.2, y: 0.0, width: 1920.1, height: 1040.0 }];
        assert_eq!(
            fingerprint(&areas, &panels_of(&areas), &primary),
            fingerprint(&noisy, &panels_of(&noisy), &primary),
            "亚像素抖动不应被判定为几何变化"
        );
    }
}
