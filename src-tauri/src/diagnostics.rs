//! 诊断日志：把前端的关键节点与错误落到应用数据目录下的 `pet-debug.log`。
//!
//! 为什么需要它（这不是"调试残留"，而是正式功能）：
//!   桌宠是**无边框透明窗口**，出问题时的表现往往只是"看不见宠物"或"点不动"——
//!   既没有界面可以看，控制台也看不到（发布版没有控制台）。没有一条从页面里出来的
//!   日志通道，排障就只能靠猜。因此把"页面到哪一步了"做成显式可读的文件日志。
//!
//! 设计约束：
//!   - 追加写、单行一条、带毫秒时间戳，方便出错后直接翻文件；
//!   - 写失败只打一次警告，不递归报错（日志不能成为新的故障源）；
//!   - 文件固定为 `pet-debug.log`，超过阈值由用户自行删除（M0 不做轮转）。

use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

/// 诊断日志文件名（位于应用数据目录）
pub const LOG_FILE_NAME: &str = "pet-debug.log";

/// 追加一条诊断日志。
///
/// @param app_data_dir 应用数据目录
/// @param label 宠物窗口标签（多开时用于区分是哪只宠物）
/// @param message 日志正文（由调用方保证已包含足够上下文）
pub fn append(app_data_dir: &Path, label: &str, message: &str) {
    let line = format!("[{}][{label}] {message}\n", timestamp());
    let path = app_data_dir.join(LOG_FILE_NAME);
    let result = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .and_then(|mut file| file.write_all(line.as_bytes()));

    if let Err(err) = result {
        // 只警告不打断：日志写入失败不应该影响宠物本身
        eprintln!("[whale-pet] 写入诊断日志失败 {}：{err}", path.display());
        return;
    }
    // 同时打到 stderr：调试构建下在控制台能立刻看到（发布版没有控制台，只有文件）
    eprint!("{line}");
}

