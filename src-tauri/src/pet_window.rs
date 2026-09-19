//! 宠物窗口：创建、定位、移动、穿透翻转。
//!
//! 窗口模型（与上游 dsh-pet 桌面模式一致，是本项目最重要的架构决定之一）：
//!   **每只宠物一个"局部小窗"**，窗口尺寸 = 宠物包围盒 + 四周外扩余量。
//!
//! 为什么不用一整块全屏透明窗：
//!   Windows 的 DWM 在"全屏透明 + 视频层"组合下会出现**黑屏**（视频合成路径的已知问题），
//!   上游实测小窗不黑、全屏必黑，因此坚持每宠一个小窗。附带好处是点击穿透的判定范围小、
//!   多显示器下的移动量也小。
//!
//! 窗口内布局：宠物固定摆在 `(margin, margin)`，**窗口移动 = 宠物移动**。
//! 前端只做窗口内布局，屏幕级定位全部在这里（单点收口，避免两处各算一遍而错位）。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use tauri::{AppHandle, Emitter, PhysicalPosition, Runtime, WebviewUrl, WebviewWindowBuilder};

use crate::config::{AppConfig, Corner, PetEntry};
use crate::display;
use crate::model::{PetConfigDto, PetRuntimeDto, PetWindowState, Rect, Size, Vec2};
use crate::watchdog;

/// 事件名：窗口位置/尺寸状态（前端用它作为位置的唯一真相）
pub const EVENT_WINDOW_STATE: &str = "pet://window-state";
/// 事件名：全局光标采样
pub const EVENT_CURSOR: &str = "pet://cursor";
/// 事件名：显示器几何变化
pub const EVENT_DISPLAYS: &str = "pet://displays";
/// 事件名：右键菜单里点了某一项（菜单是独立小窗，动作经宿主转回宠物页执行）
pub const EVENT_MENU_ACTION: &str = "pet://menu-action";

/// 动画画布宽高比（高/宽）：素材是 640×360，桌面端沿用同一比例
pub const ASPECT_RATIO: f64 = 9.0 / 16.0;

/// 身体命中框在 640×360 画布内的定义。
///
/// 直接对齐上游 `dsh-pet/src/shared/constants.ts` 的 `HIT_BOX`，属于**素材几何契约**：
/// 上游调整时必须同步（见 docs/SYNC.md）。
const CANVAS_HIT_BOX: (f64, f64, f64, f64) = (200.0, 50.0, 440.0, 335.0);
/// 画布宽（用于命中框缩放）
const CANVAS_WIDTH: f64 = 640.0;
/// 画布高（用于命中框缩放）
const CANVAS_HEIGHT: f64 = 360.0;

/// 窗口四周外扩余量比例（相对宠物宽度）。
///
/// **M0 取 0：窗口尺寸 = 宠物包围盒尺寸，UI 周围不留任何空白。**
///
/// 为什么不能留余量（实测用户反馈 + 源码依据）：
///   Tauri 的点击穿透实现是给窗口加 `WS_EX_TRANSPARENT | WS_EX_LAYERED`
///   （见 tao `window_state.rs` 的 `IGNORE_CURSOR_EVENT` 分支）。它在"光标离开宠物身体"
///   时确实生效，但**只要窗口处于可交互态，整块窗口矩形都在吃鼠标**。
///   早期实现按 0.5 比例外扩（四周各多出半个宠物大小 → 840×656 里 420×236 才是宠物），
///   用户的实际感受就是"形象周围很大一片空白区域都点不到下层页面"。
///
///   代价：气泡（余额/碎碎念）需要宠物上方有空间，M1 接入气泡时要另想办法
///   （推荐方案：气泡单独开一个不可聚焦的小窗，或仅在显示气泡时临时扩大窗口）。
///   在那之前，"不挡用户操作"比"预留气泡空间"重要得多。
///
/// 必须与前端 `src/renderer/coords.ts` 的 `WINDOW_MARGIN_RATIO` **完全一致**：
/// 一边是窗口尺寸的制造者，一边是坐标换算的使用者，不一致就会整体偏移。
pub const WINDOW_MARGIN_RATIO: f64 = 0.0;

