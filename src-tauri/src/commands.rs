//! 前端可调用的命令（`invoke` 的 Rust 侧）。
//!
//! 命名约定：命令名用蛇形（`get_pet_config`），前端 `invoke('get_pet_config', {...})`，
//! 参数名与这里函数的形参名一致（Tauri 会按名字匹配）。
//!
//! 错误约定：统一返回 `Result<T, String>`，错误信息是**给人看的**中文句子
//! （会原样出现在前端控制台与错误条里），不要返回 raw 的 Debug 输出。

use tauri::{AppHandle, Emitter, Manager, Runtime, State};

use crate::model::{DisplaysSample, PetConfigDto, PetRuntimeDto};
use crate::pet_window::{self, PetRuntime};
use crate::state::AppState;
use crate::watchdog;

use serde::Serialize;

/// 取出某个宠物的运行时。
///
/// **纪律（曾经违反过，代价是一次死锁）**：持锁期间只做"读账本 / 改账本"，
/// **绝不做窗口操作、跨 webview 调用等会阻塞在别的线程上的事**——
/// 光标轮询线程也在抢这把锁，一旦它持锁去改窗口样式（会 `SendMessage` 给主线程并等派发），
/// 而主线程正等这把锁，两边就永久互等（复盘见 `crate::watchdog` 的模块注释）。
/// 需要动窗口时：先在锁内产出"计划"，出锁后再落（见 `set_pet_interactive` 的写法）。
/// 取锁本身带"等太久就告警"（`watchdog::timed_lock`），真出问题日志里能看见。
fn with_pet<F, T>(state: &AppState, label: &str, action: F) -> Result<T, String>
where
    F: FnOnce(&mut PetRuntime<tauri::Wry>) -> Result<T, String>,
{
    let mut pets = watchdog::timed_lock(&state.pets, "pets（前端命令）");
    let runtime = pets.get_mut(label).ok_or_else(|| format!("找不到窗口标签为 {label} 的宠物"))?;
    action(runtime)
}

/// 取本窗口的宠物配置（前端启动第一步）
#[tauri::command]
pub fn get_pet_config(state: State<'_, AppState>, label: String) -> Result<PetConfigDto, String> {
    with_pet(&state, &label, |runtime| Ok(runtime.pet_config_dto()))
}

/// 取本窗口的运行时状态（前端首帧主动拉取，避免依赖事件到达顺序）
#[tauri::command]
pub fn get_pet_runtime(state: State<'_, AppState>, label: String) -> Result<PetRuntimeDto, String> {
    with_pet(&state, &label, |runtime| Ok(runtime.runtime_dto()))
}

/// 前端请求移动窗口（拖拽 / 抛掷 / 落位）
#[tauri::command]
pub fn set_pet_bounds(
    app: AppHandle,
    state: State<'_, AppState>,
    label: String,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
) -> Result<(), String> {
    with_pet(&state, &label, |runtime| pet_window::apply_bounds(runtime, x, y, width, height))?;
    // 宠物一动，头顶的气泡跟着动（宿主顺手做，省掉前端每帧一条 IPC）
    crate::bubble::follow(&app, &label);
    Ok(())
}

/// 显示宠物头顶的气泡（独立不可聚焦小窗；见 bubble.rs 的设计说明）。
///
/// 前端传入**锚点**（气泡尖角要指的位置）、**宠物包围盒原点**与文本；
/// 宿主负责建窗/摆位/注入文本。包围盒原点必须由前端一起给，理由见 `BubbleRequest`。
///
/// ## 为什么必须是 `async`
///
/// Tauri 的**同步命令跑在主线程（事件循环线程）上**，而 `WebviewWindowBuilder::build()`
/// 会**阻塞等待 WebView2 控制器创建完成**，那个完成回调又必须由事件循环泵出来——
/// 同步命令里建窗 = 自己等自己。实测症状：HWND 建出来了（进程外能看到窗口），
/// 但 `build()` 永不返回、渲染子窗口始终 0×0、页面加载事件永不触发、
/// 前端 `invoke` 的 Promise 永远挂着。声明成 `async` 后命令改在异步运行时的工作线程上执行，
/// `build()` 走 Tauri 支持的"投递到主线程并等待"路径，问题消失。
#[tauri::command]
pub async fn show_bubble<R: Runtime>(
    app: AppHandle<R>,
    label: String,
    anchor_x: f64,
    anchor_y: f64,
    box_x: f64,
    box_y: f64,
    text: String,
) -> Result<(), String> {
    let request = crate::bubble::BubbleRequest { anchor_x, anchor_y, box_x, box_y, text };
    crate::bubble::show(&app, &label, &request)
}

