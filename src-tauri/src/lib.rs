//! 桌宠应用主装配（lib 侧）。
//!
//! 启动顺序（顺序本身就是设计，改动前请先读注释）：
//!   1. 解析应用数据目录 → 首次运行落默认配置 → 读取并校验配置（失败即报错退出）；
//!   2. 登记自定义协议为 privileged（**必须在任何窗口创建之前**，否则 webview 不认识 pet://）；
//!   3. 建托盘；
//!   4. 创建宠物窗口（每只一个局部小窗）；
//!   5. 启动"光标采样 + 显示器几何 + 配置热重载"轮询线程（穿透自愈与跨屏边界的唯一来源）；
//!   6. 托管状态、注册命令。
//!
//! 插件策略（M0 的注释写的是"刻意不引入任何插件"，M2 按里程碑逐个加）：
//!   - `single-instance` 必须**第一个注册**：重复启动要在建窗、读配置之前就退出，
//!     并把已有实例的宠物显示出来（否则用户点两次图标会得到两只摸不到的幽灵宠物）；
//!   - `autostart` 提供"开机自启"（托盘勾选项与设置窗口共用同一条 Rust API，
//!     前端不引入对应 npm 包）。
//!
//! 模块划分：托盘在 `tray.rs`，配置热重载在 `reload.rs`，设置窗口在 `settings_window.rs`。

// 模块可见性：对二进制冒烟程序（src/bin/logic-smoke.rs）开放，
// 因为本机的 GNU 工具链跑不起 `cargo test` 的 libtest 可执行文件（见该文件头部说明）。
// 对外开放的仅是**纯逻辑**部分；命令/托盘等仍按内部实现使用。
pub mod bubble;
pub mod chat_window;
pub mod commands;
pub mod config;
pub mod display;
pub mod llm;
pub mod menu_window;
pub mod memory;
pub mod model;
pub mod pet_protocol;
pub mod reload;
pub mod secret;
pub mod settings_window;
pub mod state;
pub mod tray;
pub mod tray_menu;
pub mod whisper;
pub mod watchdog;