/// 一只宠物窗口的完整运行时（Rust 侧）
pub struct PetRuntime<R: Runtime> {
    /// 窗口句柄
    pub window: tauri::WebviewWindow<R>,
    /// 位置/交互记账
    pub state: PetWindowState,
    /// 下发给前端的配置（素材地址在 `pet_config_dto` 里现算，避免持有多份）
    pub config: PetConfigDto,
    /// 创建时 Tauri 给窗口设置的扩展样式（`GWL_EXSTYLE`）。
    ///
    /// 用途：`set_ignore_cursor_events` 只增删 `WS_EX_TRANSPARENT` 这一个位，
    /// 因此拿"当前样式 ^ 这个基线"就能精确判断**此刻是否处于点击穿透**。
    /// 日志里的 `transparent` 位就是它——它是"窗口是否在吃点击"的唯一真相，
    /// 比前端上报的意图更可信（前端可能因为命令失败而与实际不一致）。
    pub base_ex_style: i64,
}

impl<R: Runtime> PetRuntime<R> {
    /// 窗口标签（同时是宠物 id 派生出来的稳定标识）
    pub fn label(&self) -> &str {
        self.window.label()
    }

    /// 下发配置时使用的 DTO（每次现算，保证与当前配置一致）
    pub fn pet_config_dto(&self) -> PetConfigDto {
        self.config.clone()
    }

    /// 运行时状态 DTO
    pub fn runtime_dto(&self) -> PetRuntimeDto {
        self.state.to_dto()
    }

    /// 把窗口移动到指定的**窗口内容区**坐标（物理像素，屏幕坐标系）
    ///
    /// 计时告警的原因：`set_position` 内部会重写整个扩展样式并 `SetWindowPos(SWP_FRAMECHANGED)`，
    /// 这类调用会等目标窗口所属线程派发消息——拖拽/抛掷时它被以 60 次/秒的频率调用，
    /// 一旦观察到它耗时异常，是"卡顿/卡死"的第一手线索（见 `watchdog` 的复盘）。
    pub fn move_to(&self, origin: Vec2) -> Result<(), String> {
        watchdog::timed("移动宠物窗口", || {
            self.window
                .set_position(PhysicalPosition::new(origin.x.round(), origin.y.round()))
        })
        .map_err(|e| format!("移动窗口失败 {}：{e}", self.label()))
    }

    /// 翻转点击穿透（`ignore = true` 表示整窗穿透，鼠标事件落到下层应用）
    pub fn set_ignore_cursor(&self, ignore: bool) -> Result<(), String> {
        self.window
            .set_ignore_cursor_events(ignore)
            .map_err(|e| format!("设置点击穿透失败 {}：{e}", self.label()))
    }

    /// 宠物包围盒（**屏幕坐标系**）：命中判定与物理的基准
    pub fn box_rect(&self) -> Rect {
        let origin = self.state.box_origin();
        Rect {
            x: origin.x,
            y: origin.y,
            width: self.config.size,
            height: self.config.size * ASPECT_RATIO,
        }
    }

    /// 按前端/兜底通道的意图翻转穿透并记账。
    ///
    /// **只能在主线程（= 窗口所属线程）上调用。** 非主线程（光标轮询线程、
    /// `spawn_hide_until_shown` 之类的工作线程）必须改走
    /// 「[`Self::plan_interactive`] 记账 + `AppHandle::run_on_main_thread` 投递」这条路，
    /// 否则会死锁——跨线程改窗口样式时 Windows 会 `SendMessage` 给窗口所属线程并**等它派发**，
    /// 一旦此刻主线程正在等同一把 `state.pets` 锁，两边就互等到天荒地老。
    /// 完整的复盘见 `watchdog.rs` 的模块注释。
    ///
    /// `interactive = true`  → 窗口可交互（`ignore = false`）
    /// `interactive = false` → 整窗穿透（`ignore = true`）
    pub fn apply_interactive(&mut self, interactive: bool) -> Result<(), String> {
        match self.plan_interactive(interactive) {
            Some(value) => apply_interactive_style(&self.window, value),
            None => Ok(()),
        }
    }

    /// **持锁期**调用：只改"意图账本"，**绝不动窗口**。
    ///
    /// 返回 `Some(值)` 表示确实需要翻转（调用方应把它落到窗口样式上，且必须出锁之后再落）。
    pub fn plan_interactive(&mut self, interactive: bool) -> Option<bool> {
        if self.state.interactive == interactive {
            return None;
        }
        self.state.interactive = interactive;
        Some(interactive)
    }

