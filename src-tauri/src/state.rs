//! 应用共享状态：配置、宠物窗口表、显示器几何缓存。
//!
//! 为什么用一把 `Mutex` 而不是更细的锁：
//!   所有操作都在毫秒级以内（改一个坐标、取一份配置），争用只可能发生在
//!   "光标轮询线程"与"前端命令"之间，粒度过细反而增加死锁风险与心智负担。
//!   唯一的原则是：**绝不在持锁期间做 IO 或跨 webview 调用**（避免长持有）。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

use crate::config::AppConfig;
use crate::model::DisplaysSample;
use crate::pet_window::PetRuntime;

/// 应用状态（由 Tauri 托管，命令与轮询线程共享）
pub struct AppState {
    /// 当前生效的配置。
    ///
    /// M0/M1 它是普通字段（改配置=重启）；M2 起放进 `Mutex`，
    /// 因为"设置窗口保存"与"配置文件被外部改动"两条路都要**在运行期换掉它**。
    /// 字段本身不公开：读取一律走 [`AppState::config_snapshot`]，替换走 [`AppState::set_config`]，
    /// 这样"拿了配置就出锁"这条纪律不需要每处调用方自己记得。
    config: Mutex<AppConfig>,
    /// 应用数据目录（配置与素材都在这下面）
    pub app_data_dir: PathBuf,
    /// 宠物窗口表：标签 → 运行时
    pub pets: Mutex<HashMap<String, PetRuntime<tauri::Wry>>>,
    /// 最近一次的显示器几何（供前端首帧主动拉取）
    pub displays: Mutex<Option<DisplaysSample>>,
    /// 最近一次的几何指纹（轮询比对，变了才下发）
    pub displays_fingerprint: Mutex<String>,
}

impl AppState {
    /// 新建状态
    pub fn new(config: AppConfig, app_data_dir: PathBuf) -> Self {
        Self {
            config: Mutex::new(config),
            app_data_dir,
            pets: Mutex::new(HashMap::new()),
            displays: Mutex::new(None),
            displays_fingerprint: Mutex::new(String::new()),
        }
    }

    /// 取当前生效配置的**副本**（拿完就出锁，调用方可以放心去做窗口操作）
    pub fn config_snapshot(&self) -> AppConfig {
        match self.config.lock() {
            Ok(guard) => guard.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }

    /// 换上一份新配置（设置窗口保存 / 配置文件热重载）。
    ///
    /// 注意：本函数**只换账本**，不碰任何窗口——重建窗口是调用方（`reload::apply_config`）的事，
    /// 顺序必须是"先换账本、再按新账本建窗"，反过来新建的窗口会按旧账本初始化。
    pub fn set_config(&self, next: AppConfig) {
        match self.config.lock() {
            Ok(mut guard) => *guard = next,
            Err(poisoned) => *poisoned.into_inner() = next,
        }
    }

    /// 记录显示器几何，返回是否发生了变化
    pub fn update_displays(&self, sample: DisplaysSample, fingerprint: String) -> bool {
        let mut stored_fp = match self.displays_fingerprint.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        let changed = *stored_fp != fingerprint;
        if changed {
            *stored_fp = fingerprint;
            if let Ok(mut stored) = self.displays.lock() {
                *stored = Some(sample);
            }
        }
        changed
    }

    /// 取几何快照（前端首帧拉取用）
    pub fn displays_snapshot(&self) -> Option<DisplaysSample> {
        self.displays.lock().ok().and_then(|guard| guard.clone())
    }
}