mod diagnostics;
mod pet_window;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tauri::{AppHandle, Emitter, Manager, RunEvent};

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
        // 单实例必须**第一个注册**（官方要求）：重复启动要在建窗/读配置之前就退出
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            eprintln!("[whale-pet] 检测到第二次启动：显示已有实例（不再开新进程）");
            let handle = app.clone();
            // 回调**不保证在主线程**上：窗口操作一律投递回主线程
            // （与兜底命中同一条纪律，理由见 `apply_fallback_hit`）
            if let Err(err) = app.run_on_main_thread(move || {
                tray::show_all(&handle);
                // 设置窗开着的话顺手提到前面——用户第二次点图标多半是想找它
                if let Some(window) = handle.get_webview_window(settings_window::LABEL) {
                    let _ = window.show();
                    let _ = window.set_focus();
                }
            }) {
                eprintln!("[whale-pet] 单实例回调投递主线程失败：{err}");
            }
        }))
        // 开机自启（Windows 上写 HKCU\...\Run；由托盘勾选项与设置窗口共用）
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
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
            // M2：设置窗口
            commands::get_settings,
            commands::save_settings,
            commands::set_autostart,
            commands::open_config_location,
            commands::close_settings,
            // 外观升级：自绘托盘菜单
            commands::get_tray_menu_state,
            commands::tray_menu_action,
            commands::resize_tray_menu,
            // M3：AI（状态 / 密钥 / 自检 / 碎碎念 / 对话 / 记忆）
            commands::llm_status,
            commands::llm_save_key,
            commands::llm_clear_key,
            commands::llm_selftest,
            commands::llm_whisper_now,
            commands::llm_chat,
            commands::open_chat,
            commands::close_chat,
            commands::llm_memory,
            commands::llm_memory_clear,
        ])
        .build(tauri::generate_context!())
        .expect("初始化 Tauri 应用失败")
        .run(|_app, event| {
            // ---- 退出拦截：**只拦一种情况** ----
            //
            // 热重载/保存设置会"拆掉全部宠物窗 → 建新的"。拆的那一瞬间窗口数为 0，
            // Tauri 会认为"最后一个窗口被关了"并发来 ExitRequested——如果不拦，
            // 用户改一次配置，**应用直接退出**（实测踩过：日志里"热重载开始"之后紧跟"应用退出"）。
            //
            // 只在"确实正在重建"时拦：托盘的「退出」会先立 QUITTING 旗（`tray::is_quitting`），
            // 因此正常退出不受影响；用户主动关掉设置窗也不会被拦（宠物窗还在）。
            if let RunEvent::ExitRequested { api, .. } = event {
                if tray::is_quitting() {
                    eprintln!("[whale-pet] 应用退出（用户请求）");
                } else if reload::is_applying() {
                    api.prevent_exit();
                    eprintln!("[whale-pet] 已拦截一次误退出：重建窗口期间窗口数瞬时归零（配置热重载/保存设置的正常现象）");
                } else {
                    eprintln!("[whale-pet] 应用退出");
                }
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
        let config = state.config_snapshot();
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
            // 对话输入窗（M3）：同样是"预建 + 隐藏"，点了就出现
            if let Err(err) = chat_window::prepare(app, &label) {
                eprintln!("[whale-pet] 预备对话窗失败 {label}：{err}");
            }
        }
    }

    // ---- 5. 托盘 ----
    build_tray(app)?;

    // ---- 5.2 托盘菜单窗：预建并隐藏（"点了就出现"，与右键菜单同一考虑）----
    if let Err(err) = tray_menu::prepare(app) {
        // 预备失败不该拦住启动：`tray_menu::show` 里还有一条"按需创建"的后路
        eprintln!("[whale-pet] 预备托盘菜单窗失败：{err}");
    }

    // ---- 5.5 配置热重载的指纹基线 ----
    // 必须在启动时对齐一次：否则第一次轮询会拿 `None` 与当前文件比，判成"被修改"
    // （实测后果：启动 ~1 秒后白重建一次宠物窗，并顺带触发一次误退出）。
    reload::mark_stamp_current(&config_path);
    eprintln!("[whale-pet] 配置热重载已就绪（每 {}ms 检查一次文件变化）", reload::POLL_MS);

    // ---- 6. 轮询线程（光标采样 + 显示器几何 + 配置热重载） ----
    spawn_poll_loop(app.clone());
    // ---- 6.5 主线程健康看门狗（**独立线程**：轮询线程自己也可能是被卡住的那个，见 watchdog.rs） ----
    watchdog::spawn_main_watchdog(app.clone());

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

    // ---- 7b. 设置窗口探针（`WHALE_PET_DIAG_SETTINGS=1|save|autostart|addpet|delpet|nav:<页>`）----
    if let Ok(value) = std::env::var("WHALE_PET_DIAG_SETTINGS") {
        if !value.trim().is_empty() {
            eprintln!("[whale-pet] 启用设置窗口探针（动作={value}）");
            diagnostics::spawn_settings_probe(app.clone(), value);
        }
    }

    // ---- 7c. 托盘探针（`WHALE_PET_DIAG_TRAY=pet-hide-all,pet-home,…`，见 diagnostics）----
    if let Ok(sequence) = std::env::var("WHALE_PET_DIAG_TRAY") {
        let items: Vec<String> =
            sequence.split(',').map(|item| item.trim().to_string()).filter(|s| !s.is_empty()).collect();
        if !items.is_empty() {
            eprintln!("[whale-pet] 启用托盘探针：{} 个菜单项", items.len());
            diagnostics::spawn_tray_probe(app.clone(), items);
        }
    }

    // ---- 7d. 托盘菜单样式探针（`WHALE_PET_DIAG_TRAY_MENU=<毫秒>[:picker|:picker-expanded]）----
    if let Ok(value) = std::env::var("WHALE_PET_DIAG_TRAY_MENU") {
        let (delay_text, mode) = match value.split_once(':') {
            Some((delay, mode)) => (delay, mode),
            None => (value.as_str(), ""),
        };
        let delay: u64 = delay_text.parse().unwrap_or(4000);
        let mode: &'static str = match mode {
            "picker" => "picker",
            "picker-expanded" => "picker-expanded",
            _ => "",
        };
        eprintln!("[whale-pet] 启用托盘菜单探针（{delay}ms 后弹出，模式={mode}）");
        diagnostics::spawn_tray_menu_probe(app.clone(), delay, mode);
    }

    // ---- 7e. AI 链路探针（`WHALE_PET_DIAG_LLM=status|selftest|whisper|chat|all[:延时ms]`）----
    if let Ok(value) = std::env::var("WHALE_PET_DIAG_LLM") {
        if !value.trim().is_empty() {
            // `savekey` 不需要延时（它只是写一次密钥库）
            let (action, delay) = match value.split_once(':') {
                Some((action, delay)) => (action.to_string(), delay.parse::<u64>().unwrap_or(3000)),
                None => {
                    let action = value.clone();
                    let delay = if action == "savekey" { 800 } else { 3000 };
                    (action, delay)
                }
            };
            eprintln!("[whale-pet] 启用 AI 链路探针（动作={action}，延时={delay}ms）");
            diagnostics::spawn_llm_probe(app.clone(), action, delay);
        }
    }

    // ---- 7f. 对话窗探针（`WHALE_PET_DIAG_CHAT=<毫秒>[:消息]`）----
    if let Ok(value) = std::env::var("WHALE_PET_DIAG_CHAT") {
        if !value.trim().is_empty() {
            let (delay, message) = match value.split_once(':') {
                Some((delay, message)) => (delay.parse::<u64>().unwrap_or(3000), message.to_string()),
                None => (value.parse::<u64>().unwrap_or(3000), String::new()),
            };
            eprintln!("[whale-pet] 启用对话窗探针（{delay}ms 后发送：{message}）");
            diagnostics::spawn_chat_probe(app.clone(), delay, message);
        }
    }

    Ok(())
}

