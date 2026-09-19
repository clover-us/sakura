//! 配置热重载："改文件 → 约 1 秒内生效"，以及设置窗口保存后的"立即应用"。
//!
//! ## 为什么要热重载
//!
//! M1 改配置必须重启应用才生效。桌宠是常驻程序，用户（或设置窗口）改一次大小、
//! 换一次动画池就要重启一次，体验上完全不像"正经应用"（M2 的出口标准正是"不碰配置文件
//! 也能完成全部常用设置"，而设置窗口必须**立刻看得见效果**才算完成）。
//!
//! ## 两条路，同一个落点
//!
//! ```text
//! 设置窗口点保存 ──► config::save_config（校验 + 备份 + 原子写）──► apply_config
//! 外部编辑器改文件 ─► poll()（轮询线程发现指纹变化）──► 工作线程 ──► apply_config
//! ```
//!
//! ## 纪律：轮询线程只"发现"，重活交给工作线程
//!
//! 光标轮询线程每 16ms 跑一圈，它同时负责穿透自愈（点不到宠物/拖拽断掉那类问题的兜底）。
//! 在里面重建窗口会把它卡住几百毫秒——期间所有兜底判定停摆，用户体感就是"改完配置后
//! 宠物僵了一下"。因此 `poll()` 只做三件事：比对文件指纹、读+校验配置、起一个工作线程；
//! 拆窗/建窗/重建托盘全在工作线程上做，并用一个 `APPLYING` 标志防止并发重入。
//!
//! ## 为什么"重建窗口"而不是"就地改参数"
//!
//! 配置里能变的包括宠物数量与 id——就地改参数无法表达"多了一只/少了一只"
//! （窗口标签 `pet-<id>-<index>` 都会变）。统一走"拆掉重建"只有一条代码路径，
//! 也不会出现"窗口还是旧的、账本已经是新的"这种半新半旧状态。
//! 代价是一次几百毫秒的重建，对"用户主动改配置"这个场景完全可以接受。

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};

use tauri::{AppHandle, Manager};
use tauri_plugin_autostart::ManagerExt;

use crate::config::{self, AppConfig};
use crate::state::AppState;
use crate::watchdog;

/// 配置文件检查周期（毫秒）：外部编辑器保存到生效的最大延迟
pub const POLL_MS: u64 = 1000;

/// 配置文件的"指纹"：修改时间 + 字节数。任一变化即认为文件被改过。
#[derive(Clone, Debug, PartialEq, Eq)]
struct Stamp {
    modified: Option<SystemTime>,
    len: u64,
}

fn read_stamp(path: &Path) -> Option<Stamp> {
    let meta = std::fs::metadata(path).ok()?;
    Some(Stamp { modified: meta.modified().ok(), len: meta.len() })
}

/// 上一次已知的文件指纹（`None` = 还没建立基线，第一次 poll 只建立基线不重载）
static LAST_STAMP: Mutex<Option<Stamp>> = Mutex::new(None);

/// 是否有一次"应用新配置"正在进行。
///
/// 作用有两个：① 防止两次重载并发拆窗建窗（会互相踩标签）；
/// ② 让轮询线程在重活期间直接跳过（它不该被拖住，见模块注释）。
static APPLYING: AtomicBool = AtomicBool::new(false);

/// 建立文件指纹基线（启动时、以及**我们自己写盘之后**调用）。
///
/// 自己写盘后必须调它：否则 1 秒内轮询线程会把"我们刚写的文件"当成外部修改，
/// 再白重建一次窗口。
pub fn mark_stamp_current(path: &Path) {
    let stamp = read_stamp(path);
    let mut last = match LAST_STAMP.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    *last = stamp;
}

/// 轮询线程调用：发现配置文件变化就重载（只负责发现与调度）。
pub fn poll(app: &AppHandle) {
    if APPLYING.load(Ordering::Relaxed) {
        return;
    }
    let path = config::config_file_path(&app.state::<AppState>().app_data_dir);

    let Some(current) = read_stamp(&path) else {
        return;
    };
    {
        let mut last = match LAST_STAMP.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        if last.as_ref() == Some(&current) {
            return;
        }
        // 先记账再解析：解析失败时也不能反复重试同一个坏文件（会刷爆日志）
        *last = Some(current);
    }

    let next = match config::load_config(&path) {
        Ok(next) => next,
        Err(err) => {
            // 编辑器"写一半"或用户改错都会走到这里：**保留当前配置**继续跑，
            // 把原因写在日志里。用户改好之后指纹会再变一次，那时自然重载。
            eprintln!("[whale-pet][热重载] 配置文件目前读不动，继续用当前配置：{err}");
            return;
        }
    };

    if same_config(app, &next) {
        eprintln!("[whale-pet][热重载] 配置文件指纹变了但内容没变，忽略");
        return;
    }
    eprintln!("[whale-pet][热重载] 检测到配置文件被修改，开始应用");
    spawn_apply(app.clone(), next, "配置文件被外部修改");
}

