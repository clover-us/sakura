//! 系统托盘：桌宠的"常驻入口"。
//!
//! ## 托盘只做两件事
//!
//! ```text
//! 左键单击  → 显示/隐藏全部宠物（一键找回："宠物不见了"是最常见的求助场景）
//! 右键单击  → 弹出**自绘菜单窗**（见 tray_menu.rs：显隐切换 / 回到初始位置 /
//!             动作点播 / 设置 / 退出）
//! ```
//!
//! ## 为什么菜单不在托盘里画，而是另开一个窗口
//!
//! Windows 的原生托盘菜单由系统绘制，外观不可控；而设置窗口是自绘的，
//! 两处风格不一致会显得拼凑。所以菜单搬进一个**不可聚焦的透明小窗**
//! （`tray_menu.rs`），里面是普通 HTML/CSS——圆角、图标、深浅色、粉色强调色都能做。
//! 开机自启**从托盘移到了设置窗口**：它是"设一次就不动"的开关，
//! 常驻在托盘里只会让高频菜单变长。
//!
//! ## 纪律：持锁期间绝不动窗口
//!
//! `state.pets` 也会被光标轮询线程拿；窗口操作（`show()/hide()`）会等窗口所属线程派发消息，
//! 一旦"持锁 + 窗口调用"撞上主线程等锁就是一次真实死锁（复盘见 `watchdog.rs`）。
//! 因此这里一律"锁内只取标签清单，出锁后再动窗口"。

use std::sync::atomic::{AtomicBool, Ordering};

use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager};

use crate::menu_window::MenuAction;
use crate::pet_window::EVENT_MENU_ACTION;
use crate::state::AppState;
use crate::watchdog;

/// 托盘图标 id（全局唯一）
pub const ID: &str = "pet-tray";

/// 用户是否明确点了「退出」。
///
/// 为什么需要这个标志：`AppHandle::exit()` 也会走 `RunEvent::ExitRequested`，
/// 而我们在那里拦了"重建期间窗口数瞬时归零"造成的误退出（见 `lib.rs` 的 run 闭包）。
/// 没有这个标志，「退出」会被自己的拦截逻辑挡住——**点退出却退不掉**。
static QUITTING: AtomicBool = AtomicBool::new(false);

/// 用户是否已请求退出（`lib.rs` 的退出拦截据此放行）
pub fn is_quitting() -> bool {
    QUITTING.load(Ordering::SeqCst)
}

/// 构造托盘图标（菜单由 `tray_menu` 那个窗口负责，这里不挂原生菜单）
pub fn build(app: &AppHandle) -> Result<(), Box<dyn std::error::Error>> {
    if app.tray_by_id(ID).is_some() {
        let _ = app.remove_tray_by_id(ID);
    }

    TrayIconBuilder::with_id(ID)
        // 图标是预生成的 RGBA（源自 icons/design/app-icon.svg，与 exe 的 .ico 同源）
        // 改一处两处同时变；托盘用 `Detail::Tray`（省掉 16px 看不清的气泡与嘴、尾叶加粗）
        .icon(tray_icon_image())
        // 没有任何原生菜单：左键直接切换显隐，右键弹自绘菜单
        .show_menu_on_left_click(false)
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click { button, button_state: MouseButtonState::Up, .. } = event {
                match button {
                    MouseButton::Left => {
                        eprintln!("[whale-pet] 托盘：左键单击 → 切换显隐");
                        toggle_all(tray.app_handle());
                    }
                    MouseButton::Right => {
                        if let Err(err) = crate::tray_menu::show(tray.app_handle()) {
                            eprintln!("[whale-pet] 托盘：弹出菜单失败：{err}");
                        }
                    }
                    _ => {}
                }
            }
        })
        .build(app)?;
    Ok(())
}

/// 托盘图标：**预生成的裸 RGBA**（32×32，4096 字节）。
///
/// 图标源是 SVG（`icons/design/app-icon.svg`），由 `cargo run --example make-icon` 栅格化成
/// `icons/tray-32.rgba`；这里直接 `include_bytes!` 吃进来。
///
/// 为什么不在运行时画：应用**不该为了一个图标背上 SVG 渲染器**（resvg 及其依赖在二进制里
/// 是好几 MB）。预生成还有个附带好处：托盘图标与 exe/任务栏用的 `.ico` **出自同一张 SVG**，
/// 不可能不一致。（第一版是用 Rust 画 SDF，观感差到被用户点名"图标丑"，故换成 SVG。）
const TRAY_RGBA: &[u8] = include_bytes!("../icons/tray-32.rgba");
/// 托盘图标边长（必须与 `examples/make-icon.rs` 的 `TRAY_SIZE` 一致）
const TRAY_SIZE: u32 = 32;

