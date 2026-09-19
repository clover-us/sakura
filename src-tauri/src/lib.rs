//! 桌宠应用主装配（lib 侧）。
//!
//! 启动顺序（顺序本身就是设计，改动前请先读注释）：
//!   1. 解析应用数据目录 → 首次运行落默认配置 → 读取并校验配置（失败即报错退出）；
//!   2. 登记自定义协议为 privileged（**必须在任何窗口创建之前**，否则 webview 不认识 pet://）；
//!   3. 建托盘；
//!   4. 创建宠物窗口（每只一个局部小窗）；
//!   5. 启动"光标采样 + 显示器几何"轮询线程（穿透自愈与跨屏边界的唯一来源）；
//!   6. 托管状态、注册命令。
//!
//! 为什么没有用 `tauri::Builder::default().run()` + 插件堆：
//!   M0 刻意不引入任何 Tauri 插件（托盘除外），把依赖面压到最小，
//!   先把"透明窗 / 穿透 / 跟手 / 几何"这四件真正有风险的事验证完，
//!   再按里程碑逐个引入插件（见 docs/ROADMAP.md）。

// 模块可见性：对二进制冒烟程序（src/bin/logic-smoke.rs）开放，
// 因为本机的 GNU 工具链跑不起 `cargo test` 的 libtest 可执行文件（见该文件头部说明）。
// 对外开放的仅是**纯逻辑**部分；命令/托盘等仍按内部实现使用。
pub mod bubble;
pub mod commands;
pub mod config;
pub mod display;
pub mod menu_window;
pub mod model;
pub mod pet_protocol;
pub mod state;

mod diagnostics;
mod pet_window;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, Manager, RunEvent,
};

use crate::config::{ensure_default_config, load_config};
use crate::model::CursorSample;
use crate::pet_window::{EVENT_CURSOR, EVENT_DISPLAYS};
use crate::state::AppState;

/// 光标采样周期（毫秒）。
///
/// 16ms ≈ 60Hz 的取值依据：
///   - 这一路采样现在承担**两件事**：① 穿透↔可交互的翻转判定；② 拖拽状态机的
///     全局位置与**按键**来源（原因见 `CursorSample::primary_down` 的注释）。
///     16ms 的粒度让"按下/松手"的检出延迟与显示刷新同量级，不会再出现
///     上游那个 60ms 兜底轮询的粗粒度问题；
///   - 单次成本极低（一次 `GetAsyncKeyState` + 一次光标坐标 + 每个宠物窗口一次事件 emit），
///     实测不影响窗口移动的流畅度（拖拽期间窗口仍以 ~63 次/秒被移动）。
///
/// 拖拽的实时性**不依赖**它：拖拽期间窗口处于可交互态，页面的 DOM 指针事件仍在到达，
/// 这条路只是"DOM 事件丢失时的兜底 + 权威按键状态"。
const CURSOR_POLL_MS: u64 = 16;

/// 显示器几何检查周期（毫秒）。分辨率/缩放变化是低频事件，1.5s 的发现延迟无感。
const DISPLAY_POLL_MS: u64 = 1500;

/// 应用入口
pub fn run() {
    tauri::Builder::default()
        // 自定义协议登记：privileged 让它具备 fetch/stream/CORS 能力，
        // 否则 webview 会把它当成普通未知协议直接拒绝（视频无法播放）。
        .register_asynchronous_uri_scheme_protocol(pet_protocol::SCHEME, move |ctx, request, responder| {
            // 协议回调运行在 webview 的 IO 线程上：**不做任何可能阻塞的事**，
            // 读文件放到独立线程里执行，读完再 respond。
            let app_data_dir = ctx.app_handle().state::<AppState>().app_data_dir.clone();
            std::thread::spawn(move || {
                let response = pet_protocol::handle_request(&app_data_dir, &request);
                responder.respond(response);
            });
        })
        .setup(|app| {
            let handle = app.handle().clone();
            setup_app(&handle)?;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_pet_config,
            commands::get_pet_runtime,
            commands::set_pet_bounds,
            commands::set_pet_interactive,
            commands::set_pet_input_busy,
            commands::report_pet_error,
            commands::pet_debug_log,
            commands::get_displays,
            commands::get_asset_base_url,
            commands::raise_pet_window,
            commands::debug_synthetic_press,
            commands::debug_synthetic_drag,
            commands::debug_synthetic_right_click,
            commands::show_bubble,
            commands::hide_bubble,
            commands::show_menu,
            commands::hide_menu,
            commands::menu_action,
            commands::debug_inject_right_click,
        ])
        .build(tauri::generate_context!())
        .expect("初始化 Tauri 应用失败")
        .run(|_app, event| {
            // 目前不需要拦截任何运行时事件；保留此闭包以便将来处理 ExitRequested（托盘常驻时用）
            if let RunEvent::ExitRequested { .. } = event {
                eprintln!("[whale-pet] 应用退出");
            }
        });
}

