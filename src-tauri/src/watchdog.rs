//! 运行时健康看门狗：把"卡住"变成日志里一眼能看见的告警。
//!
//! ## 为什么需要它（一次真实死锁的复盘）
//!
//! 2026-09-19 出现过一次死锁，现象极具迷惑性：
//!   - 宠物停在半空、窗口整块透明、怎么点都没反应（前端以为"窗口已切到点击穿透"）；
//!   - 前端日志停在最后一条 `交互: 窗口切换为 点击穿透`，之后再无任何一行；
//!   - 进程 CPU 占用 0——不是在死循环，是在**等**。
//!
//! 进程外探针给出了关键证据：**三个 Tauri 顶层窗口全部 `IsHungAppWindow=True`**
//! （它们都归主线程所有），而 WebView2 自己进程里的子窗口（`Chrome_RenderWidgetHostHWND`）
//! 发 `WM_NULL` 是秒回的。结论：主线程卡死，WebView2 侧无辜。
//!
//! 根因是"**持锁期间做跨线程窗口调用**"：
//!   1. 光标轮询线程在 `apply_fallback_hit` 里**先拿了 `state.pets` 锁**，
//!      再调用 `set_ignore_cursor_events` → `SetWindowLongW(GWL_EXSTYLE)`；
//!      而这个顶层窗口归**主线程**所有——跨线程改窗口样式，Windows 会
//!      `SendMessage(WM_STYLECHANGING)` 给主线程并**等它派发**；
//!   2. 同一瞬间主线程正在处理前端命令（`set_pet_bounds` / `pet_debug_log` …），
//!      它也要拿同一把 `state.pets` 锁 → 被轮询线程挡在门外 →
//!      回不到消息循环 → 那条消息永远派发不了；
//!   3. 两个线程互等 → 永久卡死（而且没有任何一条日志能看出这件事）。
//!
//! 修复的纪律写在 `state.rs` 里：**绝不持锁动窗口**；轮询线程要改窗口，
//! 只"记账"然后 `AppHandle::run_on_main_thread` 投递给主线程。
//!
//! ## 这个模块提供什么
//!
//! - [`timed_lock`]：抢锁超过阈值就告警（把"锁被长期持有"变成看得见的事实）；
//! - [`timed`]：窗口操作/跨 webview 调用超过阈值就告警（这类调用是**唯一**会阻塞在别的线程上、
//!   从而把"慢"升级成"死锁"的东西）；
//! - [`MainThreadWatch`]：主线程泵动看门狗。轮询线程每秒投递一次打点，
//!   连续若干秒没被打上就**大声告警**——这正是上面那次死锁当时最缺的一条日志。
//!
//! 阈值都取得很宽松（几百毫秒，正常路径一次都不会触发），不会刷屏。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use tauri::{AppHandle, Runtime};

/// 抢锁告警阈值：正常临界区是微秒级，超过它说明有人在里面做了不该做的事
const LOCK_WARN_MS: u128 = 300;
/// 抢锁"疑似死锁"阈值
const LOCK_STALL_MS: u128 = 3000;
/// 单次窗口操作告警阈值：跨线程/跨进程的窗口调用可能被对方的线程拖住
const OP_WARN_MS: u128 = 200;
/// 主线程打点间隔
const PUMP_INTERVAL_MS: u128 = 1000;
/// 主线程"卡住"判定阈值
const PUMP_STALL_MS: u128 = 3000;

/// 抢状态锁，并把"等了多久"记进日志。
///
/// 语义与 `Mutex::lock` 完全一致（含"锁被污染时取原值继续"），只多一件事：
/// 等待时间超过阈值就打一行告警。**不要**用它替代所有锁——只有那些
/// "可能会和窗口调用相互等"的锁才值得监控（见模块头部的复盘）。
pub fn timed_lock<'a, T>(mutex: &'a Mutex<T>, what: &str) -> MutexGuard<'a, T> {
    let started = Instant::now();
    let guard = match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => {
            eprintln!("[whale-pet][看门] {what} 锁已被污染（此前有线程 panic），按原值继续");
            poisoned.into_inner()
        }
    };
    let waited = started.elapsed().as_millis();
    if waited >= LOCK_STALL_MS {
        eprintln!(
            "[whale-pet][看门] 等锁 {what} 用了 {waited}ms（≥{LOCK_STALL_MS}ms，疑似死锁：\
             持有者可能正卡在跨线程窗口调用上，而本线程又是窗口所属线程）"
        );
    } else if waited >= LOCK_WARN_MS {
        eprintln!("[whale-pet][看门] 等锁 {what} 用了 {waited}ms（≥{LOCK_WARN_MS}ms）");
    }
    guard
}

