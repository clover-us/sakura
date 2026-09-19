//! 系统托盘：桌宠的"常驻入口"。
//!
//! 为什么托盘的菜单必须做全（M2 的出口标准之一）：
//!   宠物窗没有标题栏、也不出现在任务栏，**托盘是唯一永远可达的入口**。
//!   M0 的托盘只有"显示/隐藏/退出"三件；这一版补齐：
//!
//! ```text
//! 显示全部宠物
//! 隐藏全部宠物（点击穿透）
//! ────────────────────────
//! 回到初始位置
//! 动作点播 ▸ 待机 / 点击回应 / <随机动作分类>…
//! ────────────────────────
//! 开机自启   （勾选项）
//! 设置…
//! 退出
//! ```
//!
//! ## 两条实现约定（都踩过坑）
//!
//! 1. **动作点播复用右键菜单那条路**：托盘只组一个 [`MenuAction`] 事件发给宠物页，
//!    由宠物页执行（动画链、朝向、包围盒、气泡锚点都在页面手里）。
//!    托盘因此不需要知道任何动画语义，也不会和右键菜单出现两套行为。
//! 2. **持锁期间绝不动窗口**：`state.pets` 这把锁也会被光标轮询线程拿，
//!    一旦"持锁 + 窗口操作"（`show()/hide()` 会等窗口所属线程派发消息）撞上主线程等锁，
//!    就是一次真实死锁（复盘见 `watchdog.rs`）。所以这里一律"锁内只取标签清单，
//!    出锁后再动窗口"。

use tauri::menu::{CheckMenuItem, IsMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager};

use std::sync::atomic::{AtomicBool, Ordering};

use crate::menu_window::MenuAction;
use crate::pet_window::EVENT_MENU_ACTION;
use crate::state::AppState;
use crate::watchdog;

/// 托盘图标 id（全局唯一；重建托盘时用它先移除旧的）
pub const ID: &str = "pet-tray";

/// 用户是否明确点了「退出」。
///
/// 为什么需要这个标志：`AppHandle::exit()` 也会走 `RunEvent::ExitRequested`，
/// 而我们在那里拦了"重建期间窗口数瞬时归零"造成的误退出（见 `lib.rs` 的 run 闭包）。
/// 没有这个标志，托盘的「退出」会被自己的拦截逻辑挡住——**点退出却退不掉**。
static QUITTING: AtomicBool = AtomicBool::new(false);

/// 用户是否已请求退出（`lib.rs` 的退出拦截据此放行）
pub fn is_quitting() -> bool {
    QUITTING.load(Ordering::SeqCst)
}

/// "动作点播"菜单项的 id 前缀：`pet-anim:<分组>:<动画名>`
///
/// 为什么把分组也编进 id：同一个动画名可能同时出现在多个分组里（例如待机池与某个分类），
/// 菜单项的 id 重复会让"按 id 取项"变得有歧义。动画名本身不含冒号
/// （Windows 文件名不允许），所以用最后一个字段承载名字是安全的。
const ANIM_ID_PREFIX: &str = "pet-anim:";

/// 构造（或重建）系统托盘。
///
/// **重建**是常规操作而非异常路径：配置热重载后"动作点播"的分类会变，
/// 而托盘菜单是建好就固定的，必须换一个。同 id 重复 build 会失败，故先移除旧的。
pub fn build(app: &AppHandle) -> Result<(), Box<dyn std::error::Error>> {
    if app.tray_by_id(ID).is_some() {
        let _ = app.remove_tray_by_id(ID);
    }

    let show = MenuItem::with_id(app, "pet-show-all", "显示全部宠物", true, None::<&str>)?;
    let hide = MenuItem::with_id(app, "pet-hide-all", "隐藏全部宠物（点击穿透）", true, None::<&str>)?;
    let home = MenuItem::with_id(app, "pet-home", "回到初始位置", true, None::<&str>)?;
    let pick = animation_menu(app)?;
    // 勾选状态**直接读系统真实状态**（而不是记在内存里）：用户可能在别处
    // （任务管理器→启动项、注册表、另一个工具）改过自启，菜单必须如实反映现状
    let autostart = CheckMenuItem::with_id(
        app,
        "pet-autostart",
        "开机自启",
        true,
        crate::reload::autostart_enabled(app),
        None::<&str>,
    )?;
    let settings = MenuItem::with_id(app, "pet-settings", "设置…", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "pet-quit", "退出", true, None::<&str>)?;

    let menu = Menu::with_items(
        app,
        &[
            &show,
            &hide,
            &PredefinedMenuItem::separator(app)?,
            &home,
            &pick,
            &PredefinedMenuItem::separator(app)?,
            &autostart,
            &settings,
            &quit,
        ],
    )?;

    TrayIconBuilder::with_id(ID)
        .menu(&menu)
        // 左键点击托盘不弹菜单（Windows 上更符合桌宠类工具的直觉：左键切换显隐）
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| handle_menu_event(app, event.id.as_ref()))
        .on_tray_icon_event(|tray, event| {
            // 左键单击：全部显示（"宠物不见了"是最常见的求助场景，给一个一键找回入口）
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_all(tray.app_handle());
            }
        })
        .build(app)?;
    Ok(())
}