/// 启动装配（setup 阶段调用；出错即让应用启动失败——配置错误必须立刻可见）
fn setup_app(app: &AppHandle) -> Result<(), Box<dyn std::error::Error>> {
    // ---- 1. 配置 ----
    let app_data_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("无法解析应用数据目录：{e}"))?;
    std::fs::create_dir_all(&app_data_dir)
        .map_err(|e| format!("创建应用数据目录失败 {}：{e}", app_data_dir.display()))?;
    let config_path = ensure_default_config(&app_data_dir)
        .map_err(|e| format!("准备默认配置失败：{e}"))?;
    // 释放内置示例动画（只在文件缺失时写）：保证"首次运行就能看到活的宠物"
    match config::ensure_sample_animations(&app_data_dir) {
        Ok(0) => {}
        Ok(count) => eprintln!("[whale-pet] 已释放 {count} 条内置示例动画到 {}", app_data_dir.join("webm").display()),
        Err(err) => eprintln!("[whale-pet] 释放示例动画失败：{err}"),
    }
    let config = load_config(&config_path).map_err(|e| {
        // 配置出错是最常见的用户故障：把"文件在哪"和"怎么改"一起打出来，省一轮排查
        eprintln!("[whale-pet] 配置加载失败：{e}");
        eprintln!("[whale-pet] 配置文件位置：{}", config_path.display());
        eprintln!("[whale-pet] 可直接编辑该文件（支持 // 注释），或删除它让程序重新生成默认配置。");
        e
    })?;
    eprintln!(
        "[whale-pet] 配置就绪：{} 只宠物，配置目录 {}",
        config.pets.len(),
        app_data_dir.display()
    );

    // ---- 2. 初始几何（供前端首帧拉取与窗口定位） ----
    let (sample, fingerprint) = display::current_sample(app).map_err(|e| format!("读取显示器几何失败：{e}"))?;
    // 启动即校验几何内在一致性（工作区必须被同序面板包含）：不合法立刻报错，
    // 而不是让宠物带着错误的边界跑起来（抛掷会穿屏或卡在屏缝）
    display::validate_geometry(&sample).map_err(|e| format!("显示器几何不合法：{e}"))?;
    eprintln!(
        "[whale-pet] 显示器：工作区 {}；面板 {}；主屏 {}",
        display::describe(&sample.areas),
        display::describe(&sample.panels),
        display::describe(&[sample.primary])
    );

    // ---- 3. 状态托管（必须在建窗口之前：窗口创建过程会读取状态里的几何） ----
    let state = AppState::new(config, app_data_dir.clone());
    state.update_displays(sample, fingerprint);
    app.manage(state);

    // ---- 4. 宠物窗口 ----
    {
        // 诊断：确认前端产物是否真的被解析到（"页面不加载"最常见的原因就是产物路径不对）
        let resource_dir = app.path().resource_dir().map_err(|e| format!("解析资源目录失败：{e}"))?;
        eprintln!(
            "[whale-pet] 资源目录：{}（index.html 存在：{}）",
            resource_dir.display(),
            resource_dir.join("index.html").is_file()
        );

        let state = app.state::<AppState>();
        let config = state.config.clone();
        let runtimes = pet_window::create_all(app, &config, &app_data_dir)
            .map_err(|e| format!("创建宠物窗口失败：{e}"))?;
        let count = runtimes.len();
        let mut pets = state.pets.lock().map_err(|_| "宠物窗口表锁已被污染")?;
        *pets = runtimes;
        eprintln!("[whale-pet] 已创建 {count} 个宠物窗口");
    }

    // ---- 4.5 气泡窗 / 菜单窗：**预先建好并隐藏** ----
    // 第一次 `say()` / 第一次右键才建窗的话，WebView2 建窗 + 页面加载要几百毫秒，
    // 用户点完要愣一下才看到东西。预建后显示时只改尺寸/位置（见 bubble::prepare / menu_window::prepare）。
    {
        let state = app.state::<AppState>();
        let pet_labels: Vec<String> = state
            .pets
            .lock()
            .map(|pets| pets.keys().cloned().collect())
            .unwrap_or_default();
        for label in pet_labels {
            if let Err(err) = bubble::prepare(app, &label) {
                // 预备失败不该拦住启动：`show_bubble` 里还有"按需创建"这条后路
                eprintln!("[whale-pet] 预备气泡窗失败 {label}：{err}");
            }
            if let Err(err) = menu_window::prepare(app, &label) {
                eprintln!("[whale-pet] 预备菜单窗失败 {label}：{err}");
            }
        }
    }

    // ---- 5. 托盘 ----
    build_tray(app)?;

    // ---- 6. 轮询线程（光标采样 + 显示器几何） ----
    spawn_poll_loop(app.clone());

    // ---- 7. 排障探针（仅在设置 WHALE_PET_DIAG_PRESS 时启用，见 diagnostics 里的说明） ----
    match std::env::var("WHALE_PET_DIAG_PRESS").as_deref() {
        Ok("1") | Ok("drag") | Ok("right") | Ok("menu") => {
            let state = app.state::<AppState>();
            let first_label = state
                .pets
                .lock()
                .ok()
                .and_then(|pets| pets.keys().next().cloned());
            if let Some(label) = first_label {
                eprintln!("[whale-pet] 启用合成输入探针（窗口 {label}）");
                diagnostics::spawn_synthetic_press_probe(app.clone(), label);
            }
        }
        _ => {}
    }

    Ok(())
}

