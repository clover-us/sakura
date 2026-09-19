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

// `get_webview_window` 来自 Manager trait（设置窗口探针要用）
use tauri::Manager;

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

/// 排障：`WHALE_PET_DIAG_SETTINGS=1|save|autostart` —— 设置窗口的**进程内自测**入口。
///
/// 为什么需要它：设置窗口平时只能靠"托盘 → 设置…"打开，而自动化环境里没有可靠的
/// 托盘点击手段（本机的真实鼠标注入被桌面环境持续干扰，见 `VERIFICATION.md` 的说明）。
/// 本探针走的是**与托盘完全相同的 `settings_window::open()`**、同一个页面，
/// 最后一步用 `eval` 触发页面上的按钮（而不是直接调 Rust 命令）——
/// 于是"页面 → 命令 → 写盘 → 重建宠物窗"整条链路都被真实走到。
///
///   - `=1`         只打开设置窗口（验证建窗、页面加载、get_settings）
///   - `=save`      再点一次页面上的「保存并立即生效」
///   - `=autostart` 再点一次页面上的「开机自启」勾选框
///   - `=addpet`    先点「＋ 添加一只宠物」再点「保存并立即生效」（验证多开）
///   - `=delpet`    点第二只宠物的「删除这只」再保存（验证多开减员）
///   - `=ownbehaviour` 把第一只宠物切成「单独设置」再保存（验证每宠独立行为）
///   - `=nav:<页>`  切到某一页（`nav:physics` / `nav:animations` / `nav:system` / `nav:about`），
///                  配合 `scripts/capture-window.ps1` 用来逐页检查排版
///
/// 只在显式设置环境变量时执行，不参与正常运行路径。
pub fn spawn_settings_probe(app: tauri::AppHandle, action: String) {
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(1500));
        eprintln!("[whale-pet][设置探针] 打开设置窗口（动作={action}）");
        if let Err(err) = crate::settings_window::open(&app) {
            eprintln!("[whale-pet][设置探针] 打开设置窗口失败：{err}");
            return;
        }
        // 切页动作单独走一条脚本（点的是导航项，不是按钮）
        if let Some(nav) = action.strip_prefix("nav:") {            std::thread::sleep(std::time::Duration::from_millis(6000));
            let Some(window) = app.get_webview_window(crate::settings_window::LABEL) else {
                eprintln!("[whale-pet][设置探针] 找不到设置窗口");
                return;
            };
            let script = format!(
                r#"(function () {{
  var tries = 0;
  function attempt() {{
    var node = document.getElementById('nav-{nav}');
    if (node) {{ node.click(); return; }}
    if (++tries < 40) setTimeout(attempt, 250);
  }}
  attempt();
  return 'scheduled';
}})()"#
            );
            match window.eval(&script) {
                Ok(()) => eprintln!("[whale-pet][设置探针] 已注入切页脚本：nav-{nav}"),
                Err(err) => eprintln!("[whale-pet][设置探针] 注入切页脚本失败：{err}"),
            }
            return;
        }
        let sequence: Vec<&str> = match action.as_str() {
            "save" => vec!["btn-save"],
            "autostart" => vec!["autostart"],
            "addpet" => vec!["btn-add-pet", "btn-save"],
            "delpet" => vec!["btn-del-pet-1", "btn-save"],
            // 每宠独立行为：切成"单独设置"（会自动从全局复制一份）再保存
            "ownbehaviour" => vec!["behaviour-own-0", "btn-save"],
            // AI：填一个测试 key → 保存 → 自检（验证"密钥加密保存 + 自检"整条链路）
            "llmkeysave" => vec!["llm-key-save", "llm-selftest"],
            _ => Vec::new(),
        };
        if sequence.is_empty() {
            return;
        }
        // 等页面取完配置、渲染出按钮（dev 模式下首次要转译模块，给足余量）
        std::thread::sleep(std::time::Duration::from_millis(6000));
        let Some(window) = app.get_webview_window(crate::settings_window::LABEL) else {
            eprintln!("[whale-pet][设置探针] 找不到设置窗口，注入取消");
            return;
        };
        // "填 key 再保存"需要一个额外的输入步骤（其余动作只是点按钮）。
        // 注意顺序：**必须先切到 AI 页**——密钥输入框与按钮只在那页上，页面没渲染出来时点它
        // 会一直重试到放弃（实测踩过：日志说"已注入点击脚本"，其实一个元素都没找到）。
        if action == "llmkeysave" {
            match window.eval(&retry_click_sequence(&["nav-ai"])) {
                Ok(()) => eprintln!("[whale-pet][设置探针] 已切到 AI 页"),
                Err(err) => eprintln!("[whale-pet][设置探针] 切换到 AI 页失败：{err}"),
            }
            std::thread::sleep(std::time::Duration::from_millis(900));
            let fill = r#"(function () {
  var input = document.getElementById('llm-key-input');
  if (!input) return 'no-input';
  input.value = 'sk-mock-probe-0123456789';
  input.dispatchEvent(new Event('input', { bubbles: true }));
  return 'filled';
})()"#;
            match window.eval(fill) {
                Ok(()) => eprintln!("[whale-pet][设置探针] 已填入测试 key（仅用于验证加密保存）"),
                Err(err) => eprintln!("[whale-pet][设置探针] 填入 key 失败：{err}"),
            }
            std::thread::sleep(std::time::Duration::from_millis(300));
        }
        match window.eval(&retry_click_sequence(&sequence)) {
            Ok(()) => eprintln!("[whale-pet][设置探针] 已注入点击脚本：{sequence:?}"),
            Err(err) => eprintln!("[whale-pet][设置探针] 注入点击脚本失败：{err}"),
        }
    });
}