/// 隐藏气泡（窗口保留复用）
#[tauri::command]
pub async fn hide_bubble<R: Runtime>(app: AppHandle<R>, label: String) -> Result<(), String> {
    crate::bubble::hide(&app, &label)
}

/// 弹出右键菜单（菜单是**独立小窗**，见 `menu_window.rs` 的设计说明）。
///
/// `x/y` 是右键点在**屏幕坐标系**里的位置：宿主据此把菜单窗摆到鼠标处
/// （放不下就整体内移，保证不被屏幕裁掉）。
#[tauri::command]
pub async fn show_menu<R: Runtime>(
    app: AppHandle<R>,
    label: String,
    x: f64,
    y: f64,
) -> Result<(), String> {
    crate::menu_window::show(&app, &label, x, y)
}

/// 隐藏右键菜单（菜单页在收掉 DOM 之后调用）
#[tauri::command]
pub async fn hide_menu<R: Runtime>(app: AppHandle<R>, label: String) -> Result<(), String> {
    crate::menu_window::close(&app, &label)
}

/// 菜单项被点击 → 转给**宠物页**执行。
///
/// 为什么不让菜单页直接执行：动画链、包围盒、气泡锚点都在宠物页手里
/// （菜单页只知道"用户点了哪个动作"）。这里只做转发，宠物页收到事件后按同一套逻辑处理，
/// 与"菜单画在宠物窗里"的旧实现行为完全一致。
#[tauri::command]
pub fn menu_action<R: Runtime>(
    app: AppHandle<R>,
    label: String,
    anim: Option<String>,
    action: Option<String>,
) -> Result<(), String> {
    let window = app
        .get_webview_window(&label)
        .ok_or_else(|| format!("找不到宠物窗口 {label}"))?;
    let payload = crate::menu_window::MenuAction { anim, action };
    window
        .emit(crate::pet_window::EVENT_MENU_ACTION, payload)
        .map_err(|e| format!("下发菜单动作失败 {label}：{e}"))
}

/// 前端上报"可交互 / 点击穿透"意图（命中判定的结果）
#[tauri::command]
pub fn set_pet_interactive(state: State<'_, AppState>, label: String, interactive: bool) -> Result<(), String> {
    with_pet(&state, &label, |runtime| runtime.apply_interactive(interactive))
}

/// 前端上报 inputBusy（"我正在使用本窗口的鼠标输入"）
///
/// 兜底命中通道在 busy 期间不做任何穿透翻转，理由见 pet_window / runtime 的注释：
/// 拖拽时宠物滞后于光标，按几何盲判会误判成"用户离开了"从而切断拖拽。
#[tauri::command]
pub fn set_pet_input_busy(state: State<'_, AppState>, label: String, busy: bool) -> Result<(), String> {
    with_pet(&state, &label, |runtime| {
        runtime.state.input_busy = busy;
        Ok(())
    })
}

/// 前端请求显示错误信息（页面红条已由前端渲染，这里只负责落到日志）
#[tauri::command]
pub fn report_pet_error(label: String, message: String) -> Result<(), String> {
    eprintln!("[whale-pet][前端][{label}] {message}");
    Ok(())
}