/// 构造系统托盘：快速退出 + 显示/隐藏全部宠物
///
/// M0 的托盘只是"能退出"的最小闭环：桌宠窗口没有标题栏，没有退出入口会让用户
/// 只能去任务管理器结束进程。完整的托盘菜单（设置/回位/动作点播）在 M2 接入。
fn build_tray(app: &AppHandle) -> Result<(), Box<dyn std::error::Error>> {
    let show = MenuItem::with_id(app, "pet-show-all", "显示全部宠物", true, None::<&str>)?;
    let hide = MenuItem::with_id(app, "pet-hide-all", "隐藏全部宠物（点击穿透）", true, None::<&str>)?;
    let separator = PredefinedMenuItem::separator(app)?;
    let quit = MenuItem::with_id(app, "pet-quit", "退出", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &hide, &separator, &quit])?;

    TrayIconBuilder::with_id("pet-tray")
        .menu(&menu)
        // 左键点击托盘不弹菜单（Windows 上更符合桌宠类工具的直觉：左键切换显隐）
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "pet-quit" => {
                eprintln!("[whale-pet] 托盘：退出");
                app.exit(0);
            }
            "pet-show-all" => toggle_all_pets(app, true),
            "pet-hide-all" => toggle_all_pets(app, false),
            other => eprintln!("[whale-pet] 未处理的托盘菜单项：{other}"),
        })
        .on_tray_icon_event(|tray, event| {
            // 左键单击：全部显示（"宠物不见了"是最常见的求助场景，给一个一键找回入口）
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                toggle_all_pets(tray.app_handle(), true);
            }
        })
        .build(app)?;
    Ok(())
}

