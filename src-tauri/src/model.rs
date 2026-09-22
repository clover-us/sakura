//! Rust ↔ 前端 的数据契约（与 `src/bridge/contract.ts` 一一对应）。
//!
//! 约定：
//!   - 所有结构体统一 `#[serde(rename_all = "camelCase")]`，前端拿到的是小驼峰字段；
//!   - 坐标一律物理像素，但**原点不同**，每个字段的文档里必须写清楚属于哪套坐标系：
//!       * 屏幕坐标系：桌面左上角为原点（多显示器时可为负）；
//!       * 窗口内容区坐标系：窗口内容区左上角为原点；
//!       * 包围盒坐标系：宠物包围盒左上角为原点。
//!   - 契约字段增删必须同步改前端 `src/bridge/contract.ts`（跨语言边界没有编译期保护）。

use serde::{Deserialize, Serialize};

/// 水平/垂直分量（速度或坐标，单位与上下文一致）
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct Vec2 {
    pub x: f64,
    pub y: f64,
}

/// 矩形（左上角 + 宽高；坐标系见使用处说明）
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Rect {
    /// 右缘（不含）
    pub fn right(&self) -> f64 {
        self.x + self.width
    }

    /// 下缘（不含）
    pub fn bottom(&self) -> f64 {
        self.y + self.height
    }

    /// 点是否落在矩形内（右/下缘不含，与工作区拼接语义一致）
    pub fn contains(&self, x: f64, y: f64) -> bool {
        x >= self.x && x < self.right() && y >= self.y && y < self.bottom()
    }
}

/// 拖拽抛掷物理参数（与 `reference/shared/physics.ts` 的 `PhysicsParams` 结构一致）。
///
/// 之所以在 Rust 侧也定义一份：Rust 需要读它并下发给前端（前端才是物理的执行者），
/// 两边靠结构兼容对接，shared 层保持"从上游原样拷贝、零改动"。
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PhysicsParams {
    /// 重力加速度 px/s²；0 = 无重力漂浮
    pub gravity: f64,
    /// 碰壁/落地恢复系数 0~1
    pub restitution: f64,
    /// 地面水平摩擦（每秒衰减系数）
    pub ground_friction: f64,
    /// 顶部是否反弹；false = 可飞出屏幕顶部靠重力落回
    pub ceiling_bounce: bool,
    /// 总力度增益（弹簧 K/C 与甩抛初速整体缩放）
    pub throw_power: f64,
    /// 多宠物互相碰撞开关（M0 单只宠物，先只透传）
    pub pet_collision: bool,
}

impl PhysicsParams {
    /// 取值范围校验：发现非法值就报错，不静默钳制
    pub fn validate(&self) -> Result<(), String> {
        if !(self.gravity.is_finite() && (0.0..=20000.0).contains(&self.gravity)) {
            return Err(format!("physics.gravity 必须在 0~20000 之间，当前为 {}", self.gravity));
        }
        if !(self.restitution.is_finite() && (0.0..=1.0).contains(&self.restitution)) {
            return Err(format!("physics.restitution 必须在 0~1 之间，当前为 {}", self.restitution));
        }
        if !(self.ground_friction.is_finite() && (0.0..=20.0).contains(&self.ground_friction)) {
            return Err(format!(
                "physics.groundFriction 必须在 0~20 之间，当前为 {}",
                self.ground_friction
            ));
        }
        if !(self.throw_power.is_finite() && (0.1..=5.0).contains(&self.throw_power)) {
            return Err(format!("physics.throwPower 必须在 0.1~5 之间，当前为 {}", self.throw_power));
        }
        Ok(())
    }
}

impl Default for PhysicsParams {
    /// 与上游 `DEFAULT_PHYSICS` 完全一致（配置缺失时的兜底值）
    fn default() -> Self {
        Self {
            gravity: 1400.0,
            restitution: 0.78,
            ground_friction: 2.5,
            ceiling_bounce: true,
            throw_power: 1.0,
            pet_collision: false,
        }
    }
}