/// 前端上报诊断日志（落文件，同时打到 stderr）
///
/// 这是**正式功能**而非调试残留：透明无边框窗口出问题时的表现往往只是"看不见/点不动"，
/// 没有界面可看、发布版也没有控制台，因此需要一条从页面出来的日志通道（见 diagnostics.rs）。
#[tauri::command]
pub fn pet_debug_log(state: State<'_, AppState>, label: String, message: String) -> Result<(), String> {
    crate::diagnostics::append(&state.app_data_dir, &label, &message);
    Ok(())
}

/// 取当前显示器几何（含工作区并集与主屏工作区）
#[tauri::command]
pub fn get_displays(state: State<'_, AppState>) -> Result<DisplaysSample, String> {
    state
        .displays_snapshot()
        .ok_or_else(|| "显示器几何尚未就绪".to_string())
}

/// 取素材根地址（前端也可从注入的环境读取；此命令作为可观测的备选入口）
#[tauri::command]
pub fn get_asset_base_url() -> String {
    crate::pet_protocol::asset_base_url()
}

/// 诊断：在指定窗口里注入一次**合成的完整拖拽**（pointerdown → pointermove ×N → pointerup）。
///
/// ## 坐标约定（这里踩过坑，务必按这个来）
///
/// 注入的是**屏幕坐标系里的目标点**，由页面在每一步现算成 `clientX/clientY`
/// （`clientX = 目标屏幕 x − window.screenX`）。
///
/// **为什么不能直接注入固定的 client 坐标**：拖拽时宠物窗会跟着指针移动，
/// 而 `clientX` 是相对窗口的——固定 client 坐标在屏幕上其实是个**随窗口漂移的点**。
/// 页面侧算出的屏幕点 = `窗口原点 + clientX`，于是轨迹里混进了"窗口自己移动的那一段"，
/// 初速估算被整体放大（实测：匀速 340px/s 的合成拖拽被估成 1214px/s）。
/// 用屏幕坐标 + 每步换算之后，合成拖拽与真实鼠标在轨迹上是等价的。
///
/// `step_delay_ms > 0` 时逐步注入并真实等待：跟手是逐帧弹簧积分，把整段事件塞进同一毫秒里
/// 弹簧来不及收敛（宠物几乎不动），那样测不到真实行为。
#[tauri::command]
pub fn debug_synthetic_drag<R: Runtime>(
    app: AppHandle<R>,
    label: String,
    dx: f64,
    dy: f64,
    steps: u32,
    step_delay_ms: u64,
) -> Result<(), String> {
    // 校验窗口存在（注入在页面里跑，错误只打日志）
    app.get_webview_window(&label)
        .ok_or_else(|| format!("找不到窗口 {label}"))?;

    let steps = steps.max(2);
    // 整段拖拽在页面里用定时器跑完：起点取"此刻的命中区中心"，位移由参数给定
    let script = format!(
        r#"(function attempt(n) {{
  var hit = document.getElementById('pet-hit');
  // 页面还没装配好就等一等（宿主无法预知页面就绪时刻；固定延时实测会在慢启动时丢事件）
  if (!hit) {{ if (n < 60) return setTimeout(function () {{ attempt(n + 1); }}, 250); return 'no-hit'; }}
  var r = hit.getBoundingClientRect();
  var sx0 = window.screenX + r.left + r.width / 2;
  var sy0 = window.screenY + r.top + r.height / 2;
  var mk = function (type, sx, sy, buttons) {{
    return new PointerEvent(type, {{ bubbles: true, cancelable: true, pointerId: 7777,
      pointerType: 'mouse', isPrimary: true, button: type === 'pointermove' ? -1 : 0,
      buttons: buttons, clientX: sx - window.screenX, clientY: sy - window.screenY }});
  }};
  var dx = {dx}, dy = {dy}, steps = {steps}, delay = {step_delay_ms};
  hit.dispatchEvent(mk('pointerdown', sx0, sy0, 1));
  var i = 1;
  function step() {{
    if (i > steps) {{ window.dispatchEvent(mk('pointerup', sx0 + dx, sy0 + dy, 0)); return; }}
    var t = i / steps;
    window.dispatchEvent(mk('pointermove', sx0 + dx * t, sy0 + dy * t, 1));
    i++;
    setTimeout(step, delay);
  }}
  setTimeout(step, delay);
  return 'ok';
}})()"#
    );
    app.get_webview_window(&label)
        .ok_or_else(|| format!("找不到窗口 {label}"))?
        .eval(&script)
        .map_err(|e| format!("注入合成拖拽失败：{e}"))?;
    eprintln!("[whale-pet] 合成拖拽已注入：位移 ({dx},{dy})，{steps} 步 × {step_delay_ms}ms");
    Ok(())
}
/// 诊断：按标签找到窗口并注入右键，**把页面脚本的返回值回传**（排障用）。
///
/// 与 `debug_synthetic_right_click` 的区别：这个可从 PowerShell 直接调用、且能拿到
/// 页面侧的执行结果（`ok client=…` / `no-hit`），用于判断"注入是否真的发生了"。
#[tauri::command]
pub fn debug_inject_right_click<R: Runtime>(
    app: AppHandle<R>,
    label: String,
) -> Result<String, String> {
    let window = app.get_webview_window(&label).ok_or_else(|| format!("找不到窗口 {label}"))?;
    let script = r#"(function () {
  var hit = document.getElementById('pet-hit');
  if (!hit) return 'no-hit';
  var r = hit.getBoundingClientRect();
  var x = r.left + r.width / 2, y = r.top + r.height / 2;
  var ev = new MouseEvent('contextmenu', { bubbles: true, cancelable: true, button: 2, buttons: 2, clientX: x, clientY: y });
  hit.dispatchEvent(ev);
  try { window.__TAURI__.core.invoke('pet_debug_log', { label: 'diag', message: '右键注入: ok client=' + x.toFixed(0) + ',' + y.toFixed(0) }); } catch (e) {}
  return 'ok';
})()"#;
    // `WebviewWindow::eval` 不返回页面表达式的值（返回 `()`），因此让页面把结果
    // **写进诊断日志**（`pet_debug_log`），Rust 侧只负责注入。
    window.eval(script).map_err(|e| format!("注入右键失败：{e}"))?;
    Ok("injected".to_string())
}

