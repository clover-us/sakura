//! 右键菜单窗口：菜单自己是一个**独立小窗**（与气泡同一套思路）。
//!
//! ## 为什么不再"临时扩大宠物窗"
//!
//! 早期把菜单画在宠物窗里，需要时把窗口临时外扩。那套做法带来了一连串坑：
//! 外扩方向与偏移换算、菜单开着时再右键会整体偏移、外扩期间要放宽命中区、
//! 以及最烦人的——**换几何时人物虚影闪一下**（窗口移动由系统异步应用，而"宠物画在窗口内
//! 什么位置"由页面 DOM 决定，两者跨进程，**不可能原子**）。
//!
//! 改成独立小窗之后，宠物窗**从头到尾不改几何**：菜单窗按右键点摆位、超界就夹进工作区，
//! 关掉就隐藏。上面那一整类问题随之消失。
//!
//! ## 三个关键约束
//!
//! - **不可聚焦**（`focusable(false)`）：点菜单不能把用户正在用的窗口顶掉
//!   （与宠物窗/气泡窗同一套理由）；
//! - **必须能收鼠标**：与气泡相反，菜单窗**不能**点击穿透（否则点不到菜单项）。
//!   代价是它覆盖的区域在菜单开着期间会吃掉点击——与旧实现（外扩后的宠物窗 780×656）
//!   量级相同，而且只持续到菜单关闭；
//! - **什么时候关**：点了菜单项 / 在菜单矩形外按下鼠标（宿主在光标轮询里判定，
//!   因为菜单窗是不可聚焦的，拿不到"失去焦点"事件）/ 再次右键（替换成新的菜单）。
//!
//! ## 摆位：把"右键点"换算成窗口原点 + 页面内缩进
//!
//! 页面只知道"菜单要画在窗口内的哪个位置"，宿主知道"窗口该摆在屏幕的哪里"。
//! 两者用同一个夹取结果算出来：
//!
//! ```text
//! winX = clamp(clickX, area.x, area.right - W)   // 窗口不越出工作区
//! insetX = clickX - winX                          // 菜单在窗口内的落点
//! ```
//!
//! 于是"能放下时菜单正好在鼠标处、放不下时整体内移但不被屏幕裁掉"。

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, PhysicalPosition, PhysicalSize, Runtime, WebviewUrl, WebviewWindowBuilder};

use crate::model::Vec2;

/// 菜单窗标签前缀（与宠物窗 `pet-*`、气泡窗 `bubble-*` 区分开）
pub const MENU_LABEL_PREFIX: &str = "menu-";

/// 菜单窗尺寸（像素）
///
/// 取值依据（见 `reference/shared/menu.ts`）：
///   - 级联最多 **三级**（动作 → 分类 → 具体动画），每级面板最宽 240px，留出间隙 → 780 够用；
///   - 每级面板最高 460px（CSS `max-height:min(62vh,460px)`），加上根面板可能的纵向错位 → 520。
///
/// 页面拿它当"允许占用的矩形"传给上游的 `mountContextMenu`（clamp），
/// 于是面板永远落在窗口内、不会越出屏幕。
pub const MENU_WINDOW_W: f64 = 780.0;
pub const MENU_WINDOW_H: f64 = 520.0;

/// 菜单项的点击结果（菜单页 → 宿主 → 宠物页）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MenuAction {
    /// 要点播的动画名（`None` 表示这是个"动作"而非"素材"）
    pub anim: Option<String>,
    /// 本地扩展动作（`home` / `say-demo`）
    pub action: Option<String>,
}

