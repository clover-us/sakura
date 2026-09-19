/**
 * Rust ↔ 前端 的**数据契约**（单一事实来源）。
 *
 * 本文件与 src-tauri/src/model.rs 一一对应，任何一边改字段都必须同步另一边。
 * 之所以把契约单独放一个文件：它是跨语言边界，字段名写错不会在编译期报错，
 * 只能靠集中定义 + 注释约束来降低排查成本。
 *
 * 命名约定：
 *   - Rust 侧结构体统一 `#[serde(rename_all = "camelCase")]`，因此这里全是小驼峰；
 *   - 坐标一律是「物理像素」，但**原点不同**，每个字段的注释里写清楚：
 *       * 屏幕坐标系：桌面左上角为原点（多显示器时可为负）
 *       * 窗口/本地坐标系：窗口内容区左上角为原点
 */

/** 水平/垂直方向的速度或位移（物理像素） */
export interface Vec2 {
  x: number;
  y: number;
}

// ---------------------------------------------------------------------------
// 动画池（与 src-tauri/src/config.rs 的 AnimationsConfig 一一对应）
// ---------------------------------------------------------------------------

/** 移动动作：动作名 + 可选覆盖参数（未写字段取 moves.default） */
export interface MoveSpec {
  /** 动画名（对应素材目录里的文件基名） */
  name: string;
  /** 覆盖参数：minDist/maxDist/margin/leadSec/tailSec 的任意子集 */
  params?: Record<string, number>;
}

/** 移动池：默认参数 + 动作列表 */
export interface MovesConfig {
  /** 每个动作都先取这套参数（minDist/maxDist/margin/leadSec/tailSec） */
  default: Record<string, number>;
  /** 可选的移动动作 */
  actions: MoveSpec[];
}

/** 随机动作分类（`noMirror: true` = 带文字、镜像会颠倒，宠物朝右时跳过） */
export interface CategoryConfig {
  id: string;
  weight: number;
  actions: string[];
  noMirror?: boolean;
}

/** 事件档位：单个动画名（固定播放）或候选数组（触发时档内随机抽 1） */
export type EventSlot = string | string[];

/** 动画池全集（与 config.jsonc 的 animations 段同构） */
export interface AnimationsConfig {
  /** 待机池（等概率抽） */
  idle: string[];
  /** 转向池（播完翻转朝向） */
  turn: string[];
  /** 拖拽池（被无形抓起悬空的姿势） */
  drag: string[];
  /** 点击回应池 */
  clicks: string[];
  /** 移动池 */
  moves: MovesConfig;
  /** 随机动作分类 */
  categories: CategoryConfig[];
  /** 事件动画：事件名 → 档位数组（不进随机链，只由代码显式触发） */
  events: Record<string, EventSlot[]>;
}

/** 动画链顶层权重（idle/turn/move；剩余概率归随机动作分类） */
export interface AnimationWeights {
  idle: number;
  turn: number;
  move: number;
}

/** 基准角落（与配置的 position.corner 一致） */
export type Corner = 'top-left' | 'top-right' | 'bottom-left' | 'bottom-right';

/** 矩形（左上角 + 宽高，物理像素，坐标系由使用处的字段注释决定） */
export interface Rect {
  x: number;
  y: number;
  width: number;
  height: number;
}

/**
 * 拖拽抛掷物理参数。
 *
 * 与 `reference/shared/physics.ts` 的 `PhysicsParams` 结构完全一致，
 * 但在本文件独立声明：shared 层是从上游原样拷贝的纯逻辑（零改动原则），
 * 不让它依赖 Tauri 契约，两边靠结构兼容对接。
 */
export interface PhysicsParams {
  /** 重力加速度 px/s²；0 = 无重力漂浮 */
  gravity: number;
  /** 碰壁/落地恢复系数 0~1 */
  restitution: number;
  /** 地面水平摩擦（每秒衰减系数） */
  groundFriction: number;
  /** 顶部是否反弹；false = 可飞出屏幕顶部靠重力落回 */
  ceilingBounce: boolean;
  /** 总力度增益（弹簧 K/C 与甩抛初速整体缩放） */
  throwPower: number;
  /** 多宠物互相碰撞开关（M0 单只宠物，先只透传） */
  petCollision: boolean;
}

