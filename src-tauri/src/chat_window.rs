//! 对话输入窗（M3）：宠物右上角的一行输入框。
//!
//! ## 为什么是独立小窗，而不是画在宠物窗里
//!
//! 上游把输入框画在宠物窗内（它给宠物窗留了 0.5 的外扩余量）。本项目**没有**这个空间：
//! 宠物窗的尺寸严格等于宠物包围盒（`WINDOW_MARGIN_RATIO = 0`，这是 M1 的刻意决定），
//! 而且在命中区之外整窗是**点击穿透**的——放在那里的输入框根本点不动。
//!
//! ## 它是本项目唯一**可聚焦**的辅助窗
//!
//! 宠物窗 / 气泡窗 / 右键菜单 / 托盘菜单统统 `focusable(false)`（绝不抢焦点，见各处注释）。
//! 输入框必须能收到键盘，所以这里是唯一的例外：`focusable(true)` + 打开时主动 `set_focus()`。
//! 这条差别是有意的，不是漏改；关掉窗口就交还焦点。
//!
//! ## 交互（照抄上游）
//!
//! 回车发送、Esc 关闭、失败**红字留在框内**（不弹气泡、不静默）；**回复不在这里显示**，
//! 而是交给宠物页的气泡——与碎碎念同一条链路（`whisper::emit`）。

use tauri::{AppHandle, Manager, PhysicalPosition, PhysicalSize, WebviewUrl, WebviewWindowBuilder};

use crate::model::Vec2;
use crate::watchdog;

/// 窗口标签前缀（与宠物窗 `pet-*`、气泡 `bubble-*`、菜单 `menu-*`、托盘菜单区分开）
pub const CHAT_LABEL_PREFIX: &str = "chat-";
/// 输入窗尺寸（够放一行中文字 + 发送提示；输入框内部自增高，窗口不变）
///
/// 宽度按用户要求收窄了五分之一（380 → 304）：输入条只占屏幕一小条，
/// 不挡桌面上的东西；窗口变窄后**仍然可以拖动**（左侧有握柄，见 chat.html）。
const WIDTH: f64 = 304.0;
/// **常态高度**：只有一条输入条（`.bar` 40px + 上下各 4px 内边距 + 4px 留给圆角/投影）
const HEIGHT: f64 = 54.0;

/// **带提示行时的高度**：输入条 + 下面一行 12px 的状态文字。
///
/// 为什么必须真的把窗口变高（而不是"文字画在 54px 里"）：
/// 54px 里 `.bar` 就占掉 42px，只剩 4px——状态行**整个落在可视区之外**，
/// `body{overflow:hidden}` 把它裁得干干净净。于是"发送失败"在用户眼里就是
/// **点了没反应**（实测反馈：AI 没开启时点发送毫无动静，其实 `llm_chat` 早就返回了
/// 「AI 功能还没开启（设置 → AI）」，只是那行红字从来没被看见过）。
const HEIGHT_WITH_STATUS: f64 = 76.0;

/// 状态行的最大字符数（与 `chat.html` 的 `.status{max-width:10em}` 对齐）。
///
/// 为什么在前端就截断、而不是靠 CSS 的 `text-overflow`：窗口宽度是固定的，
/// 提示行一长就会被省略号吃掉尾部的「设置 → AI」——那正是用户最需要看到的操作指引。
/// 截断放在**语义完整的那一段**，比让它自然被裁掉更可控。
pub const STATUS_MAX_CHARS: usize = 18;

/// 输入窗标签
pub fn label_for(pet_label: &str) -> String {
    format!("{CHAT_LABEL_PREFIX}{pet_label}")
}