/// 生成"按顺序等到元素可用就点它"的脚本。
///
/// 为什么要等待 + 重试：页面是异步取配置的，按钮在拿到配置前是 `disabled` 的；
/// 固定延时在慢机器上会点了个寂寞（测试误判成"功能没生效"）。
/// 多个目标之间留 400ms：点击会触发整页重渲染，立刻找下一个元素可能拿到还没换上的旧节点。
fn retry_click_sequence(element_ids: &[&str]) -> String {
    let list = element_ids
        .iter()
        .map(|id| format!("'{id}'"))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        r#"(function () {{
  var ids = [{list}];
  var index = 0;
  var tries = 0;
  function step() {{
    if (index >= ids.length) return;
    var node = document.getElementById(ids[index]);
    if (node && !node.hasAttribute('disabled')) {{ node.click(); index++; tries = 0; setTimeout(step, 400); return; }}
    if (++tries < 40) setTimeout(step, 250);
  }}
  step();
  return 'scheduled';
}})()"#
    )
}

/// 排障：`WHALE_PET_DIAG_TRAY=<动作>[,<动作>…]` —— 按顺序走一遍托盘菜单的动作层。
///
/// 为什么需要它：托盘的**真实点击注入不了**（本机桌面环境会持续把光标挪走，
/// M1 的合成输入自测也踩过这一点）。而菜单项被点击后最终都落到
/// [`crate::tray::run_menu_action`] 这一个函数上——探针直接调它，
/// 于是"菜单项 → 动作"这一层被真实执行到；自绘菜单页调的是同一个函数
/// （`tray_menu_action` 命令内部就是它），所以探针验过 ≈ 菜单验过。
///
/// 动作写法（`anim` 用 `动作名` 前缀分隔）：
///
/// ```text
/// WHALE_PET_DIAG_TRAY=toggle,home,anim:待机呼吸休闲,settings
/// ```
pub fn spawn_tray_probe(app: tauri::AppHandle, items: Vec<String>) {
    std::thread::spawn(move || {
        // 等窗口/页面就绪，否则"动作点播"发出去时页面还没挂上事件监听
        std::thread::sleep(std::time::Duration::from_millis(4000));
        for item in items {
            eprintln!("[whale-pet][托盘探针] 触发菜单动作：{item}");
            let (action, anim) = match item.split_once(':') {
                Some((action, anim)) => (action.to_string(), Some(anim.to_string())),
                None => (item.clone(), None),
            };
            if let Err(err) = crate::tray::run_menu_action(&app, &action, None, anim.as_deref()) {
                eprintln!("[whale-pet][托盘探针] 动作失败：{err}");
            }
            std::thread::sleep(std::time::Duration::from_millis(900));
        }
        eprintln!("[whale-pet][托盘探针] 全部动作已走完");
    });
}