/// 显示/隐藏全部宠物窗口（隐藏时同时打开穿透，避免不可见窗口吃掉鼠标事件）
fn toggle_all_pets(app: &AppHandle, visible: bool) {
    let state = app.state::<AppState>();
    let Ok(pets) = state.pets.lock() else {
        eprintln!("[whale-pet] 宠物窗口表锁已被污染，忽略托盘操作");
        return;
    };
    for (label, runtime) in pets.iter() {
        let result = if visible {
            runtime.window.show()
        } else {
            runtime.window.hide()
        };
        if let Err(err) = result {
            eprintln!("[whale-pet] 切换窗口显隐失败 {label}：{err}");
        }
    }
}

/// 启动轮询线程：光标采样（穿透自愈 + 拖拽兜底）与显示器几何变化检测。
///
/// 为什么这两件事共用一个线程：
///   它们都是"低频、必须与 UI 线程解耦"的周期任务，且都需要 AppHandle。
///   合并后只有一个线程与一个停止标志，生命周期管理简单；50ms 的单次成本也远低于
///   线程切换开销。
fn spawn_poll_loop(app: AppHandle) {
    let running = Arc::new(AtomicBool::new(true));
    let started_at = Instant::now();
    // 诊断：`WHALE_PET_DIAG_CURSOR=1` 时把**宿主读到并广播出去**的光标值打出来。
    // 用途：与页面侧 `?autotest=3` 的探针日志对照，判断"采样坐标不可信"到底出在哪一段
    // （宿主读取 → IPC/serde → 前端保存）。默认关闭，不参与运行路径。
    let log_cursor = std::env::var("WHALE_PET_DIAG_CURSOR").as_deref() == Ok("1");
    let mut cursor_log_counter: u32 = 0;

    std::thread::spawn(move || {
        let mut last_display_check = Instant::now();
        // 上一帧的左键状态：用来识别"按下的那一瞬间"（菜单的"点外面关掉"靠它）
        let mut prev_primary_down = false;
        while running.load(Ordering::Relaxed) {
            let now = Instant::now();

            // ---- 光标采样 ----
            if let Some(position) = display::cursor_position(&app) {
                let primary_down = display::primary_button_down();
                let sample = CursorSample {
                    position: crate::model::Vec2 { x: position.x, y: position.y },
                    at: started_at.elapsed().as_millis() as u64,
                    // 按键状态与位置同源采样：前端据此驱动/收尾拖拽状态机
                    primary_down,
                };
                if log_cursor {
                    cursor_log_counter += 1;
                    // 每 ~0.5 秒一行（16ms 一采样），够与外部探针/前端日志对齐
                    if cursor_log_counter % 30 == 1 {
                        eprintln!(
                            "[whale-pet][光标采样] 宿主=({:.0},{:.0}) 按键={} t={}ms",
                            sample.position.x, sample.position.y, sample.primary_down, sample.at
                        );
                    }
                }
                broadcast_cursor(&app, sample);
                // 兜底命中（仅负责"回到可交互"这一个方向，理由见 apply_fallback_hit）
                apply_fallback_hit(&app, position.x, position.y);
                // 菜单开着时"在菜单外按下鼠标"就关掉它（菜单窗不可聚焦，拿不到失焦事件）
                if primary_down && !prev_primary_down {
                    menu_window::close_on_outside_press(
                        &app,
                        crate::model::Vec2 { x: position.x, y: position.y },
                    );
                }
                prev_primary_down = primary_down;
            }

            // ---- 显示器几何 ----
            if now.duration_since(last_display_check) >= Duration::from_millis(DISPLAY_POLL_MS) {
                last_display_check = now;
                match display::current_sample(&app) {
                    Ok((sample, fingerprint)) => {
                        let state = app.state::<AppState>();
                        if state.update_displays(sample.clone(), fingerprint) {
                            pet_window::log_geometry_change(&sample.areas);
                            let _ = app.emit(EVENT_DISPLAYS, sample);
                        }
                    }
                    Err(err) => eprintln!("[whale-pet] 读取显示器几何失败：{err}"),
                }
            }

            std::thread::sleep(Duration::from_millis(CURSOR_POLL_MS));
        }
    });
}