/// 两份配置在"配置语义"上是否相同。
///
/// **必须用规范 JSON 比较**（键名排序）：`animations.events` 是 `HashMap`，
/// 直接比序列化字符串会永远判为"不同"，于是启动时就会白重建一次窗口
/// （复盘见 `config::canonical_json` 的注释——那一次重建还顺带触发过一个"应用直接退出"）。
fn same_config(app: &AppHandle, other: &AppConfig) -> bool {
    let current = app.state::<AppState>().config_snapshot();
    config::canonical_json(&current) == config::canonical_json(other)
}

/// 是否正处于"应用新配置"的过程中（重建期间窗口数会瞬间归零，见 `lib.rs` 的退出拦截）
pub fn is_applying() -> bool {
    APPLYING.load(Ordering::SeqCst)
}

/// 起一个工作线程应用新配置（重活不能压在轮询线程上）
pub fn spawn_apply(app: AppHandle, next: AppConfig, reason: &'static str) {
    if APPLYING.swap(true, Ordering::SeqCst) {
        eprintln!("[whale-pet][热重载] 上一次应用还没结束，跳过本次（{reason}）");
        return;
    }
    std::thread::spawn(move || {
        let started = Instant::now();
        match apply_config(&app, &next) {
            Ok(count) => eprintln!(
                "[whale-pet][热重载] 已应用（{reason}）：{count} 只宠物，用时 {}ms",
                started.elapsed().as_millis()
            ),
            Err(err) => eprintln!("[whale-pet][热重载] 应用失败（{reason}）：{err}"),
        }
        APPLYING.store(false, Ordering::SeqCst);
    });
}

/// **在当前线程**应用新配置（设置窗口保存时调用：它本身就跑在工作线程上）。
///
/// 与 [`spawn_apply`] 共用同一个 `APPLYING` 标志，因此两条路不会并发拆窗。
pub fn apply_now(app: &AppHandle, next: &AppConfig, reason: &'static str) -> Result<usize, String> {
    if APPLYING.swap(true, Ordering::SeqCst) {
        return Err("另一次配置应用正在进行中，请稍后再试".to_string());
    }
    let started = Instant::now();
    let result = apply_config(app, next);
    APPLYING.store(false, Ordering::SeqCst);
    match &result {
        Ok(count) => eprintln!(
            "[whale-pet][热重载] 已应用（{reason}）：{count} 只宠物，用时 {}ms",
            started.elapsed().as_millis()
        ),
        Err(err) => eprintln!("[whale-pet][热重载] 应用失败（{reason}）：{err}"),
    }
    result
}