/// 构造系统托盘（M2 完整版：显隐 / 回位 / 动作点播 / 自启 / 设置 / 退出）。
///
/// 实现全在 [`crate::tray`]：那里同时说明了"动作点播为什么复用右键菜单那条通道"
/// 与"持锁期间为什么绝不能动窗口"两条约定。
fn build_tray(app: &AppHandle) -> Result<(), Box<dyn std::error::Error>> {
    tray::build(app)
}

/// 启动轮询线程：光标采样（穿透自愈 + 拖拽兜底）、显示器几何变化检测、配置热重载。
///
/// 为什么这三件事共用一个线程：
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
        let mut last_config_check = Instant::now();
        let mut last_whisper_check = Instant::now();
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
                    // 托盘菜单同理（两者互斥显示，但关闭判定各管各的）
                    tray_menu::close_on_outside_press(
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

            // ---- 碎碎念（每秒问一次"到点了吗"，真正的请求丢给工作线程）----
            if now.duration_since(last_whisper_check) >= Duration::from_millis(1000) {
                last_whisper_check = now;
                whisper::tick(&app);
            }

            // ---- 配置热重载 ----
            // 这里**只做发现**：解析 + 比对指纹很便宜（毫秒级），而"重建窗口"是重活，
            // 由 `reload` 起独立工作线程去做，绝不压在轮询线程上（见 reload.rs 的模块注释）。
            if now.duration_since(last_config_check) >= Duration::from_millis(reload::POLL_MS) {
                last_config_check = now;
                reload::poll(&app);
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
        let pets = watchdog::timed_lock(&state.pets, "pets（广播光标）");
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
/// 用包围盒会和前端的精确判定互相打架，详见下面的注释。
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
///
/// ## 两段式写法（**这里曾经死锁过，不要再合并回一段**）
///
/// 本函数跑在光标轮询线程上，而"翻转穿透"最终会走到 `SetWindowLongW`——目标窗口归
/// **主线程**所有，跨线程改样式时 Windows 会 `SendMessage` 给主线程并**等它派发**。
/// 于是老写法（持锁 + 在锁内翻转穿透）构成一个死锁环：
///   轮询线程持有 `state.pets` 锁 → 等主线程派发样式消息 → 主线程正卡在 `state.pets` 锁上。
/// 现象是"宠物停在半空、点不动、日志戛然而止、CPU 0"，完整复盘见 `watchdog.rs`。
///
/// 因此固定成两步：
///   ① 持锁只做"读 + 记账"（`plan_interactive`），**绝不动窗口**；
///   ② 出锁之后把"落样式"投递给主线程（`run_on_main_thread`，非阻塞）——
///      轮询线程既不持锁做窗口调用，也不会阻塞在任何窗口调用上，死锁环从结构上不成立。
fn apply_fallback_hit(app: &AppHandle, cursor_x: f64, cursor_y: f64) {
    // ① 持锁期：判定 + 记账，产出"需要恢复交互"的标签清单
    let planned: Vec<String> = {
        let state = app.state::<AppState>();
        let mut pets = watchdog::timed_lock(&state.pets, "pets（兜底命中判定）");
        let mut planned = Vec::new();
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
            if runtime.plan_interactive(true).is_some() {
                planned.push(label.clone());
            }
        }
        planned
    };

    // ② 出锁期：把"落样式"投递给主线程（= 窗口所属线程），失败只记日志
    for label in planned {
        let poster = app.clone();
        let target = label.clone();
        if let Err(err) = app.run_on_main_thread(move || {
            let Some(window) = poster.get_webview_window(&target) else {
                return;
            };
            if let Err(err) = pet_window::apply_interactive_style(&window, true) {
                eprintln!("[whale-pet] 兜底恢复交互失败 {target}：{err}");
            }
        }) {
            eprintln!("[whale-pet] 兜底恢复交互：投递主线程失败 {label}：{err}");
        }
    }
}