/// 初始位置（角落 + 边距）：与配置 `pets[].position` 同构
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PositionDto {
    /// 基准角落（kebab-case：top-left 等）
    pub corner: crate::config::Corner,
    pub margin_x: i32,
    pub margin_y: i32,
}

/// 下发给前端的宠物完整配置（`get_pet_config` 的返回值）
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PetConfigDto {
    /// 窗口标签（同时是宠物唯一标识）
    pub label: String,
    /// 配置里的**稳定 id**（如 `main`）
    ///
    /// 与 `label` 的区别：label 带下标（`pet-main-0`），宠物增删/换序就会变；
    /// "跟着这只宠物走"的数据（M3 的 `memory.json`）必须用 id 做键，
    /// 否则用户把宠物顺序调一下，记忆就"换了个人"。
    pub id: String,
    /// 显示名
    pub name: String,
    /// 包围盒宽度（像素）
    pub size: f64,
    /// 高度/宽度比（M0 固定 9/16，对齐 640×360 素材）
    pub aspect_ratio: f64,
    /// 待机动画文件名（相对 `webm/`；点播/随机链不用它，只用它做首帧）
    pub idle: String,
    /// 点击回应动画文件名；空字符串 = 无
    pub click: String,
    /// 命中框（包围盒内坐标，像素）
    pub hit_box: Rect,
    /// 初始位置（角落 + 边距）：菜单的"回到初始位置"按它归位，与启动定位同一套语义
    pub position: PositionDto,
    /// 物理参数
    pub physics: PhysicsParams,
    /**
     * 动画池（待机/转向/拖拽/点击/移动/随机分类/事件）。
     *
     * 直接把配置原样下发：**选择逻辑全部在前端的 shared 纯逻辑里**（pickers.ts），
     * 与上游浏览器端共用同一份实现，Rust 侧只负责"把池给过去、把素材服务好"。
     */
    pub animations: crate::config::AnimationsConfig,
    /// 动画链顶层权重（idle/turn/move；剩余概率归随机动作）
    pub animation_weights: crate::config::AnimationWeights,
    /**
     * 素材清单：`webm/` 目录下**全部可用素材的基名**（不含扩展名）。
     *
     * 为什么是基名列表而不是"名字→URL 映射"：
     *   - 前端 `animUrl(name)` 拼接即可，映射表是多余的中间层；
     *   - 基名列表同时可用于前端判断"配置里引用的动画是否存在"，避免请求 404。
     */
    pub available_animations: Vec<String>,
}

/// 下发给前端的窗口运行时状态（`get_pet_runtime` 的返回值 / `pet://window-state` 的载荷）
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PetRuntimeDto {
    /// 窗口内容区左上角（**屏幕坐标系**，物理像素）
    pub window_origin: Vec2,
    /// 窗口内容区尺寸（物理像素）
    pub window_size: Size,
    /// 宠物包围盒相对窗口内容区左上角的偏移（= 外扩余量，物理像素）
    pub box_offset: Vec2,
    /// 宿主侧记账的"窗口此刻是否可交互"（`false` = 整窗点击穿透）。
    ///
    /// 为什么必须下发：窗口**创建时**是可交互的，而前端的"我已上报过什么"标志
    /// 如果从"穿透"起步，它算出的第一次 `setInteractive(false)` 会被自己的去重吞掉，
    /// 窗口就一直保持可交互（包围盒里的透明区域一直吃点击）。
    /// 前端在启动时用这个字段对齐一次，两边就不会各说各话。
    pub interactive: bool,
    /// 对话输入条是否正在占用鼠标（闸门尚未到期）：见 [`PetWindowState::chat_blocked`]。
    ///
    /// 前端据此**整段跳过命中判定**——只在"窗口吃不吃点击"这一步尊重它是不够的：
    /// 光标采样通道也会起手（`origin='sampler'`），那条路绕过了窗口样式，
    /// 宠物照样会被拎着走（用户实测：拖动输入条经过宠物身上，宠物跟着一起动）。
    pub chat_input_held: bool,
}