/// 把光标采样广播给所有宠物窗口（前端据此做命中判定与拖拽跟随）
fn broadcast_cursor(app: &AppHandle, sample: CursorSample) {
    // 先把标签列表取出来，**别在持锁期间做跨 webview 调用**（避免长持有与死锁风险）
    let labels: Vec<String> = {
        let state = app.state::<AppState>();
        let pets = match state.pets.lock() {
            Ok(pets) => pets,
            Err(_) => return,
        };
        let labels = pets.keys().cloned().collect();
        labels
    };
    for label in labels {
        if let Some(window) = app.get_webview_window(&label) {
            let _ = window.emit(EVENT_CURSOR, sample);
        }
    }
}

/// 兜底命中：当光标进入某只宠物的**身体命中区**时，把窗口恢复为"可交互"。
///
/// 注意判据是**命中区**（与前端 `hitbox.ts::scaleHitBox` 同一套几何），不是整个包围盒——
/// 用包围盒会和前端的精确判定互相打架，详见函数内部的注释。
///
/// 为什么只做"恢复"这一个方向（这一点与上游 Electron 实现不同，务必理解）：
///   - Electron 的 `setIgnoreMouseEvents(true, { forward: true })` 在穿透时仍会转发鼠标移动，
///     页面能自己发现"光标进来了"再翻转；Tauri 没有 forward，穿透状态下页面**收不到任何事件**，
///     如果没有 Rust 侧这条兜底通道，宠物一旦进入穿透就再也无法交互。
///   - 反向（"光标离开 → 翻回穿透"）在这里**故意不做**：
///       * 精确判定归前端（它知道真实的身体命中区），几何盲判只作为"恢复"手段；
///       * 拖拽时宠物由弹簧追赶光标、滞后于光标，按几何盲判会在光标滑出包围盒时
///         误判为"用户离开了"而翻回穿透——正在进行的拖拽当场断掉（这正是上游
///         inputBusy 那条注释描述的事故）。因此 busy 期间绝对不动。
///   - 结果是：最坏情况只是窗口多保持一会儿可交互（透明区域仍由页面自己的命中区约束），
///     不会出现"点不到宠物"或"拖拽断掉"这两类致命问题。
fn apply_fallback_hit(app: &AppHandle, cursor_x: f64, cursor_y: f64) {
    let state = app.state::<AppState>();
    let Ok(mut pets) = state.pets.lock() else {
        return;
    };
    for (label, runtime) in pets.iter_mut() {
        if runtime.state.input_busy || runtime.state.interactive {
            continue;
        }
        // **必须用"身体命中区"判定，而不是整个包围盒**（这里踩过一个很隐蔽的坑）：
        //
        // 前端用的是命中区（`hitbox.ts::scaleHitBox`，只有身体那一块），包围盒里
        // "命中区之外的那一圈"是透明像素、前端判定为**应当穿透**。早期这里按包围盒来判，
        // 于是两边**互相打架**：前端翻成穿透 → 兜底立刻翻回可交互 → 前端下一帧又翻成穿透 → …
        // 日志特征很好认（用户实测日志里成对出现）：
        //
        // ```text
        // [whale-pet] 可交互=false → transparent=true      ← 前端要求穿透
        // [whale-pet] 可交互=true  → transparent=false     ← 兜底立刻翻回来（前端没有对应日志）
        // ```
        //
        // 后果有两个，用户都报了：**那一圈时而吃点击时而不吃**（点不动/抓不住），
        // 以及"拖拽释放后感觉宠物不听使唤"。改成与前端同一套命中几何之后，
        // 两边在同一时刻必然得出同一结论，不可能再打架。
        let origin = runtime.state.box_origin();
        let hit = runtime.config.hit_box;
        let inside = cursor_x >= origin.x + hit.x
            && cursor_x < origin.x + hit.x + hit.width
            && cursor_y >= origin.y + hit.y
            && cursor_y < origin.y + hit.y + hit.height;
        if !inside {
            continue;
        }
        if let Err(err) = runtime.apply_interactive(true) {
            eprintln!("[whale-pet] 兜底恢复交互失败 {label}：{err}");
        }
    }
}