/// 诊断：注入一次合成的 `contextmenu` 事件（排障用）。
///
/// 用途：区分"菜单链路本身有问题"与"真实右键没有送到 webview"。
/// 合成事件的坐标用命中区中心（一定在宠物身体上）。
#[tauri::command]
pub fn debug_synthetic_right_click<R: Runtime>(app: AppHandle<R>, label: String) -> Result<(), String> {
    let window = app.get_webview_window(&label).ok_or_else(|| format!("找不到窗口 {label}"))?;
    // 脚本**自己重试**：宿主不知道页面何时把监听器绑好（固定延时曾经踩过坑——
    // 注入赶在页面装配之前，事件派发出去但没人听，表现为"探针启用了却什么都没发生"）。
    // 判据只能是"**宠物页**装配好了"（命中区存在）：菜单现在是独立小窗，
    // 它的 DOM 不在这里——早期版本查 `.dsh-pet-menu` 会永远查不到，于是每 250ms
    // 重复右键一次、连着刷 15 秒（实测踩过）。
    let script = r#"(function attempt(n) {
  var hit = document.getElementById('pet-hit');
  if (hit) {
    var r = hit.getBoundingClientRect();
    var x = r.left + r.width / 2, y = r.top + r.height / 2;
    var ev = new MouseEvent('contextmenu', { bubbles: true, cancelable: true, button: 2, buttons: 2, clientX: x, clientY: y });
    hit.dispatchEvent(ev);
    return 'ok client=' + x.toFixed(0) + ',' + y.toFixed(0);
  }
  if (n < 60) return setTimeout(function () { attempt(n + 1); }, 250);
  return 'no-hit';
})(0)"#;
    window.eval(script).map_err(|e| format!("注入右键失败：{e}"))?;
    Ok(())
}