/// 尺寸
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Size {
    pub width: f64,
    pub height: f64,
}

/// `pet://cursor` 事件载荷：全局光标采样
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CursorSample {
    /// 光标屏幕坐标（原点 = 桌面左上角）
    pub position: Vec2,
    /// 采样时刻（相对进程启动的毫秒数）
    pub at: u64,
    /// **主键是否处于按下状态**（`GetAsyncKeyState(VK_LBUTTON)`）。
    ///
    /// 为什么必须由宿主提供这个位：桌宠的拖拽状态机不能只依赖页面的 pointer 事件——
    /// 窗口在"点击穿透"期间**收不到任何鼠标事件**，而拖拽中宠物滞后于光标、光标很容易
    /// 滑出身体命中区，前端随即把窗口翻回穿透；此时用户松手的 `pointerup` 永远到不了页面，
    /// 状态机会卡在"按下中"：宠物继续跟着指针、窗口也不再恢复穿透，手感彻底失控。
    /// 有了这个位，前端就能在"物理按键已松开但状态机还认为按着"时强制收尾（见 runtime.ts）。
    pub primary_down: bool,
    /// 主键按下的同时，鼠标是否被**别的进程**的窗口捕获（`display::foreign_mouse_capture`）。
    ///
    /// 用途同 `primary_down`：都是给前端的"起手/收尾"判据。前端只在
    /// "亲眼看到按键由松到按 **且** 这一刻没有外来捕获" 时才允许采样兜底起手——
    /// 少了后者，用户在桌面框选 / 在别的窗口里拖选时，光标扫过宠物就会把它一起带走
    /// （用户实测的 bug）。查询失败时为 `false`（放行），不会吞掉正常点击。
    pub foreign_capture: bool,
}

/// `pet://displays` 事件载荷：显示器几何
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DisplaysSample {
    /// 全部显示器**工作区**（**屏幕坐标系**）：漫游落点、角落定位、落地判定都用它
    pub areas: Vec<Rect>,
    /// 全部显示器**完整面板**（含任务栏区域，与 `areas` 同序）。
    ///
    /// 为什么必须单独下发它：抛掷的"越界侧到底有没有邻屏"探测要用面板而不是工作区。
    /// 上下叠放的两块屏在接缝处会隔一条**不属于任何工作区**的条带（任务栏所在的区域），
    /// 按工作区探测会把那条带当成"空洞"从而误判为墙——宠物甩到接缝处会在任务栏上沿反弹，
    /// 永远穿不过去。面板包含任务栏条带，跨越接缝自然放行；而真正的错位空洞处
    /// 面板并集依然是空的，"空洞是墙"的语义不受影响。
    pub panels: Vec<Rect>,
    /// 主显示器工作区（屏幕坐标系）
    pub primary: Rect,
}

/// 一只宠物窗口的运行时记账（Rust 侧用，不下发）
#[derive(Debug, Clone)]
pub struct PetWindowState {
    /// 窗口内容区左上角（屏幕坐标系）
    pub origin: Vec2,
    /// 窗口内容区尺寸
    pub size: Size,
    /// 宠物包围盒相对窗口的偏移（外扩余量）
    pub box_offset: Vec2,
    /// 前端是否已把本窗口置为"可交互"（幂等去重用）
    pub interactive: bool,
    /// 前端是否正在使用鼠标输入（拖拽中）——此期间兜底通道绝不翻回穿透
    pub input_busy: bool,
    /// **对话输入闸门**：对话输入条正在被拖动时，宠物窗口必须整段让开鼠标。
    ///
    /// 为什么需要它（用户实测的 bug）：输入条与宠物是两个**独立置顶小窗**。
    /// 用户按住握柄把输入条拖到宠物身上时，系统把鼠标消息同时投给下面的宠物窗，
    /// 而宠物的命中判定只看"光标在不在身体上"——于是宠物把这次拖动当成了
    /// "抓住我"，**跟着输入条一起走**。
    ///
    /// ## 为什么收闸判据在**宿主**（这条是用户复测两次后的结论）
    ///
    /// 第一版是"页面收到 mousemove 的 `buttons === 0` 就收闸"，实测**按下 13ms 就被关掉**：
    /// 系统拖动（`startDragging`）那段模态循环里，页面收到的鼠标事件**按键位不可信**。
    /// 宿主这里不收任何页面事件的影响——它每 16ms 自己查一次全局按键位
    /// （`display::primary_button_down()`，与宠物自己那条拖拽看门狗同一个来源，
    /// 注释里写着"这是唯一不依赖窗口消息的可靠来源"），**看到主键真的松开才收闸**。
    ///
    /// 页面这边的职责只剩"**续期**"：拖动期间每 300ms 上报一次 `dragging = true`。
    /// 于是两边各自只需做自己可靠的那半件事，谁也不依赖"网页事件是否可信"。
    pub chat_input_dragging: bool,
    /// 闸门轮次（每开/关一次 +1）：前端"已上报"缓存据此识别闸门换了一轮。
    pub chat_input_epoch: u64,
}