/** 一只宠物的静态配置（Rust 侧读配置文件后下发；窗口创建参数也由它决定） */
export interface PetConfig {
  /** 窗口标签，也是本宠物的唯一标识（如 `pet-0`） */
  label: string;
  /** 配置里的稳定 id（如 `main`）：记忆这类"跟着宠物走"的数据用它做键（label 会随增删换序变化） */
  id: string;
  /** 显示名（调试/未来气泡用） */
  name: string;
  /**
   * 资源根地址（Rust 侧按平台注入，形如 `pet://localhost`）。
   *
   * 为什么不写死在 TS 里：自定义协议在各平台的 URL 形式不同
   * （Windows 是 `http://<scheme>.localhost/...`），由宿主注入才不用在前端做平台判断。
   */
  assetBaseUrl: string;
  /** 包围盒宽度（像素）：实际尺寸 = size × size×(9/16) */
  size: number;
  /** 播放画布宽高比倒数：视频按 9/16 布局（对齐上游 CANVAS 640×360） */
  aspectRatio: number;
  /** 待机动画文件名（相对 `${assetBaseUrl}/webm/`，含扩展名） */
  idle: string;
  /** 点击回应动画文件名；空字符串 = 无（点击只做 Q 弹） */
  click: string;
  /** 命中框（以包围盒左上角为原点、包围盒内部的像素矩形，已按 size 缩放） */
  hitBox: Rect;
  /**
   * 初始位置（角落 + 边距）。
   *
   * 菜单的"回到初始位置"需要它；与启动定位共用同一套语义
   * （`@shared/motion.ts` 的 `anchorPixel`），保证"回到的位置"就是"开机时的位置"。
   */
  position: { corner: Corner; marginX: number; marginY: number };
  /** 抛掷物理参数 */
  physics: PhysicsParams;
  /**
   * 动画池（待机/转向/拖拽/点击/移动/随机分类/事件）。
   *
   * 选择逻辑**全部在前端的 shared 纯逻辑里**（`@shared/pickers.ts`），与上游浏览器端
   * 共用同一份实现；Rust 只负责"把池给过来、把素材服务好"。
   */
  animations: AnimationsConfig;
  /** 动画链顶层权重 */
  animationWeights: AnimationWeights;
  /**
   * 素材清单：`webm/` 目录下全部可用素材的**基名**（不含扩展名）。
   *
   * 用途：① 拼素材 URL；② 判断配置里引用的动画是否存在（避免请求 404 后一片空白）。
   */
  availableAnimations: string[];
}

/**
 * 本宠物窗口的运行时状态（Rust → 前端，每帧跟随窗口移动而变化）。
 *
 * 调用方向：**Rust 是位置的唯一真相**。前端只上报"我希望窗口去哪"，
 * Rust 移动窗口后再把真实位置回传；前端用真实位置做命中判定与渲染，
 * 避免两边各自记账导致的累积误差（这是桌面端最容易出的错位 bug）。
 */
export interface PetRuntime {
  /** 窗口内容区左上角在**屏幕坐标系**中的位置（物理像素） */
  windowOrigin: Vec2;
  /** 窗口内容区尺寸（物理像素） */
  windowSize: { width: number; height: number };
  /**
   * 宠物包围盒左上角相对窗口内容区左上角的偏移（物理像素）。
   *
   * 常态下等于外扩余量（本项目为 0）；右键菜单把窗口临时外扩时，这个偏移会变大——
   * 前端据此摆放宠物 DOM，保证**宠物在屏幕上的位置不变**（否则菜单一开宠物就跳）。
   */
  boxOffset: Vec2;
  /**
   * **宿主侧**记账的"窗口此刻是否可交互"（`false` = 整窗点击穿透）。
   *
   * 前端启动时用它对齐自己的"已上报"标志：窗口是**可交互**创建的，
   * 若前端从"穿透"起步，它算出的第一次 `setInteractive(false)` 会被去重吞掉，
   * 窗口就一直吃点击（包围盒里的透明区域也一样）。
   */
  interactive: boolean;
}

/** Rust 事件 `pet://cursor` 的载荷：全局光标位置（屏幕坐标系，物理像素）+ 按键状态 */
export interface CursorSample {
  /** 光标屏幕坐标；坐标系原点 = 桌面左上角（多显示器时可为负） */
  position: Vec2;
  /** 该采样点的时间戳（毫秒，与 performance.now 同基准，由 Rust 侧给出） */
  at: number;
  /**
   * 主键是否按下（宿主的全局按键查询，不依赖窗口消息）。
   *
   * 为什么必须有：窗口在点击穿透期间收不到任何鼠标事件，而拖拽中宠物滞后于光标、
   * 光标会滑出命中区并触发穿透——那一刻的 `pointerup` 会丢，前端状态机就永久卡在
   * "按下中"（宠物继续跟指针、窗口不再恢复穿透）。这个位是唯一可靠的收尾依据。
   */
  primaryDown: boolean;
}

/** Rust 事件 `pet://displays` 的载荷：显示器几何变化（分辨率/缩放/插拔/旋转） */
export interface DisplaysSample {
  /**
   * 全部显示器**工作区**（不含任务栏，屏幕坐标系）。
   * 漫游落点、角落定位、落地判定用它——判定必须是"并集"而不是外接矩形：
   * 不规则布局下外接矩形含大片不属于任何屏的空洞，宠物会飞进去消失。
   */
  areas: Rect[];
  /**
   * 全部显示器**完整面板**（含任务栏区域，与 `areas` 同序，屏幕坐标系）。
   *
   * 用途只有一个但不可省：抛掷越界时探测"那一侧到底有没有邻屏"。
   * 上下叠放的两块屏在接缝处隔一条不属于任何工作区的条带（任务栏所在），
   * 按工作区探测会把那条带当墙——宠物在接缝处反弹、永远穿不过去。
   */
  panels: Rect[];
  /** 主显示器工作区（屏幕坐标系），角落定位用它 */
  primary: Rect;
}