/// 排障：`WHALE_PET_DIAG_PRESS=1` 时，在页面就绪后注入一次**合成 pointerdown/pointerup**。
///
/// 目的：把"宿主下发的光标采样坐标"与"页面收到的 DOM 事件坐标"放在同一次事件里对照。
/// 合成事件的 `screenX/screenY` 由本函数给出（这里固定为一个可识别的哨兵值），
/// 因此日志里能直接看出两条坐标链是否一致、以及采样坐标是否等于光标的真实屏幕坐标。
///
/// 只在显式设置环境变量时执行，不参与正常运行路径。
pub fn spawn_synthetic_press_probe(app: tauri::AppHandle, label: String) {
    std::thread::spawn(move || {
        // 给页面留出加载与装配的时间（实测 ~200ms 就绪，这里留足余量）
        std::thread::sleep(std::time::Duration::from_millis(2500));
        if std::env::var("WHALE_PET_DIAG_PRESS").as_deref() == Ok("right") {
            // 右键注入模式：验证菜单链路（与"真实右键是否送达"分开排查）
            if let Err(err) = crate::commands::debug_synthetic_right_click(app, label) {
                eprintln!("[whale-pet] 合成右键探针失败：{err}");
            }
            return;
        }
        if std::env::var("WHALE_PET_DIAG_PRESS").as_deref() == Ok("menu") {
            // 菜单全链路复现（**用户的实际操作顺序**）：
            //   注入右键（宠物窗）→ 菜单窗弹出 → 在**菜单窗**里点「工具 → 显示一句气泡」
            //   → 菜单关闭 → 气泡显示。
            // 菜单现在是独立小窗，所以"点菜单项"这一步必须注入到菜单窗，而不是宠物窗。
            // 中间那 1.2 秒是等菜单页挂载，贴近真人点菜单的节奏。
            if let Err(err) = crate::commands::debug_synthetic_right_click(app.clone(), label.clone()) {
                eprintln!("[whale-pet] 合成右键探针失败：{err}");
                return;
            }
            // 250ms：菜单页挂载只需要几十毫秒。等太久会被"用户在别处按了一下鼠标"打断
            // （外部按下会**正确地**关掉菜单），测试就变成看运气了。
            // 菜单页自带重试（最长 15 秒），所以这里给一个短延时 + 一次补发即可。
            std::thread::sleep(std::time::Duration::from_millis(250));
            let menu_label = crate::menu_window::label_for(&label);
            if let Err(err) = crate::commands::debug_menu_click(
                app.clone(),
                menu_label.clone(),
                "工具".to_string(),
                "显示一句气泡".to_string(),
            ) {
                eprintln!("[whale-pet] 菜单点播探针失败：{err}");
            }
            // 补一发：万一第一发被外部点击打断（菜单被关掉），再开一次菜单点一遍
            std::thread::sleep(std::time::Duration::from_millis(1200));
            let _ = crate::commands::debug_synthetic_right_click(app.clone(), label.clone());
            std::thread::sleep(std::time::Duration::from_millis(250));
            let _ = crate::commands::debug_menu_click(
                app.clone(),
                menu_label,
                "工具".to_string(),
                "显示一句气泡".to_string(),
            );
            // 再合成一次拖拽：宠物移动后气泡必须跟着走（宿主在 set_pet_bounds 里顺手重摆）。
            // 位移由探针给定，外部脚本比对"宠物窗位移"与"气泡窗位移"是否逐像素相等。
            std::thread::sleep(std::time::Duration::from_millis(1000));
            // 匀速拖拽：位移 (-150, +74) 分 12 步 × 40ms = 480ms → 指针平均 ~340px/s。
            // 用于核对初速估算是否忠实（估算值应当和指针速度同一量级）。
            let moved = crate::commands::debug_synthetic_drag(app.clone(), label.clone(), -150.0, 74.0, 12, 40);
            if let Err(err) = moved {
                eprintln!("[whale-pet] 菜单流程里的合成拖拽失败：{err}");
            }
            // 复现用户的报告：**气泡显示又消失之后，再右键还弹不弹得出菜单**。
            // 这里等 7 秒（气泡自动隐藏是 6 秒），右键一次后**故意不点任何菜单项**，
            // 1.5 秒后自检菜单窗是否仍然可见——不可见就说明它被谁立刻藏掉了
            // （历史 bug：页面与宿主互相调用 close，一秒钟藏它几千次）。
            std::thread::sleep(std::time::Duration::from_millis(7000));
            if let Err(err) = crate::commands::debug_synthetic_right_click(app.clone(), label.clone()) {
                eprintln!("[whale-pet] 气泡消失后的右键失败：{err}");
            } else {
                std::thread::sleep(std::time::Duration::from_millis(1500));
                if crate::menu_window::is_visible(&app, &label) {
                    eprintln!("[whale-pet][自检] 气泡消失后再右键：菜单窗仍可见 ✓");
                } else {
                    eprintln!("[whale-pet][自检] 气泡消失后再右键：**菜单窗不见了** ✗（被立刻藏掉了？）");
                }
            }
            // 最后再走一轮菜单：验证"回初始角落"这条动作链路（宠物已被上一步甩走，
            // 点完应该回到配置里的角落）。
            std::thread::sleep(std::time::Duration::from_millis(800));
            if let Err(err) = crate::commands::debug_synthetic_right_click(app.clone(), label.clone()) {
                eprintln!("[whale-pet] 第二轮合成右键失败：{err}");
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(250));
            if let Err(err) = crate::commands::debug_menu_click(
                app,
                crate::menu_window::label_for(&label),
                "工具".to_string(),
                "回到初始位置".to_string(),
            ) {
                eprintln!("[whale-pet] 回初始角落探针失败：{err}");
            }
            return;
        }
        if std::env::var("WHALE_PET_DIAG_PRESS").as_deref() == Ok("drag") {
            // 拖拽验证模式：从窗口内容区 (420,336)（= 命中区中心）拖到 (120,400)，
            // 每步 40ms（贴近真实拖拽的采样节奏，让弹簧逐帧收敛）。
            // 期望结果：拖拽结束后宠物包围盒位移 ≈ 指针位移 = (-300, +64)；
            // 松手时带速度则抛掷（本探针的末段速度不高，多半是温柔放下）。
            // 第一步：匀速偏快的一甩（位移 306px / 480ms ≈ 639px/s）——用来核对
            // "初速估算是否忠实"以及"折扣后的甩出速度是否合理"。
            if let Err(err) = crate::commands::debug_synthetic_drag(app.clone(), label.clone(), -300.0, 64.0, 12, 40) {
                eprintln!("[whale-pet] 合成拖拽探针失败：{err}");
            }
            // 等它落地停稳
            std::thread::sleep(std::time::Duration::from_millis(4000));
            // 第二步：**慢慢**把宠物拖到屏幕上方之外再松手（位移 600px / 1440ms ≈ 417px/s，
            // 低于死区 → 温柔放下）。用来验证"落点会被夹回工作区"：
            // 修之前宠物会停在屏幕外，用户再也抓不到它。
            if let Err(err) = crate::commands::debug_synthetic_drag(app, label, 60.0, -600.0, 12, 120) {
                eprintln!("[whale-pet] 越界落点探针失败：{err}");
            }
        } else {
            // 坐标对照模式：哨兵屏幕坐标（故意取不可能与真实光标重合的值）
            let (sentinel_x, sentinel_y) = (4242.0_f64, 2424.0_f64);
            if let Err(err) = crate::commands::debug_synthetic_press(app, label, sentinel_x, sentinel_y) {
                eprintln!("[whale-pet] 合成按下探针失败：{err}");
            }
        }
    });
}

/// 当前时间戳（`YYYY-MM-DD HH:MM:SS.mmm`，本地时区不可用，统一用 UTC+0 便于对齐日志）
fn timestamp() -> String {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    let total_secs = now.as_secs();
    let millis = now.subsec_millis();
    let secs_of_day = total_secs % 86_400;
    // 说明：只做"时分秒"级别的可读化，日期用自 epoch 起的天数表示——
    // 本日志用于连续排障，精确到毫秒的相对顺序才是重点，绝对日期由用户环境的
    // 文件时间戳提供。这样避免引入 chrono / time 依赖。
    let days = total_secs / 86_400;
    format!(
        "d{days} {:02}:{:02}:{:02}.{millis:03}",
        secs_of_day / 3600,
        (secs_of_day % 3600) / 60,
        secs_of_day % 60
    )
}