/// 按新配置重建所有宠物窗口。
///
/// 步骤顺序是设计的一部分：
///   1. **拆旧窗**（宠物窗 + 它们的气泡窗/菜单窗）——标签会重复使用，旧窗必须先消失；
///   2. **换账本**（`AppState::set_config`）；
///   3. 按新账本建窗（`pet_window::create_all` 会读 state 里的几何与配置）；
///   4. 预备气泡窗/菜单窗（保持"点了就出现"）；
///   5. 重建托盘（"动作点播"的分类来自配置）。
///
/// 全程**不在持 `state.pets` 锁期间做窗口操作**：锁只用来取/换那张表。
pub fn apply_config(app: &AppHandle, next: &AppConfig) -> Result<usize, String> {
    next.validate()?;

    // ---- 1. 拆旧窗 ----
    let old = {
        let state = app.state::<AppState>();
        let mut pets = watchdog::timed_lock(&state.pets, "pets（热重载拆除）");
        std::mem::take(&mut *pets)
    };
    let mut old_labels: Vec<String> = Vec::new();
    for (label, runtime) in old {
        old_labels.push(label.clone());
        if let Err(err) = runtime.window.destroy() {
            eprintln!("[whale-pet] 关闭旧宠物窗失败 {label}：{err}");
        }
    }
    // 气泡窗/菜单窗的标签也是派生的（`bubble-<宠物标签>`），一并拆掉并**一起等**：
    // 少等它们的话，`bubble::prepare` 可能在旧窗还没消失时看到同名标签就提前返回，
    // 结果新宠物没有预备好的气泡窗（虽然 show_bubble 有按需创建的后路，但那会丢掉"点了就出现"）。
    old_labels.extend(close_aux_windows(app));
    wait_windows_gone(app, &old_labels);

    // ---- 2. 换账本 ----
    app.state::<AppState>().set_config(next.clone());

    // ---- 3. 建新窗 ----
    let app_data_dir = app.state::<AppState>().app_data_dir.clone();
    let runtimes = crate::pet_window::create_all(app, next, &app_data_dir)?;
    let labels: Vec<String> = {
        let state = app.state::<AppState>();
        let mut pets = watchdog::timed_lock(&state.pets, "pets（热重载重建）");
        *pets = runtimes;
        pets.keys().cloned().collect()
    };

    // ---- 4. 预备气泡窗/菜单窗 ----
    for label in &labels {
        if let Err(err) = crate::bubble::prepare(app, label) {
            eprintln!("[whale-pet] 热重载后预备气泡窗失败 {label}：{err}");
        }
        if let Err(err) = crate::menu_window::prepare(app, label) {
            eprintln!("[whale-pet] 热重载后预备菜单窗失败 {label}：{err}");
        }
    }

    // ---- 5. 重建托盘（菜单 API 归主线程）----
    let handle = app.clone();
    if let Err(err) = app.run_on_main_thread(move || {
        if let Err(err) = crate::tray::build(&handle) {
            eprintln!("[whale-pet] 热重载后重建托盘失败：{err}");
        }
    }) {
        eprintln!("[whale-pet] 热重载后重建托盘：投递主线程失败：{err}");
    }

    Ok(labels.len())
}

/// 关掉所有气泡窗与菜单窗（它们都是按宠物标签派生的，宠物重建后一律作废），返回被关掉的标签
fn close_aux_windows(app: &AppHandle) -> Vec<String> {
    let victims: Vec<String> = app
        .webview_windows()
        .keys()
        .filter(|label| {
            label.starts_with(crate::bubble::BUBBLE_LABEL_PREFIX)
                || label.starts_with(crate::menu_window::MENU_LABEL_PREFIX)
        })
        .cloned()
        .collect();
    for label in &victims {
        if let Some(window) = app.get_webview_window(label) {
            if let Err(err) = window.destroy() {
                eprintln!("[whale-pet] 关闭辅助窗失败 {label}：{err}");
            }
        }
    }
    victims
}

/// 等旧窗口从窗口表里消失（最多 2 秒）。
///
/// 为什么必须等：窗口标签是**复用**的（`pet-<id>-<index>`），而 `WebviewWindowBuilder::build()`
/// 遇到已存在的标签会直接失败。销毁是异步走主线程的，所以这里等它落地；
/// 等不到也不算致命（`create_all` 会报错并保留日志），所以只警告不返回错误。
fn wait_windows_gone(app: &AppHandle, labels: &[String]) {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let remaining: Vec<&String> = labels
            .iter()
            .filter(|label| app.get_webview_window(label).is_some())
            .collect();
        if remaining.is_empty() {
            return;
        }
        if Instant::now() >= deadline {
            eprintln!(
                "[whale-pet] 警告：等待旧窗口销毁超时，仍有 {} 个窗口在场：{:?}",
                remaining.len(),
                remaining
            );
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// 开机自启的当前状态（直接问系统，不缓存）
pub fn autostart_enabled(app: &AppHandle) -> bool {
    app.autolaunch().is_enabled().unwrap_or(false)
}

/// 设置开机自启；成功后顺手同步托盘勾选，返回**系统的真实状态**
pub fn set_autostart(app: &AppHandle, enabled: bool) -> Result<bool, String> {
    let manager = app.autolaunch();
    let outcome = if enabled { manager.enable() } else { manager.disable() };
    outcome.map_err(|e| {
        format!("{}开机自启失败：{e}", if enabled { "启用" } else { "关闭" })
    })?;
    let now = autostart_enabled(app);
    eprintln!("[whale-pet] 开机自启：{}", if now { "已启用" } else { "已关闭" });
    Ok(now)
}