/// 启动时预建并隐藏（与气泡/菜单同一考虑：点了就出现，不用等 WebView2 建窗）
pub fn prepare(app: &AppHandle, pet_label: &str) -> Result<(), String> {
    let label = label_for(pet_label);
    if app.get_webview_window(&label).is_some() {
        return Ok(());
    }
    let window = WebviewWindowBuilder::new(app, &label, WebviewUrl::App(page_url(pet_label).into()))
        // 标题在无边框窗口上看不见，但窗口枚举/截图探针要靠它认人
        .title("whale-pet 对话")
        .inner_size(WIDTH, HEIGHT)
        .position(0.0, 0.0)
        .transparent(true)
        .decorations(false)
        .shadow(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .resizable(false)
        .maximizable(false)
        .minimizable(false)
        // **唯一的可聚焦辅助窗**：要打字（见模块注释）
        .focusable(true)
        .visible(false)
        .on_page_load(|_window, payload| {
            eprintln!("[whale-pet] 对话窗页面加载事件 {:?} url={}", payload.event(), payload.url());
        })
        .build()
        .map_err(|e| format!("创建对话窗失败 {label}：{e}"))?;
    window.hide().map_err(|e| format!("预备对话窗隐藏失败 {label}：{e}"))?;
    eprintln!("[whale-pet] 对话窗已创建（隐藏）{label}：{WIDTH}×{HEIGHT}");
    Ok(())
}

/// 打开某只宠物的输入框：摆到它右上角、显示、聚焦、清空上一次的输入
pub fn open(app: &AppHandle, pet_label: &str) -> Result<(), String> {
    let label = label_for(pet_label);
    // 按需创建这条后路（与 `bubble::show` 同样的考虑）：预建只是"点了就出现"的优化，
    // 任何一条重建路径漏了预备，功能也不该直接失效——实测漏过一次（保存配置后对话窗没回来），
    // 当时用户看到的是"对话窗不存在"这种内部错误。
    if app.get_webview_window(&label).is_none() {
        eprintln!("[whale-pet] 对话窗 {label} 不在（可能刚重建过），按需创建");
        prepare(app, pet_label)?;
    }
    let window = app
        .get_webview_window(&label)
        .ok_or_else(|| format!("对话窗 {label} 创建后仍取不到（见日志里的 WebView2 报错）"))?;

    if let Some(origin) = anchor_origin(app, pet_label, HEIGHT) {
        window
            .set_size(PhysicalSize::new(WIDTH, HEIGHT))
            .map_err(|e| format!("设置对话窗尺寸失败：{e}"))?;
        window
            .set_position(PhysicalPosition::new(origin.x, origin.y))
            .map_err(|e| format!("设置对话窗位置失败：{e}"))?;
        // 摆位这件事必须"能对上账"：把输入量（宠物包围盒 / 命中区 / 工作区）与结果一起打出来，
        // 否则"输入框出现在奇怪的地方"只能靠猜（截图核对时就遇到过一次）。
        eprintln!(
            "[whale-pet] 对话窗摆位 {label}：原点=({:.0},{:.0}) 尺寸={WIDTH}×{HEIGHT}",
            origin.x, origin.y
        );
    }
    window.show().map_err(|e| format!("显示对话窗失败：{e}"))?;
    // 抢焦点是这里**特意**做的：用户点了"说两句"，接下来就要打字
    if let Err(err) = window.set_focus() {
        eprintln!("[whale-pet] 对话窗聚焦失败（仍可点击输入框）：{err}");
    }
    let script = "(function () { var c = window.__whalePetChat; return c ? c.focusInput() : 'not-ready'; })()";
    if let Err(err) = window.eval(script) {
        eprintln!("[whale-pet] 对话窗聚焦注入失败：{err}");
    }
    Ok(())
}

/// 按提示行的有无**改变输入窗高度**，并保持它仍然"挂在宠物命中区右上角"。
///
/// 为什么要在运行期改高度（而不是一开始就做成两行高）：
/// 常态下输入条应该是**一条干净的胶囊**（用户明确要求过"只占屏幕一小条"）；
/// 只有真的要显示"正在思考…"/失败原因/AI 未开启的提示时才需要第二行。
/// 两种高度都从**同一条锚定规则**算出来，因此加高时输入条本身在屏幕上原地不动
/// （高度是从底边往上长的：锚点是命中区顶边，不是窗口顶边）。
///
/// `mode`：`status` = 带提示行（高） / 其余 = 常态（矮）。高度在白名单里选，
/// 不接受调用方传数值——窗口几何是宿主的事，页面只表达"要不要那一行"。
pub fn resize(app: &AppHandle, pet_label: &str, mode: &str) -> Result<(), String> {
    let label = label_for(pet_label);
    let Some(window) = app.get_webview_window(&label) else {
        // 窗口不在（已被热重载拆掉）：不是错误，页面自己也会在下次打开时对齐
        return Ok(());
    };
    let height = match mode {
        "status" => HEIGHT_WITH_STATUS,
        _ => HEIGHT,
    };
    let Some(origin) = anchor_origin(app, pet_label, height) else {
        return Ok(());
    };
    window
        .set_size(PhysicalSize::new(WIDTH, height))
        .map_err(|e| format!("调整对话窗高度失败：{e}"))?;
    window
        .set_position(PhysicalPosition::new(origin.x, origin.y))
        .map_err(|e| format!("重新摆放对话窗失败：{e}"))?;
    eprintln!("[whale-pet] 对话窗高度 {label}：{WIDTH}×{height}（mode={mode}）");
    Ok(())
}

/// 关闭（只隐藏窗口，页面状态由页面自己在下次 `focusInput` 时重置）
pub fn close(app: &AppHandle, pet_label: &str) -> Result<(), String> {
    let label = label_for(pet_label);
    if let Some(window) = app.get_webview_window(&label) {
        window.hide().map_err(|e| format!("隐藏对话窗失败 {label}：{e}"))?;
    }
    // 收闸：输入条不在了，宠物窗口必须马上恢复正常的命中判定。
    //
    // 为什么收闸放在**宿主**而不是等页面自己调 `mark_chat_input(false)`：
    // 关闭有三条路（Esc / 发送成功 / 将来可能有的失焦自动关），页面可能已经不可信
    // （甚至正在销毁）。宿主这里是"窗口真的没了"的唯一确定点。
    let state = app.state::<crate::state::AppState>();
    let changed = {
        let mut pets = watchdog::timed_lock(&state.pets, "pets（对话窗关闭收闸）");
        match pets.get_mut(pet_label) {
            Some(runtime) => runtime.release_chat_input(),
            None => false,
        }
        // 锁在这一行的右花括号处释放；下面的广播**必须**在出锁之后
        //（广播要再拿一次同一把锁，锁内调用就是死锁——与 watchdog.rs 复盘的那条纪律同源）
    };
    if changed {
        crate::pet_window::emit_window_state_by_label(app, pet_label);
    }
    Ok(())
}

/// 页面 URL（把宠物标签与显示名带进去；宠物 id / 名字变了就重建窗口，见 reload）
fn page_url(pet_label: &str) -> String {
    format!("chat.html?label={pet_label}")
}

/// 摆位：贴在宠物**命中区**的右上角，再夹进它所在那一块屏的工作区。
///
/// 用命中区而不是包围盒：命中区才是"看得见的身体"（包围盒里那一圈是透明像素）。
///
/// `height` 由调用方给（常态 / 带提示行两种）：锚定的是**窗口底边**，
/// 所以从矮变高时输入条本身在屏幕上不动、只是下面长出一行提示——
/// 这正是"用户看到的输入条不该因为报个错就跳位置"所要求的。
fn anchor_origin(app: &AppHandle, pet_label: &str, height: f64) -> Option<Vec2> {
    let state = app.state::<crate::state::AppState>();
    let (box_origin, hit_box) = {
        let pets = watchdog::timed_lock(&state.pets, "pets（对话窗摆位）");
        let runtime = pets.get(pet_label)?;
        (runtime.state.box_origin(), runtime.config.hit_box)
    };

    // 命中区右上角再往右 8px；纵向让输入框底边压在命中区顶边上方 8px（像挂在宠物头顶）
    let right = box_origin.x + hit_box.x + hit_box.width;
    let top = box_origin.y + hit_box.y;
    let mut origin = Vec2 { x: (right + 8.0).round(), y: (top - height - 8.0).round() };

    let center = Vec2 { x: right, y: top };
    let area = crate::display::work_area_containing(app, center)
        .or_else(|| crate::display::primary_work_area(app))?;
    let max_x = (area.x + area.width - WIDTH).max(area.x);
    let max_y = (area.y + area.height - height).max(area.y);
    origin.x = origin.x.clamp(area.x, max_x);
    origin.y = origin.y.clamp(area.y, max_y);
    eprintln!(
        "[whale-pet] 对话窗摆位输入：包围盒=({:.0},{:.0}) 命中区=({:.0},{:.0} {:.0}×{:.0}) 期望=({:.0},{:.0}) 夹取后=({:.0},{:.0}) 尺寸={WIDTH}×{height} 工作区=({:.0},{:.0} {:.0}×{:.0})",
        box_origin.x,
        box_origin.y,
        hit_box.x,
        hit_box.y,
        hit_box.width,
        hit_box.height,
        right + 8.0,
        top - height - 8.0,
        origin.x,
        origin.y,
        area.x,
        area.y,
        area.width,
        area.height
    );
    Some(origin)
}