impl PetWindowState {
    /// 组装下发给前端的 DTO
    pub fn to_dto(&self) -> PetRuntimeDto {
        PetRuntimeDto {
            window_origin: self.origin,
            window_size: self.size,
            box_offset: self.box_offset,
            interactive: self.interactive,
            chat_input_held: self.chat_blocked(),
        }
    }

    /// 宠物包围盒左上角（屏幕坐标系）
    pub fn box_origin(&self) -> Vec2 {
        Vec2 {
            x: self.origin.x + self.box_offset.x,
            y: self.origin.y + self.box_offset.y,
        }
    }

    /// 闸门是否生效
    pub fn chat_blocked(&self) -> bool {
        self.chat_input_dragging
    }

    /// 放行/续期闸门
    pub fn hold_chat_input(&mut self) {
        if !self.chat_input_dragging {
            self.chat_input_dragging = true;
            self.chat_input_epoch = self.chat_input_epoch.wrapping_add(1);
        }
    }

    /// 收闸（**主键真的松开** / 窗口关闭 / 拖动结束确认后调用）
    pub fn release_chat_input(&mut self) {
        if self.chat_input_dragging {
            self.chat_input_dragging = false;
            self.chat_input_epoch = self.chat_input_epoch.wrapping_add(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rect_contains_excludes_right_and_bottom_edges() {
        let r = Rect { x: 10.0, y: 20.0, width: 100.0, height: 50.0 };
        assert!(r.contains(10.0, 20.0), "左上角属于矩形");
        assert!(r.contains(109.9, 69.9), "右/下缘内侧属于矩形");
        assert!(!r.contains(110.0, 35.0), "右缘不含");
        assert!(!r.contains(50.0, 70.0), "下缘不含");
        assert!(!r.contains(9.9, 35.0), "左外侧不含");
    }

    #[test]
    fn physics_default_matches_upstream_and_passes_validation() {
        let physics = PhysicsParams::default();
        assert!((physics.gravity - 1400.0).abs() < f64::EPSILON);
        assert!((physics.restitution - 0.78).abs() < f64::EPSILON);
        physics.validate().expect("默认值必须合法");
    }

    #[test]
    fn physics_validation_rejects_out_of_range_values() {
        // 用结构体更新语法构造越界值（clippy::field_reassign_with_default 的要求，
        // 同时这也更清晰地表达"只改一个字段、其余取默认"的意图）
        let bad_restitution = PhysicsParams { restitution: 1.5, ..PhysicsParams::default() };
        assert!(bad_restitution.validate().is_err(), "恢复系数 > 1 必须被拒绝");

        let negative_gravity = PhysicsParams { gravity: -1.0, ..PhysicsParams::default() };
        assert!(negative_gravity.validate().is_err(), "负重力必须被拒绝");

        let zero_power = PhysicsParams { throw_power: 0.0, ..PhysicsParams::default() };
        assert!(zero_power.validate().is_err(), "力度 0 必须被拒绝");
    }
}