/// 托盘图标（读预生成的 RGBA；长度不对说明生成物与代码版本不匹配，直接报出来）
fn tray_icon_image() -> tauri::image::Image<'static> {
    let expected = (TRAY_SIZE * TRAY_SIZE * 4) as usize;
    if TRAY_RGBA.len() != expected {
        eprintln!(
            "[whale-pet] 托盘图标数据长度异常：{} 字节（应为 {expected}）——请重新运行 `cargo run --example make-icon`",
            TRAY_RGBA.len()
        );
    }
    tauri::image::Image::new_owned(TRAY_RGBA.to_vec(), TRAY_SIZE, TRAY_SIZE)
}

/// 有任意一只宠物当前可见吗（托盘菜单的"显示/隐藏"切换项据此决定动作与文案）
pub fn any_visible(app: &AppHandle) -> bool {
    let windows: Vec<tauri::WebviewWindow> = {
        let state = app.state::<AppState>();
        let pets = watchdog::timed_lock(&state.pets, "pets（托盘可见性）");
        pets.keys().filter_map(|label| app.get_webview_window(label)).collect()
    };
    windows.iter().any(|window| window.is_visible().unwrap_or(false))
}

/// 显示/隐藏的**一个**切换动作（托盘左键与菜单第一项共用）
pub fn toggle_all(app: &AppHandle) {
    if any_visible(app) {
        hide_all(app);
    } else {
        show_all(app);
    }
}

/// 显示全部宠物
pub fn show_all(app: &AppHandle) {
    set_all_visible(app, true);
}

/// 隐藏全部宠物
pub fn hide_all(app: &AppHandle) {
    set_all_visible(app, false);
}

/// 显示/隐藏全部宠物窗口。
///
/// 两段式：**锁内只取标签**，出锁后再 `show()/hide()`（见模块头的死锁纪律）。
fn set_all_visible(app: &AppHandle, visible: bool) {
    let labels: Vec<String> = {
        let state = app.state::<AppState>();
        let pets = watchdog::timed_lock(&state.pets, "pets（托盘显隐）");
        pets.keys().cloned().collect()
    };
    let count = labels.len();
    for label in labels {
        let Some(window) = app.get_webview_window(&label) else {
            continue;
        };
        let result = if visible { window.show() } else { window.hide() };
        if let Err(err) = result {
            eprintln!("[whale-pet] 切换窗口显隐失败 {label}：{err}");
        }
    }
    eprintln!(
        "[whale-pet] 托盘：{}全部宠物（{count} 只）",
        if visible { "显示" } else { "隐藏" }
    );
}

/// 把一条菜单动作广播给宠物页（`label = None` 表示全部）。
///
/// 与右键菜单同一条通道：菜单只负责"点了什么"，动画链、朝向、包围盒、气泡锚点都在页面手里。
pub fn dispatch_menu_action(app: &AppHandle, label: Option<&str>, action: MenuAction) {
    let labels: Vec<String> = match label {
        Some(one) => vec![one.to_string()],
        None => {
            let state = app.state::<AppState>();
            let pets = watchdog::timed_lock(&state.pets, "pets（托盘动作）");
            pets.keys().cloned().collect()
        }
    };
    for label in labels {
        if let Some(window) = app.get_webview_window(&label) {
            if let Err(err) = window.emit(EVENT_MENU_ACTION, action.clone()) {
                eprintln!("[whale-pet] 托盘动作下发失败 {label}：{err}");
            }
        }
    }
}

/// 执行一条托盘菜单动作。
///
/// **这个函数是托盘菜单的唯一执行入口**：自绘菜单页通过 `tray_menu_action` 命令调它，
/// 排障探针（`WHALE_PET_DIAG_TRAY`）也调它——于是"菜单项 → 动作"这一层只有一条代码路径，
/// 探针验过就等于菜单验过。
///
/// `action` 取值：`toggle` / `home` / `anim` / `settings` / `quit` / `close`
pub fn run_menu_action(
    app: &AppHandle,
    action: &str,
    label: Option<&str>,
    anim: Option<&str>,
) -> Result<(), String> {
    match action {
        "toggle" => {
            toggle_all(app);
            Ok(())
        }
        "home" => {
            // 点了"回到初始位置"却看不见宠物会很困惑：先确保可见，再回位
            show_all(app);
            dispatch_menu_action(app, label, MenuAction { anim: None, action: Some("home".to_string()) });
            Ok(())
        }
        "anim" => {
            let name = anim.ok_or_else(|| "动作点播缺少动画名".to_string())?;
            eprintln!("[whale-pet] 托盘菜单：点播动作 {name}");
            show_all(app);
            dispatch_menu_action(app, label, MenuAction { anim: Some(name.to_string()), action: None });
            Ok(())
        }
        "settings" => crate::settings_window::open(app),
        "quit" => {
            eprintln!("[whale-pet] 托盘菜单：退出");
            QUITTING.store(true, Ordering::SeqCst);
            app.exit(0);
            Ok(())
        }
        "close" => crate::tray_menu::hide(app),
        other => Err(format!("未知的托盘菜单动作：{other}")),
    }
}