    /// **出锁期（或主线程）**调用：把意图落到窗口样式上。
    ///
    /// 记账已由 [`Self::plan_interactive`] 完成，所以这里失败只记日志、不回滚账本
    /// （下一步翻转会重新对齐；这也是原来 `apply_interactive` 的行为）。
    pub fn apply_interactive_style(&self, interactive: bool) -> Result<(), String> {
        apply_interactive_style(&self.window, interactive)
    }
}

/// **只凭窗口句柄**翻转点击穿透。
///
/// 为什么单独有这个自由函数：兜底命中的"落样式"是**投递到主线程**执行的
/// （见 `lib.rs::apply_fallback_hit`），那个闭包里只有窗口句柄、拿不到 `PetRuntime`。
/// 它同时也把"记账已由 `plan_interactive` 完成"这件事写死在调用约定里：
/// 本函数不管账本，只管窗口。
pub fn apply_interactive_style<R: Runtime>(
    window: &tauri::WebviewWindow<R>,
    interactive: bool,
) -> Result<(), String> {
    watchdog::timed("翻转点击穿透", || window.set_ignore_cursor_events(!interactive))
        .map_err(|e| format!("设置点击穿透失败 {}：{e}", window.label()))?;
    // 每次翻转都把**实际样式**打出来：这是判断"窗口此刻是否在吃点击"的唯一真相
    // （前端上报的是意图，命令失败或竞态时可能与实际不一致）
    eprintln!(
        "[whale-pet] 窗口 {}：可交互={} → {}",
        window.label(),
        interactive,
        style_summary(window)
    );
    Ok(())
}

/// 创建全部宠物窗口，返回 `标签 -> 运行时` 的表
pub fn create_all<R: Runtime>(
    app: &AppHandle<R>,
    config: &AppConfig,
    app_data_dir: &Path,
) -> Result<HashMap<String, PetRuntime<R>>, String> {
    let primary = display::primary_work_area(app)
        .ok_or_else(|| "无法读取主显示器工作区".to_string())?;
    if !(primary.width > 0.0 && primary.height > 0.0) {
        return Err(format!(
            "主显示器工作区尺寸异常（{}×{}），无法为宠物定位",
            primary.width, primary.height
        ));
    }

    let webm_root = app_data_dir.join(crate::config::WEBM_DIR_NAME);
    let available = scan_webm_files(&webm_root);
    if available.is_empty() {
        // 没有素材时仍然建窗口：让页面里的错误条可见，用户才知道该往哪儿放文件
        eprintln!(
            "[whale-pet] 警告：{} 下没有 .webm 素材，宠物将无法显示动画（请把 VP9-alpha 透明动画放进去）",
            webm_root.display()
        );
    } else {
        // 素材清单非空时校验动画池：把"哪些名字查不到素材"明确列出来。
        // 用**警告**而不是错误：素材是用户自己导入的，没导入完不该让应用起不来。
        let warnings = crate::config::validate_animation_assets(config, &available);
        if warnings.is_empty() {
            eprintln!("[whale-pet] 动画池校验通过：{} 条素材全部命中", available.len());
        } else {
            eprintln!(
                "[whale-pet] 动画池有 {} 个名字找不到素材（这些动作不会生效）：",
                warnings.len()
            );
            for line in warnings.iter().take(20) {
                eprintln!("[whale-pet]   - {line}");
            }
            if warnings.len() > 20 {
                eprintln!("[whale-pet]   …（其余 {} 条省略）", warnings.len() - 20);
            }
        }
    }

    let mut runtimes = HashMap::new();
    for (index, pet) in config.pets.iter().enumerate() {
        let runtime = create_one(app, pet, config, &available, index, &primary)?;
        runtimes.insert(runtime.label().to_string(), runtime);
    }
    Ok(runtimes)
}

