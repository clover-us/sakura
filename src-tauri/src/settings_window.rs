//! 设置窗口：一个**普通窗口**（有边框、可聚焦、不置顶、整窗吃鼠标）。
//!
//! ## 为什么它可以"不守桌宠的规矩"
//!
//! 宠物窗 / 气泡窗 / 菜单窗三条特殊规则（透明、置顶、跳过任务栏、穿透翻转、不可聚焦）
//! 都是为"贴在桌面上的装饰物"服务的。设置窗口是**正经的交互界面**：
//! 用户要能拖它、能最小化、能在任务栏找到它、能正常输入文字——所以它一个特殊位都不设，
//! 也不参与任何穿透自愈逻辑（`lib.rs::apply_fallback_hit` 只遍历 `state.pets`，
//! 设置窗口不在那张表里，天然不会被误翻转）。
//!
//! ## 建窗必须离开主线程（踩过的坑）
//!
//! `WebviewWindowBuilder::build()` 会**阻塞等 WebView2 控制器创建完成**，而那个回调
//! 必须由主线程的事件循环泵出来。托盘菜单事件、同步命令都跑在主线程上，
//! 在那里建窗就是"自己等自己"：窗口句柄建出来了，`build()` 却永不返回。
//! （同一坑的完整记录见 `commands.rs::show_bubble` 的说明。）
//! 所以 [`open`] 在"需要新建"时把建窗动作放到工作线程上——`build()` 内部会投递到主线程
//! 并等待，那是 Tauri 支持的路径。
//!
//! ## 页面
//!
//! `settings.html`（Vite 多页面入口之一）→ `src/settings-page.ts`。
//! 页面只调本应用自己的命令（`get_settings` / `save_settings` / `set_autostart` …），
//! 这些命令经 `invoke_handler` 注册，不走权限系统；窗口仍需要一条 capability
//! 让它能用事件通道（见 `capabilities/default.json` 的说明）。

use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};

/// 设置窗口标签。
///
/// 刻意**不叫** `pet-settings`：`pet-*` 是"宠物窗口"的命名空间
/// （热重载拆窗、托盘显隐、光标广播都按 `state.pets` 这张表走，虽然不会误伤，
/// 但把两种窗口混在一个前缀下迟早让人看错日志）。单独的 `settings` 一眼可辨。
pub const LABEL: &str = "settings";

/// 默认尺寸（逻辑像素）：够放下"宠物卡片 + 物理参数 + 动画池"三段
const WIDTH: f64 = 1000.0;
const HEIGHT: f64 = 760.0;

/// 打开设置窗口：已存在就显示并聚焦，不存在就新建
pub fn open(app: &AppHandle) -> Result<(), String> {
    if let Some(window) = app.get_webview_window(LABEL) {
        window.show().map_err(|e| format!("显示设置窗口失败：{e}"))?;
        let _ = window.unminimize();
        window.set_focus().map_err(|e| format!("聚焦设置窗口失败：{e}"))?;
        eprintln!("[whale-pet] 设置窗口已存在，显示并聚焦");
        return Ok(());
    }

    // 建窗放工作线程：见模块注释（主线程建窗 = 自己等自己）
    let handle = app.clone();
    std::thread::spawn(move || {
        if let Err(err) = create(&handle) {
            eprintln!("[whale-pet] 打开设置窗口失败：{err}");
        }
    });
    Ok(())
}

/// 真正建窗（在工作线程上执行）
fn create(app: &AppHandle) -> Result<(), String> {
    // 自检用的显式主题（`WHALE_PET_DIAG_THEME=dark|light`）：深色那套是跟随系统的，
// 本机桌面为浅色，不强制一次就永远没人看过它（详见 settings-page.ts 的 applyThemeOverride）
    let page = match std::env::var("WHALE_PET_DIAG_THEME").as_deref() {
        Ok(theme @ ("dark" | "light")) => format!("settings.html?theme={theme}"),
        _ => "settings.html".to_string(),
    };
    let window = WebviewWindowBuilder::new(app, LABEL, WebviewUrl::App(page.into()))
        .title("whale-pet 设置")
        .inner_size(WIDTH, HEIGHT)
        // 低于这个尺寸表单会挤成一团（宽度低于 720 时"宠物卡片"要换行）
        .min_inner_size(720.0, 520.0)
        .resizable(true)
        .center()
        .visible(true)
        .on_page_load(|_window, payload| {
            eprintln!(
                "[whale-pet] 设置页面加载事件 {:?} url={}",
                payload.event(),
                payload.url()
            );
        })
        .build()
        .map_err(|e| format!("创建设置窗口失败：{e}"))?;
    eprintln!("[whale-pet] 设置窗口已创建：{WIDTH}×{HEIGHT}");
    let _ = window.set_focus();
    Ok(())
}

/// 关闭设置窗口（页面上的"关闭"按钮走这条命令，因此页面不需要窗口权限）
pub fn close(app: &AppHandle) -> Result<(), String> {
    if let Some(window) = app.get_webview_window(LABEL) {
        window.close().map_err(|e| format!("关闭设置窗口失败：{e}"))?;
    }
    Ok(())
}