/// 诊断：在**已打开的**级联菜单里点某一项（`branch` 分组 → `item` 叶子）。
///
/// 用途：复现"用户从右键菜单里点播"的完整链路（含菜单开合带来的窗口外扩/缩回）。
/// 这件事用真鼠标很难自动化——桌面上的真实指针随时可能被用户挪走，`mouseleave`
/// 会让菜单自己关掉。合成事件走的是与真人点击**同一套 DOM 监听**
/// （`mouseenter` 展开分支、`click` 触发动作），因此真实覆盖了
/// `close() → 窗口缩回 → onAction` 这个顺序。
#[tauri::command]
pub fn debug_menu_click<R: Runtime>(
    app: AppHandle<R>,
    label: String,
    branch: String,
    item: String,
) -> Result<(), String> {
    let window = app.get_webview_window(&label).ok_or_else(|| format!("找不到窗口 {label}"))?;
    // 菜单标签 `menu-pet-X` → 宠物标签 `pet-X`（页面重新弹菜单时要用）
    let pet_label = label.strip_prefix(crate::menu_window::MENU_LABEL_PREFIX).unwrap_or(&label).to_string();
    // 同样自带重试：等菜单真的挂载出来（宿主无法预知页面装配完成的时刻）。
    // 另外**自带自愈**：菜单会在"鼠标离开它 200ms"之后自动关闭（上游行为），
    // 自动化点击很容易撞上这一点——发现菜单不在了就用本窗自己的屏幕位置重新弹一次。
    let script = format!(
        r#"(function attempt(n) {{
  // 自愈不要每拍都做（会刷屏）：每 5 拍（约 1 秒）补开一次就够
  if (n % 5 === 0 && !document.querySelector('.dsh-pet-menu-item')) {{
    try {{
      window.__TAURI__.core.invoke('show_menu', {{ label: '{pet_label}', x: window.screenX + 20, y: window.screenY + 12 }});
    }} catch (e) {{}}
  }}
  function fire(el, type) {{
    var r = el.getBoundingClientRect();
    el.dispatchEvent(new MouseEvent(type, {{ bubbles: true, cancelable: true, button: 0, buttons: 1, clientX: r.left + r.width / 2, clientY: r.top + r.height / 2 }}));
  }}
  function itemByText(text) {{
    var items = document.querySelectorAll('.dsh-pet-menu-item');
    // 分支项的文字后面跟着箭头（`工具▸`），所以按"以 text 开头"匹配，而不是全等
    for (var i = 0; i < items.length; i++) {{ if (items[i].textContent.trim().indexOf(text) === 0) return items[i]; }}
    return null;
  }}
  var group = itemByText('{branch}');
  if (group) {{
    fire(group, 'mouseenter');
    var leaf = itemByText('{item}');
    if (leaf) {{ fire(leaf, 'click'); return 'ok'; }}
  }}
  if (n < 80) return setTimeout(function () {{ attempt(n + 1); }}, 200);
  // 放弃前把"当前到底有哪些菜单项"记一笔：定位"点了没反应"时最关键的一条线索
  var seen = [];
  var all = document.querySelectorAll('.dsh-pet-menu-item');
  for (var j = 0; j < all.length; j++) seen.push(all[j].textContent.trim());
  try {{ window.__TAURI__.core.invoke('pet_debug_log', {{ label: '{branch}', message: '菜单项候选=' + seen.join(' / ') }}); }} catch (e) {{}}
  return 'no-item';
}})(0)"#
    );
    window.eval(&script).map_err(|e| format!("注入菜单点击失败：{e}"))?;
    Ok(())
}
/// 把窗口置于所有其它窗口之前（托盘/菜单将来会用；M0 作为诊断入口保留）
#[tauri::command]
pub fn raise_pet_window<R: Runtime>(app: AppHandle<R>, label: String) -> Result<(), String> {
    let window = app
        .get_webview_window(&label)
        .ok_or_else(|| format!("找不到窗口 {label}"))?;
    window.set_always_on_top(true).map_err(|e| format!("置顶失败：{e}"))?;
    window.show().map_err(|e| format!("显示窗口失败：{e}"))
}