/// 创建单只宠物窗口
fn create_one<R: Runtime>(
    app: &AppHandle<R>,
    pet: &PetEntry,
    config: &AppConfig,
    available: &[String],
    index: usize,
    primary: &Rect,
) -> Result<PetRuntime<R>, String> {
    let physics = &config.physics;
    let label = format!("pet-{}-{index}", pet.id);
    let margin = (pet.size * WINDOW_MARGIN_RATIO).round();
    let box_offset = Vec2 { x: margin, y: margin };
    let window_size = Size {
        width: pet.size + margin * 2.0,
        height: (pet.size * ASPECT_RATIO).round() + margin * 2.0,
    };

    let box_origin = anchor_box(pet, primary);
    let origin = anchored_window_origin(box_origin, &window_size, &box_offset, primary);

    // 素材解析：待机 / 点击回应名字**从动画池派生**（见 config::pet_animation_defaults），
    // 池为空的情况已在 config.validate_animations 里拦下，这里只做兜底取值
    let defaults = crate::config::pet_animation_defaults(config);
    let idle = if pet.idle.trim().is_empty() { defaults.idle } else { pet.idle.clone() };
    let click = if pet.click.trim().is_empty() { defaults.click } else { pet.click.clone() };

    // 诊断：`WHALE_PET_AUTOTEST=<N>` 时给页面加上 `?autotest=<N>`，让它跑对应的受控自测
    // （1 = 拖拽 + 甩抛；2 = "拖到命中区外松手"的失控场景。真实鼠标注入在自动化
    //  环境里不可靠，见 PetRuntime 的自测方法说明）
    let autotest = match std::env::var("WHALE_PET_AUTOTEST").as_deref() {
        Ok("1") => "&autotest=1",
        Ok("2") => "&autotest=2",
        Ok("3") => "&autotest=3",
        Ok("4") => "&autotest=4",
        Ok("5") => "&autotest=5",
        Ok("6") => "&autotest=6",
        // 7 = 气泡长驻（60 秒），只为进程外探针争取采样时间，见 main.ts 的说明
        Ok("7") => "&autotest=7",
        // 8 = 把气泡压在宠物身上，供"真鼠标点击"判定点击归属
        Ok("8") => "&autotest=8",
        // 9 = 气泡"显示→隐藏→再显示"，验证窗口复用与穿透位保持
        Ok("9") => "&autotest=9",
        // 10 = 重复甩出 / 空中重甩，断言每次松手都真的起飞（回归网：曾经"只有首次能甩"）
        Ok("10") => "&autotest=10",
        _ => "",
    };

    let window = WebviewWindowBuilder::new(
        app,
        &label,
        WebviewUrl::App(format!("index.html?label={label}{autotest}").into()),
    )
        // ---- 透明置顶桌宠窗口的关键参数 ----
        .inner_size(window_size.width, window_size.height) // 内容区尺寸（物理像素，逻辑/物理 1:1）
        .position(origin.x, origin.y)
        .transparent(true) // 背景透明（依赖 webview 的透明支持）
        .decorations(false) // 无边框无标题栏
        .shadow(false) // 无系统投影（投影会让透明窗出现方形阴影）
        .always_on_top(true) // 置顶
        .skip_taskbar(true) // 不出现在任务栏
        .resizable(false) // 尺寸由配置决定，用户不可拖拽改变
        .maximizable(false)
        .minimizable(false)
        // **不可聚焦**：宠物窗口只需要鼠标事件，绝不能被激活。
        //
        // 为什么这条很重要（实测事故）：`focusable(true)` 时，鼠标在宠物身上按下会**激活本窗口**
        // 并把它变成前台窗口；拖拽结束、窗口恢复"点击穿透"后前台焦点仍留在宠物窗口上，
        // 于是用户去点别的窗口时第一次点击只被用来切换前台——表现为
        // "点击其他页面选中不了"（宠物被拖到屏幕中间、透明窗覆盖大片区域时尤其明显）。
        // `focusable(false)` 让系统在鼠标按下时按 `MA_NOACTIVATE` 处理：窗口照常收到鼠标消息
        // （命中区交互不受影响），但永不抢焦点。
        .focusable(false)
        // 页面加载诊断：透明窗里"页面没加载成"是完全不可见的故障，
        // 必须能把实际 URL 与加载结果写进诊断日志（这是排障的第一现场）
        .on_page_load(move |window, payload| {
            eprintln!(
                "[whale-pet] 页面加载事件 {} url={}",
                match payload.event() {
                    tauri::webview::PageLoadEvent::Started => "started",
                    tauri::webview::PageLoadEvent::Finished => "finished",
                },
                payload.url()
            );
            let _ = window;
        })
        .build()
        .map_err(|e| format!("创建宠物窗口 {label} 失败：{e}"))?;

    let hit_box = scale_hit_box(pet.size);
    // 记录创建时的扩展样式（点击穿透的唯一真相就是它的 WS_EX_TRANSPARENT 位，见结构体注释）
    let base_ex_style = window_ex_style(&window);
    let state = PetWindowState {
        origin,
        size: window_size,
        box_offset,
        // 创建后先设为可交互：等前端完成首次命中判定后自然会翻成穿透。
        // 反过来（先穿透）会出现"启动后前几百毫秒点不到宠物"的空窗期。
        //
        // ⚠️ 这个初值**必须让前端知道**（`PetRuntimeDto::interactive`）：
        // 前端自己的"我已上报过什么"标志如果不知道宿主此刻是可交互的，
        // 它算出的第一次 `setInteractive(false)` 会被自己的去重逻辑吞掉，
        // 于是窗口**一直保持可交互**——宠物包围盒里那些透明区域会一直吃点击，
        // 直到用户第一次悬停宠物才恢复（实测踩过的 bug）。
        interactive: true,
        input_busy: false,
    };

    let runtime = PetRuntime {
        window,
        state,
        config: PetConfigDto {
            label: label.clone(),
            name: pet.name.clone(),
            size: pet.size,
            aspect_ratio: ASPECT_RATIO,
            idle,
            click,
            hit_box,
            position: crate::model::PositionDto {
                corner: pet.position.corner,
                margin_x: pet.position.margin_x,
                margin_y: pet.position.margin_y,
            },
            physics: *physics,
            // 动画池与权重原样下发：选择逻辑全在前端 shared 的 pickers 里（与上游同源）
            animations: config.animations.clone(),
            animation_weights: config.animation_weights,
            // 素材清单下发**基名**（不含扩展名）：前端 animUrl() 拼接即可，
            // 同时可用于判断配置引用的动画是否存在（避免 404）
            available_animations: available.iter().map(|file| strip_extension(file)).collect(),
        },
        base_ex_style,
    };
    // 初始可交互态要落到窗口上（构造时已记账为 true，这里做实际翻转）
    runtime.set_ignore_cursor(false)?;
    // 启动日志：把窗口标志与初始落点写清楚——"宠物挡不挡点击"全靠这几个位
    eprintln!(
        "[whale-pet] 窗口 {label}：原点=({:.0},{:.0}) 尺寸={:.0}×{:.0} 包围盒=({:.0},{:.0}) {}",
        runtime.state.origin.x,
        runtime.state.origin.y,
        runtime.state.size.width,
        runtime.state.size.height,
        runtime.state.box_origin().x,
        runtime.state.box_origin().y,
        style_summary(&runtime.window),
    );
    Ok(runtime)
}

