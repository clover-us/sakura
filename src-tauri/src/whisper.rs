//! 碎碎念定时器：到点让宠物说一句（气泡显示，可选播一条 whisper 动画）。
//!
//! ## 为什么定时器在 Rust 侧
//!
//! 前端也可以做（`setInterval` + invoke），但那样"生成中"与"页面是否还活着"耦在一起：
//! 页面重载、宠物窗口重建都会让周期重新计时。放在宿主这边，配置热重载、多宠物轮换、
//! 失败退避都只在一处。
//!
//! ## 三条硬性约束
//!
//! 1. **绝不在轮询线程里发 HTTP**：`tick` 只做"到点了吗"的判断，真正的请求丢给工作线程；
//!    HTTP 超时是 30~60 秒，压在轮询线程上等于穿透自愈（16ms 一轮）直接停摆。
//! 2. **不并发、不叠加**：一个 `WHISPER_BUSY` 标志；上一句还在生成时这一轮直接跳过。
//! 3. **失败不刷屏**：网络错误只写一行日志，并且**推迟下一次尝试**（避免在离线状态下
//!    每 300 秒都白打一次请求——更糟的是用户看到的是"宠物不说话"而不是"网络不通"）。

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::llm::Failure;
use crate::state::AppState;
use crate::watchdog;

/// 上一次成功/尝试的时刻（进程内单例：只跑一条轮询线程）
static LAST_ATTEMPT: Mutex<Option<Instant>> = Mutex::new(None);
/// 是否已有一次生成在进行
static BUSY: AtomicBool = AtomicBool::new(false);
/// 多宠物轮换用的下标
static ROTATION: AtomicUsize = AtomicUsize::new(0);

/// 发给宠物页的碎碎念事件
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WhisperEvent {
    /// 哪只宠物要说（窗口标签）
    pub pet_label: String,
    /// 说的内容
    pub text: String,
    /// 可选配图：**相对素材路径**（如 `memes/xxx.png`），页面拼上 assetBaseUrl 后显示
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
}

/// 轮询线程每秒调一次
pub fn tick(app: &AppHandle) {
    if BUSY.load(Ordering::Relaxed) {
        return;
    }

    let (config, app_data_dir, pets) = {
        let state = app.state::<AppState>();
        let config = state.config_snapshot();
        let pets: Vec<(String, String)> = {
            let runtimes = watchdog::timed_lock(&state.pets, "pets（碎碎念）");
            runtimes
                .values()
                .map(|runtime| (runtime.label().to_string(), runtime.config.name.clone()))
                .collect()
        };
        (config, state.app_data_dir.clone(), pets)
    };

    if !config.llm.whisper_ready() || pets.is_empty() {
        return;
    }

    // 到点了吗
    let interval = Duration::from_secs(config.llm.whisper.interval_sec.max(30));
    {
        let mut last = match LAST_ATTEMPT.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        match *last {
            Some(at) if at.elapsed() < interval => return,
            // 第一次 tick 只记基线：应用刚启动就冒一句话会显得突兀
            None => {
                *last = Some(Instant::now());
                return;
            }
            _ => {}
        }
        *last = Some(Instant::now());
    }

    // 多宠物轮换：每轮只让一只说，避免同时打 N 个请求（也避免 N 只同时开口）
    let index = ROTATION.fetch_add(1, Ordering::Relaxed) % pets.len();
    let (label, name) = pets[index].clone();

    if BUSY.swap(true, Ordering::SeqCst) {
        return;
    }
    // `AppHandle` 是克隆即用的句柄（内部是 Arc）：工作线程必须持有自己的那份
    let app = app.clone();
    std::thread::spawn(move || {
        let started = Instant::now();
        let memes = crate::memes::pool(&config, &app_data_dir);
        match crate::llm::whisper(&config.llm, &app_data_dir, &name, &memes) {
            Ok(generation) => {
                let text = generation.text;
                let image = generation.meme.as_ref().map(|meme| crate::memes::asset_path(&meme.name));
                eprintln!(
                    "[whale-pet][碎碎念] {label}：{text}（{}ms）",
                    started.elapsed().as_millis()
                );
                if let Some(window) = app.get_webview_window(&label) {
                    if let Err(err) = window.emit(crate::pet_window::EVENT_WHISPER, WhisperEvent { pet_label: label.clone(), text, image }) {
                        eprintln!("[whale-pet] 碎碎念下发失败 {label}：{err}");
                    }
                }
            }
            Err(failure) => {
                log_failure(&failure, &label, started.elapsed());
            }
        }
        BUSY.store(false, Ordering::SeqCst);
    });
}

/// 失败只落一行日志（并按需延长下一次尝试的间隔）
fn log_failure(failure: &Failure, label: &str, elapsed: Duration) {
    eprintln!(
        "[whale-pet][碎碎念] 生成失败（{}）：{}（{}ms）",
        failure.reason(),
        failure.message(),
        elapsed.as_millis()
    );
    match failure {
        // 没配 key / 没开开关都是"用户还没准备好"：安静跳过，等用户配好
        Failure::Disabled | Failure::NoKey | Failure::NoModel => {}
        _ => {
            // 网络/限流类失败：把下一次尝试推迟到下一整轮之后（这里把基线往后挪半小时，
            // 免得离线状态下每 5 分钟白打一次）
            if let Ok(mut last) = LAST_ATTEMPT.lock() {
                *last = Some(Instant::now() + Duration::from_secs(1800));
            }
            let _ = label;
        }
    }
}

/// 把一句话交给宠物页显示（气泡 + 可选动画）。
///
/// 碎碎念与**对话回复**共用这条链路（与上游一致）：回复不在输入框里堆历史，
/// 而是让宠物"说出来"——这正是桌宠该有的样子。
pub fn emit(app: &AppHandle, label: &str, text: &str, image: Option<&str>) {
    let Some(window) = app.get_webview_window(label) else {
        eprintln!("[whale-pet] 找不到宠物窗口 {label}，这条话没人显示：{text}");
        return;
    };
    let payload = WhisperEvent {
        pet_label: label.to_string(),
        text: text.to_string(),
        image: image.map(|path| path.to_string()),
    };
    if let Err(err) = window.emit(crate::pet_window::EVENT_WHISPER, payload) {
        eprintln!("[whale-pet] 碎碎念下发失败 {label}：{err}");
    }
}

/// 手动触发一次（设置窗口的"现在说一句"按钮 / 自检）
pub fn say_now(app: &AppHandle, label: &str, name: &str) -> Result<String, Failure> {
    let (config, app_data_dir) = {
        let state = app.state::<AppState>();
        (state.config_snapshot(), state.app_data_dir.clone())
    };
    if !config.llm.whisper_ready() {
        return Err(Failure::Disabled);
    }
    let memes = crate::memes::pool(&config, &app_data_dir);
    let generation = crate::llm::whisper(&config.llm, &app_data_dir, name, &memes)?;
    let image = generation.meme.as_ref().map(|meme| crate::memes::asset_path(&meme.name));
    emit(app, label, &generation.text, image.as_deref());
    Ok(generation.text)
}