/// 诊断：在指定窗口里注入一次**合成的 `pointerdown` / `pointerup`**（仅排障用）。
///
/// 为什么需要它：坐标错位这类问题必须把"页面收到的 DOM 坐标"与"宿主下发的光标采样坐标"
/// 放在**同一次事件**里对照，否则只能靠猜。真实鼠标注入在自动化环境里不可靠
/// （桌面可能被别的窗口遮挡），合成事件则完全可控、可重复。
///
/// 合成事件的 `screenX/screenY` 由参数给出（用于与光标采样对照），
/// `clientX/clientY` 用命中区中心（保证事件确实落在命中区上）。
///
/// 注意：这会真实驱动拖拽状态机（按下→抬起），因此不要再对正在使用的实例调用它。
#[tauri::command]
pub fn debug_synthetic_press<R: Runtime>(
    app: AppHandle<R>,
    label: String,
    screen_x: f64,
    screen_y: f64,
) -> Result<(), String> {
    let window = app
        .get_webview_window(&label)
        .ok_or_else(|| format!("找不到窗口 {label}"))?;
    // 用 JSON 传参，避免浮点格式与转义问题
    let script = format!(
        r#"(function () {{
  var hit = document.getElementById('pet-hit');
  if (!hit) return 'no-hit-element';
  var r = hit.getBoundingClientRect();
  var cx = r.left + r.width / 2;
  var cy = r.top + r.height / 2;
  var opts = {{ bubbles: true, cancelable: true, button: 0, buttons: 1,
                clientX: cx, clientY: cy, screenX: {sx}, screenY: {sy} }};
  hit.dispatchEvent(new PointerEvent('pointerdown', opts));
  window.dispatchEvent(new PointerEvent('pointerup', {{ bubbles: true, cancelable: true, button: 0, buttons: 0 }}));
  return 'ok client=' + cx.toFixed(0) + ',' + cy.toFixed(0);
}})()"#,
        sx = screen_x,
        sy = screen_y
    );
    window
        .eval(&script)
        .map_err(|e| format!("注入合成事件失败：{e}"))?;
    Ok(())
}

// ============================================================================
//  设置窗口（M2）
//
//  设计要点：
//    - 一次 `get_settings` 把页面需要的**全部**信息取完（配置 + 路径 + 素材清单 + 自启状态），
//      避免"打开设置窗先发五条 IPC"这种碎片化；
//    - 页面**不逐字段建模**：它拿到的是完整的 `AppConfig` JSON，改哪几个字段就动哪几个，
//      其余（例如 `animations.events`）原样带回——这样 Rust 侧加字段不需要同步改前端类型；
//    - `save_settings` 是 **async**：它要写盘 + 重建宠物窗，同步命令跑在主线程上会把
//      事件循环堵住（建窗就是"自己等自己"，见 `show_bubble` 的说明）。
// ============================================================================

/// 设置窗口首屏要用的全部信息
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsDto {
    /// 当前生效的完整配置（前端原样持有，只改它要改的字段）
    pub config: crate::config::AppConfig,
    /// 配置文件路径（页面上展示 + "打开所在目录"用）
    pub config_path: String,
    /// 应用数据目录
    pub app_data_dir: String,
    /// 素材目录里可用的动画名（不含扩展名的基名，供动画池编辑器做候选）
    pub available_animations: Vec<String>,
    /// 开机自启的系统真实状态
    pub autostart: bool,
}