/// 排障：`WHALE_PET_DIAG_TRAY_MENU=<毫秒>[:picker|:picker-expanded]` —— 启动后按固定延时**弹出托盘菜单**。
///
/// 用途：托盘菜单的样式只能看（`scripts/capture-window.ps1` 会按标题截图），
/// 而"点托盘图标"在自动化环境里注不进去（见 `spawn_tray_probe` 的说明）。
/// 本探针走的是**与托盘点击完全相同的 `tray_menu::show()`**，
/// 因此截到的就是真实弹出效果（只是锚点取的是当时的鼠标位置）。
///
///   - `:picker`          再点一下「动作点播」（验证展开后的分类折叠态）
///   - `:picker-expanded` 再展开第一个分类（验证点击展开与窗口变高）
pub fn spawn_tray_menu_probe(app: tauri::AppHandle, delay_ms: u64, mode: &'static str) {
    std::thread::spawn(move || {
        // 探针模式下不让"外点关闭"把菜单收掉：本机桌面环境时不时会有零星鼠标按下，
        // 那一下会让截图变成"窗口不存在"（实测撞过两次）
        crate::tray_menu::keep_open_for_probe();
        std::thread::sleep(std::time::Duration::from_millis(delay_ms));
        eprintln!("[whale-pet][托盘菜单探针] 弹出托盘菜单（模式={mode}）");
        if let Err(err) = crate::tray_menu::show(&app) {
            eprintln!("[whale-pet][托盘菜单探针] 弹出失败：{err}");
            return;
        }
        if mode.is_empty() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(1200));
        let Some(window) = app.get_webview_window(crate::tray_menu::LABEL) else {
            eprintln!("[whale-pet][托盘菜单探针] 找不到托盘菜单窗");
            return;
        };
        let picker_script = r#"(function () {
  var node = document.querySelector('[data-act="picker"]');
  if (!node) return 'no-item';
  node.click();
  return 'clicked';
})()"#;
        match window.eval(picker_script) {
            Ok(()) => eprintln!("[whale-pet][托盘菜单探针] 已点击「动作点播」"),
            Err(err) => eprintln!("[whale-pet][托盘菜单探针] 注入点击失败：{err}"),
        }
        if mode != "picker-expanded" {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(900));
        let expand_script = r#"(function () {
  var node = document.querySelector('.group-head');
  if (!node) return 'no-group';
  node.click();
  return 'clicked';
})()"#;
        match window.eval(expand_script) {
            Ok(()) => eprintln!("[whale-pet][托盘菜单探针] 已展开第一个分类"),
            Err(err) => eprintln!("[whale-pet][托盘菜单探针] 注入展开失败：{err}"),
        }
    });
}