/// 读窗口的扩展样式（`GWL_EXSTYLE = -20`）；失败时返回 0（只影响诊断输出）。
///
/// 直接链接 `user32` 声明该函数，而不是引入 `windows-sys` 依赖：
/// 这里只需要一个只读查询，为本项目保持"依赖最小"（与 display.rs 的 Win32 调用同一做法）。
#[cfg(windows)]
fn window_ex_style<R: Runtime>(window: &tauri::WebviewWindow<R>) -> i64 {
    /// 取窗口扩展样式
    const GWL_EXSTYLE: i32 = -20;

    #[link(name = "user32")]
    extern "system" {
        fn GetWindowLongPtrW(hwnd: isize, index: i32) -> isize;
    }

    match window.hwnd() {
        // SAFETY: 只读查询窗口样式；句柄来自 Tauri 的窗口对象，生命周期覆盖本次调用
        Ok(hwnd) => unsafe { GetWindowLongPtrW(hwnd.0 as isize, GWL_EXSTYLE) as i64 },
        Err(_) => 0,
    }
}

/// 非 Windows：没有样式查询
#[cfg(not(windows))]
fn window_ex_style<R: Runtime>(_window: &tauri::WebviewWindow<R>) -> i64 {
    0
}

/// 窗口样式的可读摘要（诊断用）。
///
/// 关键位的含义：
///   - `transparent`（`WS_EX_TRANSPARENT 0x20`）= 点击穿透中（鼠标事件落到下层应用）；
///   - `noactivate`（`WS_EX_NOACTIVATE 0x08000000`）= 不会被激活/抢前台焦点；
///   - `layered`（`WS_EX_LAYERED 0x80000`）= 透明窗口的实现基础。
///
/// 之所以把这几个位打进日志：宠物是**置顶透明窗**，"点不到别的窗口"这类问题
/// 只有看这几个位才能确证，前端上报的"意图"可能与实际不一致。
pub fn style_summary<R: Runtime>(window: &tauri::WebviewWindow<R>) -> String {
    let current = window_ex_style(window);
    let transparent = current & 0x20 != 0;
    format!(
        "ex=0x{current:X} transparent={} noactivate={} layered={}",
        transparent,
        current & 0x0800_0000 != 0,
        current & 0x0008_0000 != 0,
    )
}