/// 计时执行一次"可能阻塞在别的线程上"的操作（窗口操作、跨 webview 调用），超阈值告警。
///
/// 为什么只包这类调用：它们会 `SendMessage` 到窗口所属线程并等对方派发——
/// 只有这种"等待别的线程处理消息"的调用，才可能和锁一起构成死锁环。
pub fn timed<T>(what: &str, action: impl FnOnce() -> T) -> T {
    let started = Instant::now();
    let out = action();
    let took = started.elapsed().as_millis();
    if took >= OP_WARN_MS {
        eprintln!("[whale-pet][看门] {what} 耗时 {took}ms（≥{OP_WARN_MS}ms：该调用会等窗口所属线程派发消息）");
    }
    out
}

/// 主线程泵动计数：轮询线程投递打点，主线程取到任务后 +1
static MAIN_PUMPS: AtomicU64 = AtomicU64::new(0);

/// 主线程泵动看门狗。
///
/// 判据是"打点计数有没有涨"：`run_on_main_thread` 投递的任务必须由主线程的
/// 事件循环取出执行；主线程一旦卡住（等锁、等别的线程派发消息），计数就停住。
/// 这条判据不依赖任何业务逻辑，所以能抓住"谁都没想到"的卡法。
///
/// **它必须跑在自己的线程上**（见 [`spawn_main_watchdog`]）：最初我把它挂在光标轮询
/// 线程里，而真实死锁恰恰是轮询线程卡在跨线程窗口调用上——那样看门狗自己也动不了，
/// 一条日志都发不出来。看门狗是最后一道观察哨，不能和"嫌疑人"共用一条命。
pub struct MainThreadWatch {
    /// 上一次看到的打点计数
    last_seen: u64,
    /// 计数停住的起点（恢复后清空）
    stalled_since: Option<Instant>,
    /// 本轮停住是否已经告警过（避免每秒刷屏）
    warned: bool,
}

impl MainThreadWatch {
    /// 新建看门狗
    pub fn new() -> Self {
        Self { last_seen: 0, stalled_since: None, warned: false }
    }

    /// 检查一次：投一次打点，并判断主线程是否还在处理事件。
    ///
    /// 返回 `false` 表示事件循环已经不在了（正常退出），看门狗线程应当收工。
    pub fn tick<R: Runtime>(&mut self, app: &AppHandle<R>) -> bool {
        // 关窗/退出时事件循环已收摊，投递失败属正常，不告警
        if app
            .run_on_main_thread(|| {
                MAIN_PUMPS.fetch_add(1, Ordering::Relaxed);
            })
            .is_err()
        {
            return false;
        }
        let now = MAIN_PUMPS.load(Ordering::Relaxed);
        if now != self.last_seen {
            self.last_seen = now;
            self.stalled_since = None;
            self.warned = false;
            return true;
        }
        let stalled = self.stalled_since.get_or_insert_with(Instant::now).elapsed().as_millis();
        if stalled >= PUMP_STALL_MS && !self.warned {
            self.warned = true;
            eprintln!(
                "[whale-pet][看门] 主线程已 {stalled}ms 没有处理任何事件（疑似死锁/长阻塞）：\
                 此刻窗口移动、点击翻转、前端命令全部停摆。请把这一行连同前后的 [whale-pet] 日志一起反馈"
            );
        }
        true
    }
}

impl Default for MainThreadWatch {
    fn default() -> Self {
        Self::new()
    }
}

/// 启动主线程健康看门狗线程（独立线程，理由见 [`MainThreadWatch`] 的说明）
pub fn spawn_main_watchdog<R: Runtime>(app: AppHandle<R>) {
    std::thread::spawn(move || {
        let mut watch = MainThreadWatch::new();
        loop {
            std::thread::sleep(PUMP_INTERVAL);
            if !watch.tick(&app) {
                return; // 事件循环已退出：正常收工
            }
        }
    });
}

/// 打点间隔（也是看门狗的检查周期）
pub const PUMP_INTERVAL: Duration = Duration::from_millis(PUMP_INTERVAL_MS as u64);