/// 托盘菜单项被点击。
///
/// `pub` 是为了让诊断探针（`diagnostics::spawn_tray_probe`）能按 id 走**同一条分支**：
/// 自动化环境里点不了真实托盘图标（本机的鼠标注入被桌面环境持续干扰），
/// 但"菜单项 → 动作"这一层可以用同一入口验到，剩下的只有 muda 把系统点击派发进来这一步。
pub fn handle_menu_event(app: &AppHandle, id: &str) {
    match id {
        "pet-quit" => {
            eprintln!("[whale-pet] 托盘：退出");
            // 先立旗再退：`exit()` 会触发 ExitRequested，而那里有一道"重建期间不许退"的拦截
            QUITTING.store(true, Ordering::SeqCst);
            app.exit(0);
        }
        "pet-show-all" => show_all(app),
        "pet-hide-all" => hide_all(app),
        "pet-home" => {
            // 点了"回到初始位置"却看不见宠物会很困惑：先确保可见，再回位
            show_all(app);
            dispatch_menu_action(app, MenuAction { anim: None, action: Some("home".to_string()) });
        }
        "pet-settings" => {
            if let Err(err) = crate::settings_window::open(app) {
                eprintln!("[whale-pet] 托盘：打开设置窗口失败：{err}");
            }
        }
        "pet-autostart" => {
            let now = crate::reload::autostart_enabled(app);
            if let Err(err) = crate::reload::set_autostart(app, !now) {
                eprintln!("[whale-pet] 托盘：{err}");
            }
        }
        other => match other.strip_prefix(ANIM_ID_PREFIX).and_then(|rest| rest.split_once(':')) {
            Some((_group, anim)) => {
                eprintln!("[whale-pet] 托盘：点播动作 {anim}");
                // 同上：隐藏状态下的点播用户什么都看不到
                show_all(app);
                dispatch_menu_action(app, MenuAction { anim: Some(anim.to_string()), action: None });
            }
            None => eprintln!("[whale-pet] 未处理的托盘菜单项：{other}"),
        },
    }
}

/// 构造"动作点播"子菜单：待机 / 点击回应 / 各随机动作分类。
///
/// 动作列表**来自配置**（而不是硬编码）：用户在设置窗口里改了动画池，重建托盘即可生效。
fn animation_menu(app: &AppHandle) -> Result<Submenu<tauri::Wry>, Box<dyn std::error::Error>> {
    let config = app.state::<AppState>().config_snapshot();

    let mut groups: Vec<(String, Vec<String>)> = Vec::new();
    if !config.animations.idle.is_empty() {
        groups.push(("待机".to_string(), config.animations.idle.clone()));
    }
    if !config.animations.clicks.is_empty() {
        groups.push(("点击回应".to_string(), config.animations.clicks.clone()));
    }
    for category in &config.animations.categories {
        if !category.actions.is_empty() {
            groups.push((category.id.clone(), category.actions.clone()));
        }
    }

    let mut submenus: Vec<Submenu<tauri::Wry>> = Vec::new();
    for (title, actions) in &groups {
        let items = actions
            .iter()
            .map(|name| {
                MenuItem::with_id(app, format!("{ANIM_ID_PREFIX}{title}:{name}"), name, true, None::<&str>)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let refs: Vec<&dyn IsMenuItem<tauri::Wry>> =
            items.iter().map(|item| item as &dyn IsMenuItem<tauri::Wry>).collect();
        submenus.push(Submenu::with_items(app, title, true, &refs)?);
    }

    let refs: Vec<&dyn IsMenuItem<tauri::Wry>> =
        submenus.iter().map(|sub| sub as &dyn IsMenuItem<tauri::Wry>).collect();
    Ok(Submenu::with_items(app, "动作点播", true, &refs)?)
}

/// 显示全部宠物（供托盘左键、托盘菜单、重复启动的聚焦逻辑复用）
pub fn show_all(app: &AppHandle) {
    set_all_visible(app, true);
}

/// 隐藏全部宠物
pub fn hide_all(app: &AppHandle) {
    set_all_visible(app, false);
}

/// 显示/隐藏全部宠物窗口。
///
/// 两段式：**锁内只取标签**，出锁后再 `show()/hide()`。
/// 窗口调用会等"窗口所属线程"派发消息，持锁做它就有死锁风险（见模块注释）。
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

/// 把一条菜单动作广播给所有宠物页（与右键菜单同一条通道：面板只管点，页面只管演）
fn dispatch_menu_action(app: &AppHandle, action: MenuAction) {
    let labels: Vec<String> = {
        let state = app.state::<AppState>();
        let pets = watchdog::timed_lock(&state.pets, "pets（托盘动作）");
        pets.keys().cloned().collect()
    };
    for label in labels {
        if let Some(window) = app.get_webview_window(&label) {
            if let Err(err) = window.emit(EVENT_MENU_ACTION, action.clone()) {
                eprintln!("[whale-pet] 托盘动作下发失败 {label}：{err}");
            }
        }
    }
}

/// 让托盘重新反映"开机自启"的真实状态。
///
/// 为什么是"重建整个托盘"而不是"改那一个勾选框"：Tauri 的 `TrayIcon` 只提供 `set_menu`，
/// **没有取回当前菜单的接口**（所以拿不到当初那个 `CheckMenuItem` 句柄）。而托盘菜单本来就
/// 是按系统真实状态现建的（见 `build` 里的注释），重建一次最简单也最不容易出错；
/// 触发时机只有"用户切换自启"这一种，代价可以忽略。
pub fn refresh_after_autostart_change(app: &AppHandle) {
    let handle = app.clone();
    // 菜单/托盘归主线程：投递过去执行（本函数可能被工作线程调用）
    if let Err(err) = app.run_on_main_thread(move || {
        if let Err(err) = build(&handle) {
            eprintln!("[whale-pet] 重建托盘失败：{err}");
        }
    }) {
        eprintln!("[whale-pet] 重建托盘：投递主线程失败：{err}");
    }
}