/// 把命中框从 640×360 画布坐标换算到包围盒内坐标
fn scale_hit_box(size: f64) -> Rect {
    let height = size * ASPECT_RATIO;
    let (x0, y0, x1, y1) = CANVAS_HIT_BOX;
    Rect {
        x: (x0 / CANVAS_WIDTH) * size,
        y: (y0 / CANVAS_HEIGHT) * height,
        width: ((x1 - x0) / CANVAS_WIDTH) * size,
        height: ((y1 - y0) / CANVAS_HEIGHT) * height,
    }
}

/// 按角落 + 边距算出宠物包围盒左上角（屏幕坐标系）
fn anchor_box(pet: &PetEntry, area: &Rect) -> Vec2 {
    let height = pet.size * ASPECT_RATIO;
    let left = area.x + f64::from(pet.position.margin_x);
    let top = area.y + f64::from(pet.position.margin_y);
    let right = area.right() - pet.size - f64::from(pet.position.margin_x);
    let bottom = area.bottom() - height - f64::from(pet.position.margin_y);
    match pet.position.corner {
        Corner::TopLeft => Vec2 { x: left, y: top },
        Corner::TopRight => Vec2 { x: right, y: top },
        Corner::BottomLeft => Vec2 { x: left, y: bottom },
        Corner::BottomRight => Vec2 { x: right, y: bottom },
    }
}

/// 角落 + 边距 → 窗口内容区左上角（屏幕坐标系）。
///
/// **不变量**：窗口原点 = 包围盒原点 − 外扩余量，创建后永不改变。
/// 这条不变量是"宠物在屏幕上动、页面只做窗口内布局"这一分工的基础：
/// 前端每帧用 `包围盒 = 窗口原点 + 外扩余量` 反推命中位置，一旦相对关系变了就会整体错位。
///
/// 约束的作用对象是**宠物包围盒**（要让它落在工作区内、不被任务栏挡住），
/// 而不是窗口矩形——窗口允许略微越出工作区上/左边缘（那部分只是透明外扩余量，
/// 不可见也不可交互）。早期版本按窗口矩形夹取，结果窗口被顶回工作区左上角，
/// 宠物被整整推出去一个外扩余量（实测：配置 marginY=100 却出现在 210）。
/// 注意：参数名不能叫 `box`——那是 Rust 关键字（装箱类型 `Box` 的原始标识符）。
fn anchored_window_origin(box_origin: Vec2, window_size: &Size, box_offset: &Vec2, area: &Rect) -> Vec2 {
    // 包围盒原点在"工作区 − 包围盒尺寸"范围内的合法区间
    let box_w = window_size.width - box_offset.x * 2.0;
    let box_h = window_size.height - box_offset.y * 2.0;
    let min_box_x = area.x;
    let max_box_x = (area.right() - box_w).max(area.x);
    let min_box_y = area.y;
    let max_box_y = (area.bottom() - box_h).max(area.y);

    let clamped_box = Vec2 {
        x: box_origin.x.clamp(min_box_x, max_box_x),
        y: box_origin.y.clamp(min_box_y, max_box_y),
    };

    Vec2 {
        x: (clamped_box.x - box_offset.x).round(),
        y: (clamped_box.y - box_offset.y).round(),
    }
}

/// 扫描 `webm/` 目录下可用的动画文件名（按文件名排序，保证"第一个"稳定可预期）
pub fn scan_webm_files(root: &PathBuf) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .flatten()
        .filter(|e| e.path().is_file())
        .filter_map(|e| e.file_name().to_str().map(str::to_string))
        .filter(|name| name.to_ascii_lowercase().ends_with(".webm") || name.to_ascii_lowercase().ends_with(".mov"))
        .collect();
    names.sort();
    names
}