/// 启动时把菜单窗建好并隐藏（与气泡一样，避免第一次右键时现建窗的等待）
pub fn prepare<R: Runtime>(app: &AppHandle<R>, pet_label: &str) -> Result<(), String> {
    let label = menu_label(pet_label);
    if app.get_webview_window(&label).is_some() {
        return Ok(());
    }
    let window = WebviewWindowBuilder::new(app, &label, WebviewUrl::App(menu_url(pet_label).into()))
        .inner_size(MENU_WINDOW_W, MENU_WINDOW_H)
        .position(0.0, 0.0)
        .transparent(true)
        .decorations(false)
        .shadow(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .resizable(false)
        .maximizable(false)
        .minimizable(false)
        // 预先建好但不显示
        .visible(false)
        // **绝不抢焦点**：点菜单不该把用户正在用的窗口顶掉
        .focusable(false)
        .on_page_load(|_window, payload| {
            eprintln!(
                "[whale-pet] 菜单页面加载事件 {:?} url={}",
                payload.event(),
                payload.url()
            );
        })
        .build()
        .map_err(|e| format!("创建菜单窗口失败 {label}：{e}"))?;
    // 建窗过程可能把窗口显示出来，显式再藏一次（气泡那边实测过同样的问题）
    window.hide().map_err(|e| format!("预备菜单窗隐藏失败 {label}：{e}"))?;
    eprintln!("[whale-pet] 菜单窗已创建 {label}：{MENU_WINDOW_W}×{MENU_WINDOW_H}（隐藏）");
    Ok(())
}

/// 菜单窗的页面 URL（把宠物标签带进页面，页面据此取配置、建菜单树）
fn menu_url(pet_label: &str) -> String {
    format!("menu.html?label={pet_label}")
}

/// 菜单窗标签
fn menu_label(pet_label: &str) -> String {
    format!("{MENU_LABEL_PREFIX}{pet_label}")
}

/// 对外的菜单窗标签（诊断脚本/命令要按它定位菜单窗）
pub fn label_for(pet_label: &str) -> String {
    menu_label(pet_label)
}

/// 在指定的**屏幕坐标**上弹出该宠物的菜单。
///
/// `screen_x/screen_y` 是右键点在屏幕坐标系里的位置（前端从 DOM 事件推出来的，
/// 与命中判定同一套坐标）。
pub fn show<R: Runtime>(
    app: &AppHandle<R>,
    pet_label: &str,
    screen_x: f64,
    screen_y: f64,
) -> Result<(), String> {
    let label = menu_label(pet_label);
    let window = app
        .get_webview_window(&label)
        .ok_or_else(|| format!("菜单窗不存在（{label}），prepare 没跑到？"))?;

    // 用"右键点所在的那块屏"的工作区做夹取；取不到就退化成主屏
    let area = crate::display::work_area_containing(app, Vec2 { x: screen_x, y: screen_y })
        .or_else(|| crate::display::primary_work_area(app))
        .ok_or_else(|| "无法读取显示器工作区".to_string())?;

    // 窗口原点 + 页面内缩进：两者由同一个夹取结果算出来（见模块头注释）
    let max_x = (area.x + area.width - MENU_WINDOW_W).max(area.x);
    let max_y = (area.y + area.height - MENU_WINDOW_H).max(area.y);
    let origin = Vec2 {
        x: screen_x.clamp(area.x, max_x).round(),
        y: screen_y.clamp(area.y, max_y).round(),
    };
    let inset = Vec2 { x: (screen_x - origin.x).round(), y: (screen_y - origin.y).round() };

    window
        .set_size(PhysicalSize::new(MENU_WINDOW_W, MENU_WINDOW_H))
        .map_err(|e| format!("设置菜单窗尺寸失败 {label}：{e}"))?;
    window
        .set_position(PhysicalPosition::new(origin.x, origin.y))
        .map_err(|e| format!("设置菜单窗位置失败 {label}：{e}"))?;
    window.show().map_err(|e| format!("显示菜单窗失败 {label}：{e}"))?;

    // 让页面对着自己那份窗口尺寸（innerWidth/innerHeight）把菜单落到内缩进处。
    // 用 eval 而不是事件：菜单页不需要事件权限，宿主也不用等回执。
    let script = format!(
        "(function () {{ var m = window.__whalePetMenu; if (!m) return 'not-ready'; m.show({}, {}); return 'ok'; }})()",
        inset.x, inset.y
    );
    if let Err(err) = window.eval(&script) {
        eprintln!("[whale-pet] 菜单落位注入失败 {label}：{err}");
    }
    eprintln!(
        "[whale-pet] 菜单窗显示 {label}：右键点=({screen_x:.0},{screen_y:.0}) →窗口原点=({},{}) 内缩进=({},{}) 工作区=({:.0},{:.0} {:.0}×{:.0})",
        origin.x, origin.y, inset.x, inset.y, area.x, area.y, area.width, area.height
    );
    Ok(())
}

/// 关闭菜单窗（**只隐藏窗口**，不动页面里的 DOM）。
///
/// ## 为什么这里刻意不 eval 页面去"收 DOM"（踩过的坑）
///
/// 早先这里会调 `window.__whalePetMenu.close()` 让页面把菜单 DOM 收掉，而页面的
/// `close()` 又会 `invoke('hide_menu')` 请宿主隐藏窗口——**两边互相调用，形成回声**：
///
/// ```text
/// 宿主 close() → eval m.close() → 页面 closeMenu() → invoke hide_menu → 宿主 close() → …
/// ```
///
/// 每轮只花一次 IPC，于是窗口被**反复隐藏**：用户点过「显示一句气泡」之后再右键就
/// "弹不出交互窗口"了（每次 show 立刻被循环里的 hide 抹掉）。实测踩过，所以现在职责切干净：
///
///   - **宿主只负责窗口**（显示 / 隐藏 / 摆位）；
///   - **页面只负责 DOM**（它在下一次 `show()` 开头会顺手把上一份收掉）。
pub fn close<R: Runtime>(app: &AppHandle<R>, pet_label: &str) -> Result<(), String> {
    let label = menu_label(pet_label);
    if let Some(window) = app.get_webview_window(&label) {
        window.hide().map_err(|e| format!("隐藏菜单窗失败 {label}：{e}"))?;
        // 每次关闭记一行：正常一次交互只有 1~2 行；如果这里开始刷屏，
        // 说明页面与宿主又形成了"互相调用"的回声（见上面那段说明）
        eprintln!("[whale-pet] 菜单窗已隐藏 {label}");
    }
    Ok(())
}

/// 菜单窗当前是否可见（诊断自检用：验证"弹出来之后没有被谁立刻藏掉"）
pub fn is_visible<R: Runtime>(app: &AppHandle<R>, pet_label: &str) -> bool {
    app.get_webview_window(&menu_label(pet_label))
        .map(|window| window.is_visible().unwrap_or(false))
        .unwrap_or(false)
}

/// 光标轮询里调用：菜单开着时，**在菜单矩形外按下鼠标**就关掉它。
///
/// 为什么由宿主判定而不是页面：菜单窗是不可聚焦的（`focusable(false)`），
/// 它拿不到"失去焦点"这类事件；而"鼠标在别处按下了"这件事，宿主手里的
/// 全局光标与按键状态本来就有（而且实测是准的）。
pub fn close_on_outside_press<R: Runtime>(app: &AppHandle<R>, cursor: Vec2) {
    // 先把标签列表拷出来：别在持锁期间做跨 webview 调用（与 lib.rs 的广播同一约定）
    let labels: Vec<String> = {
        let state = app.state::<crate::state::AppState>();
        let pets = crate::watchdog::timed_lock(&state.pets, "pets（菜单外点关闭）");
        pets.keys().map(|label| menu_label(label)).collect()
    };
    for label in labels {
        let Some(window) = app.get_webview_window(&label) else {
            continue;
        };
        if !window.is_visible().unwrap_or(false) {
            continue;
        }
        // 菜单矩形（屏幕坐标）：命中就不关（这一下是点在菜单里的）
        let (Ok(origin), Ok(size)) = (window.outer_position(), window.inner_size()) else {
            continue;
        };
        let inside = cursor.x >= f64::from(origin.x)
            && cursor.x < f64::from(origin.x) + f64::from(size.width)
            && cursor.y >= f64::from(origin.y)
            && cursor.y < f64::from(origin.y) + f64::from(size.height);
        if inside {
            continue;
        }
        eprintln!("[whale-pet] 菜单窗外部按下 → 关闭 {label}");
        // 从标签反推宠物标签（`menu-` 前缀之后就是它）
        if let Some(pet_label) = label.strip_prefix(MENU_LABEL_PREFIX) {
            let _ = close(app, pet_label);
        }
    }
}