/// 排障：`WHALE_PET_DIAG_LLM=status|selftest|whisper|chat|all` —— AI 链路的**进程内自测**入口。
///
/// 为什么需要它：AI 这条链路要验证的是"配置 → 密钥 → 请求组装 → 响应解析 → 记忆/气泡"，
/// 而其中请求组装与错误分支可以完全对着**本地 mock 服务端**（`cargo run --bin mock-llm`）跑，
/// 不需要真实 API key。探针把这些动作按顺序执行并把结果打进日志，
/// 于是"有没有跑通"变成可 grep 的证据（见 `VERIFICATION.md` 第 13 节）。
///
/// 只在显式设置环境变量时执行，不参与正常运行路径。
pub fn spawn_llm_probe(app: tauri::AppHandle, action: String, delay_ms: u64) {
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(delay_ms));
        let state = app.state::<crate::state::AppState>();
        let config = state.config_snapshot();
        let app_data_dir = state.app_data_dir.clone();
        let (label, name, pet_id) = {
            let pets = crate::watchdog::timed_lock(&state.pets, "pets（LLM 探针）");
            pets.values()
                .next()
                .map(|runtime| {
                    (
                        runtime.label().to_string(),
                        runtime.config.name.clone(),
                        runtime.config.label.trim_start_matches("pet-").to_string(),
                    )
                })
                .unwrap_or_else(|| ("pet-main-0".into(), "小鲸鱼".into(), "main".into()))
        };

        eprintln!(
            "[whale-pet][LLM 探针] 动作={action} provider={} baseUrl={} model={} enabled={}",
            config.llm.provider,
            config.llm.effective_base_url().unwrap_or_default(),
            config.llm.effective_model().unwrap_or_default(),
            config.llm.enabled
        );

        let want = |what: &str| action == "all" || action == what;

        if want("status") {
            let store = crate::secret::SecretStore::new(&app_data_dir);
            let key = store.load().ok().flatten();
            eprintln!(
                "[whale-pet][LLM 探针] 状态：hasKey={} keyHint={:?} 密钥文件={}",
                key.is_some(),
                key.as_deref().map(crate::secret::mask),
                store.path().display()
            );
        }

        if want("selftest") {
            match crate::llm::selftest(&config.llm, &app_data_dir, &name) {
                Ok(text) => eprintln!("[whale-pet][LLM 探针] 自检成功：{text}"),
                Err(failure) => {
                    eprintln!("[whale-pet][LLM 探针] 自检失败（{}）：{}", failure.reason(), failure.message())
                }
            }
        }

        if want("whisper") {
            match crate::whisper::say_now(&app, &label, &name) {
                Ok(text) => eprintln!("[whale-pet][LLM 探针] 碎碎念成功：{text}"),
                Err(failure) => {
                    eprintln!("[whale-pet][LLM 探针] 碎碎念失败（{}）：{}", failure.reason(), failure.message())
                }
            }
        }

        if want("chat") {
            match crate::llm::chat(&config.llm, &app_data_dir, &pet_id, &name, "你好呀，今天过得怎么样？") {
                Ok(text) => {
                    eprintln!("[whale-pet][LLM 探针] 对话成功：{text}");
                    let memory = crate::memory::MemoryStore::new(&app_data_dir);
                    let all = memory.all(&pet_id);
                    eprintln!(
                        "[whale-pet][LLM 探针] 记忆文件 {}：{} 条（最近一轮：{:?} → {:?}）",
                        memory.path().display(),
                        all.len(),
                        all.iter().rev().nth(1).map(|m| m.content.clone()),
                        all.last().map(|m| m.content.clone())
                    );
                }
                Err(failure) => {
                    eprintln!("[whale-pet][LLM 探针] 对话失败（{}）：{}", failure.reason(), failure.message())
                }
            }
        }

        eprintln!("[whale-pet][LLM 探针] 动作结束：{action}");
    });
}

/// 排障：`WHALE_PET_DIAG_CHAT=<毫秒>[:消息]` —— 打开对话输入窗、填一句话并发送。
///
/// 验证的是**用户真实要走的那条路**：宿主开窗摆位（贴命中区右上角）→ 页面填入并点发送 →
/// `llm_chat` → 记忆落盘 + 回复交给宠物页气泡。证据分布在三处（都可以 grep）：
///   - 对话页日志：`对话: 已回复（…ms）：…`
///   - 宠物页日志：`碎碎念: …`（回复走的就是碎碎念那条气泡链路）
///   - `memory.json`：多了一轮 user/assistant
pub fn spawn_chat_probe(app: tauri::AppHandle, delay_ms: u64, message: String) {
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(delay_ms));
        let label = {
            let state = app.state::<crate::state::AppState>();
            let pets = crate::watchdog::timed_lock(&state.pets, "pets（对话探针）");
            pets.keys().next().cloned()
        };
        let Some(label) = label else {
            eprintln!("[whale-pet][对话探针] 没有宠物，取消");
            return;
        };
        eprintln!("[whale-pet][对话探针] 打开 {label} 的对话窗");
        if let Err(err) = crate::chat_window::open(&app, &label) {
            eprintln!("[whale-pet][对话探针] 打开失败：{err}");
            return;
        }
        // 空消息 = **只开窗不发送**（截图用：窗口发送成功就会自己收起来）
        if message.trim().is_empty() {
            eprintln!("[whale-pet][对话探针] 只开窗（未发送），供截图");
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(1500));
        let Some(window) = app.get_webview_window(&crate::chat_window::label_for(&label)) else {
            eprintln!("[whale-pet][对话探针] 找不到对话窗");
            return;
        };
        let text = serde_json::to_string(&message).unwrap_or_else(|_| "\"你好\"".to_string());
        let script = format!(
            r#"(function () {{
  var input = document.getElementById('input');
  if (!input) return 'no-input';
  input.value = {text};
  input.dispatchEvent(new Event('input', {{ bubbles: true }}));
  var send = document.getElementById('send');
  if (!send) return 'no-send';
  send.click();
  return 'sent';
}})()"#
        );
        match window.eval(&script) {
            Ok(()) => eprintln!("[whale-pet][对话探针] 已填入并点击发送：{message}"),
            Err(err) => eprintln!("[whale-pet][对话探针] 注入失败：{err}"),
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