/// 去掉扩展名（`待机呼吸休闲.webm` → `待机呼吸休闲`）。
///
/// 下发给前端的素材清单用基名：前端 `animUrl(名字)` 直接拼 `${base}/webm/${名字}.webm`，
/// 不必在配置里维护"名字 → 文件名"的映射表。
pub fn strip_extension(file_name: &str) -> String {
    match file_name.rfind('.') {
        Some(index) if index > 0 => file_name[..index].to_string(),
        _ => file_name.to_string(),
    }
}

/// 把窗口状态广播给对应的前端（前端据此刷新"位置真相"）
pub fn emit_window_state<R: Runtime>(runtime: &PetRuntime<R>) {
    // 跨 webview 调用同样会等窗口所属线程派发消息，故一并计时（见 watchdog 的复盘）
    let sent = watchdog::timed("广播窗口状态", || {
        runtime.window.emit(EVENT_WINDOW_STATE, runtime.runtime_dto())
    });
    if let Err(err) = sent {
        // 事件发不出去通常意味着 webview 已销毁（窗口正在关闭），只记日志不打断
        eprintln!("[whale-pet] 广播窗口状态失败 {}：{err}", runtime.label());
    }
}

/// 处理前端的 `set_pet_bounds`：移动窗口（并允许按需改尺寸）后回传真实位置。
///
/// 语义说明：前端传的是**窗口内容区**目标坐标与**窗口内容区**尺寸。
///
/// 尺寸：宠物窗**永远严格等于宠物包围盒**（见 `WINDOW_MARGIN_RATIO` 的说明——留空白会挡住
/// 下层点击），菜单与气泡各自是独立小窗（`menu_window.rs` / `bubble.rs`），
/// 所以这里实际上只会收到同一个尺寸值。保留改尺寸的能力是为了将来需要时不必再动契约，
/// 但**当前没有任何调用方会改变它**（早期菜单靠"临时外扩"腾地方，那套已经拆掉了）。
pub fn apply_bounds<R: Runtime>(runtime: &mut PetRuntime<R>, x: f64, y: f64, width: f64, height: f64) -> Result<(), String> {
    // 尺寸合法性：过小会让视频布局崩掉，过大则可能是契约被破坏（这里只做范围校验，不再要求恒等）
    if !(width.is_finite() && height.is_finite() && width >= 32.0 && height >= 32.0 && width <= 8192.0 && height <= 8192.0) {
        return Err(format!("窗口尺寸非法：{width}×{height}"));
    }
    let size_changed = (width - runtime.state.size.width).abs() > 0.5 || (height - runtime.state.size.height).abs() > 0.5;
    if size_changed {
        runtime.state.size = Size { width: width.round(), height: height.round() };
        // 窗口尺寸变化同样走"计时告警"（会等窗口所属线程派发消息）
        watchdog::timed("调整宠物窗口尺寸", || {
            runtime
                .window
                .set_size(tauri::PhysicalSize::new(runtime.state.size.width, runtime.state.size.height))
        })
        .map_err(|e| format!("调整窗口尺寸失败 {}：{e}", runtime.label()))?;
    }
    // 位置按"增量"累加，避免前端持有过期的绝对坐标把窗口拽回去（快速拖拽时确实会发生）
    let delta = Vec2 {
        x: x - runtime.state.origin.x,
        y: y - runtime.state.origin.y,
    };
    runtime.state.origin = Vec2 { x: x.round(), y: y.round() };
    if delta.x.abs() >= 0.5 || delta.y.abs() >= 0.5 {
        runtime.move_to(runtime.state.origin)?;
    }
    // 诊断：**改尺寸的那一次**才打（菜单外扩/缩回；拖拽时尺寸不变，不会刷屏）。
    // 外扩要求窗口向左上长大 → 原点会变成负坐标（屏幕外），必须确认系统没有把窗口夹回来。
    if size_changed {
        let actual = runtime
            .window
            .outer_position()
            .map(|p| format!("({},{})", p.x, p.y))
            .unwrap_or_else(|_| "读取失败".to_string());
        eprintln!(
            "[whale-pet] 窗口几何 {}：请求原点=({},{}) 尺寸={}×{}；读到实际原点={}",
            runtime.label(),
            x.round(),
            y.round(),
            width.round(),
            height.round(),
            actual
        );
    }
    emit_window_state(runtime);
    Ok(())
}

/// 日志：几何采样（节流由调用方负责）
pub fn log_geometry_change(areas: &[Rect]) {
    eprintln!("[whale-pet] 显示器几何变化：{}", display::describe(areas));
}