/// 保存设置的结果
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveSettingsDto {
    /// 配置文件路径
    pub path: String,
    /// 备份路径（原来没有配置文件时为 `None`）
    pub backup: Option<String>,
    /// 素材警告（配置里引用了但目录里找不到的动画名）
    pub warnings: Vec<String>,
    /// 应用后实际存在的宠物数量
    pub pet_count: usize,
}

/// 素材目录里的动画名（基名）
fn available_animations(app_data_dir: &std::path::Path) -> Vec<String> {
    pet_window::scan_webm_files(&crate::config::webm_dir(app_data_dir))
        .iter()
        .map(|file| pet_window::strip_extension(file))
        .collect()
}

/// 取设置窗口所需的全部信息
#[tauri::command]
pub fn get_settings(app: AppHandle, state: State<'_, AppState>) -> Result<SettingsDto, String> {
    Ok(SettingsDto {
        config: state.config_snapshot(),
        config_path: crate::config::config_file_path(&state.app_data_dir).display().to_string(),
        app_data_dir: state.app_data_dir.display().to_string(),
        available_animations: available_animations(&state.app_data_dir),
        autostart: crate::reload::autostart_enabled(&app),
    })
}

/// 保存设置：校验 → 落盘（备份 + 原子写）→ **立即应用**（重建宠物窗）→ 报结果
///
/// 为什么"保存即生效"而不是"提示重启"：M2 的出口标准是"不碰配置文件也能完成全部常用设置"，
/// 而"改完还要重启"会让设置窗口比手改文件更麻烦。重建一次宠物窗约几百毫秒，
/// 用户点保存后立刻能看到新尺寸/新位置/新宠物。
#[tauri::command]
pub async fn save_settings(
    app: AppHandle,
    config: crate::config::AppConfig,
) -> Result<SaveSettingsDto, String> {
    // 1. 先校验：宁可不写盘，也不要把一份过不了校验的配置落下去（下次启动会直接起不来）
    config.validate()?;

    let app_data_dir = app.state::<AppState>().app_data_dir.clone();
    let report = crate::config::save_config(&app_data_dir, &config)?;
    // 2. 我们自己写的文件不算"外部修改"：先把热重载指纹对齐，
    //    否则 1 秒内轮询线程会再拆一次窗（白重建一遍）
    crate::reload::mark_stamp_current(&crate::config::config_file_path(&app_data_dir));

    // 3. 素材缺失只警告不拦截（用户可能还没导入素材集，见 config::validate_animation_assets）
    let warnings = crate::config::validate_animation_assets(&config, &available_animations(&app_data_dir));
    for line in warnings.iter().take(10) {
        eprintln!("[whale-pet] 设置保存警告：{line}");
    }

    // 4. 立即应用
    let pet_count = crate::reload::apply_now(&app, &config, "设置窗口保存")?;
    eprintln!("[whale-pet] 设置已保存：{}（{pet_count} 只宠物）", report.path);

    Ok(SaveSettingsDto {
        path: report.path,
        backup: report.backup,
        warnings,
        pet_count,
    })
}

/// 设置开机自启（返回系统真实状态；顺带同步托盘勾选）
#[tauri::command]
pub fn set_autostart(app: AppHandle, enabled: bool) -> Result<bool, String> {
    crate::reload::set_autostart(&app, enabled)
}

/// 在资源管理器里选中配置文件（比"打开目录"更精确：用户一眼看到要改的是哪个文件）
#[tauri::command]
pub fn open_config_location(state: State<'_, AppState>) -> Result<(), String> {
    let path = crate::config::config_file_path(&state.app_data_dir);
    std::process::Command::new("explorer")
        .arg(format!("/select,{}", path.display()))
        .spawn()
        .map_err(|e| format!("打开配置文件所在目录失败：{e}"))?;
    Ok(())
}

/// 关闭设置窗口（页面上的按钮走这条命令，因此设置页不需要任何窗口权限）
#[tauri::command]
pub fn close_settings(app: AppHandle) -> Result<(), String> {
    crate::settings_window::close(&app)
}

// ============================================================================
//  自绘托盘菜单（外观升级）
//
//  这一组命令只服务 `tray-menu.html`：它取代了系统原生托盘菜单，
//  因此需要"当前状态"（宠物列表 / 显隐 / 每只宠物的动画池）与"执行动作"两个入口。
// ============================================================================

/// 托盘菜单里的一只宠物
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrayPetDto {
    /// 窗口标签（动作下发要按它定位）
    pub label: String,
    /// 显示名
    pub name: String,
    /// 当前是否可见
    pub visible: bool,
    /// **这只宠物实际生效**的动画池（覆盖了就是它自己的）
    pub animations: crate::config::AnimationsConfig,
    /// 这只宠物实际生效的动画链权重
    pub animation_weights: crate::config::AnimationWeights,
    /// 是否自定义了行为（菜单里给个角标，用户一眼看出"这只不一样"）
    pub custom_behaviour: bool,
}

/// 托盘菜单首屏所需的全部状态
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrayMenuStateDto {
    pub pets: Vec<TrayPetDto>,
    /// 是否有任意一只可见（"显示/隐藏"切换项的文案与动作都看它）
    pub any_visible: bool,
    /// 素材目录里可用的动画名（不含扩展名；菜单里可用来判断"配置引用了但没素材"）
    pub available_animations: Vec<String>,
}

/// 取托盘菜单状态
#[tauri::command]
pub fn get_tray_menu_state(state: State<'_, AppState>) -> Result<TrayMenuStateDto, String> {
    let config = state.config_snapshot();
    // 锁内只取标签与可见性，出锁后再组装（与"持锁期不做窗口操作"同一纪律）
    let rows: Vec<(String, String, bool)> = {
        let pets = watchdog::timed_lock(&state.pets, "pets（托盘菜单状态）");
        pets.values()
            .map(|runtime| {
                let visible = runtime.window.is_visible().unwrap_or(false);
                (runtime.label().to_string(), runtime.config.name.clone(), visible)
            })
            .collect()
    };

    let mut pets = Vec::with_capacity(rows.len());
    for (index, (label, name, visible)) in rows.into_iter().enumerate() {
        // 按"窗口标签顺序"取配置里的宠物：create_one 用的标签是 `pet-<id>-<index>`，
        // 所以这里用下标对齐（配置里的顺序 = 建窗顺序）
        let pet = config.pets.get(index);
        let (animations, weights, custom) = match pet {
            Some(pet) => (
                config.effective_animations(pet),
                config.effective_weights(pet),
                crate::config::AppConfig::pet_has_custom_behaviour(pet),
            ),
            None => (config.animations.clone(), config.animation_weights, false),
        };
        pets.push(TrayPetDto { label, name, visible, animations, animation_weights: weights, custom_behaviour: custom });
    }

    Ok(TrayMenuStateDto {
        any_visible: pets.iter().any(|pet| pet.visible),
        pets,
        available_animations: available_animations(&state.app_data_dir),
    })
}

/// 执行一条托盘菜单动作（自绘菜单页调用）
#[tauri::command]
pub fn tray_menu_action(
    app: AppHandle,
    action: String,
    label: Option<String>,
    anim: Option<String>,
) -> Result<(), String> {
    // 动作执行入口只有 `tray::run_menu_action` 一处：命令与排障探针共用，
    // 于是"探针验过"就等于"菜单验过"
    crate::tray::run_menu_action(&app, &action, label.as_deref(), anim.as_deref())?;
    Ok(())
}

/// 托盘菜单换尺寸（主菜单 ↔ 动作点播展开）：页面按内容算好高度报上来
#[tauri::command]
pub fn resize_tray_menu(app: AppHandle, width: f64, height: f64) -> Result<(), String> {
    crate::tray_menu::resize(&app, width, height)
}
