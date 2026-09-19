/**
 * 拖拽交互：过阻尼弹簧跟手 + 松手甩抛。
 *
 * 与上游 dsh-pet 的手感对齐，靠的是**复用同一份纯逻辑**：
 *   - 松手初速估算    → `@shared/physics.ts` 的 `estimateReleaseVelocity`（端点均值 + 峰值加权 + 停顿判定 + 软钳速）
 *   - 拖拽轨迹裁剪    → 同文件的 `trimTrail`
 * 本文件只负责"什么时候调用它们"以及输入/边界判定，不做任何物理公式的二次实现。
 *
 * **唯一的例外是跟手弹簧**（见下面的 `DRAG_FOLLOW_K/C`）：上游 `springStep` 的 K/C 比例
 * 让随动滞后恒为 `v·C/K ≈ 0.15s`，快速拖动时宠物明显"飘在后面"（用户实测反馈），
 * 因此这一层按"同一套过阻尼弹簧公式、不同的 K/C"自己积分——shared 文件保持零改动。
 *
 * 坐标约定（关键，错了必然错位）：
 *   物理层（physics.ts）的坐标语义 = **宠物包围盒左上角**的屏幕坐标。
 *   因此这里所有坐标都在"屏幕坐标系"里运算，只在最后交给 Rust 移动窗口时按窗口外扩
 *   余量换算一次（见 runtime.ts）。绝不混入窗口本地坐标。
 *
 * 状态机：
 *   idle --pointerdown(命中区)--> pressed --位移>阈值--> dragging --pointerup--> 抛出/放下 --> idle
 *   点击（未超过位移阈值）= pressed --pointerup--> 派发 click 回调
 */
import {
  DEFAULT_PHYSICS,
  estimateReleaseVelocity,
  trimTrail,
  type DragSample,
} from '@shared/physics.ts';
import { DRAG_THRESHOLD } from '@shared/constants.ts';
import type { PhysicsParams } from '@shared/types.ts';
import type { Vec2 } from '../bridge/contract.ts';

/** 一次拖拽结束后的结果 */
export interface DragRelease {
  /** 松手时的宠物包围盒位置（屏幕坐标） */
  box: Vec2;
  /** 估算出的初速（px/s）；null = 温柔放下（不抛） */
  velocity: Vec2 | null;
}

/** 拖拽控制器的对外回调 */
export interface DragCallbacks {
  /**
   * 拖拽中每帧的目标位置（宠物包围盒左上角，屏幕坐标）。
   * 调用方据此驱动窗口跟随（Rust 侧的 set_pet_bounds）。
   */
  onDragMove(target: Vec2): void;
  /** 松手：携带最终位置与初速（velocity 为 null 表示放下不抛） */
  onRelease(result: DragRelease): void;
  /** 未拖动就松手 = 点击（用于播放点击回应动画 + Q 弹） */
  onClick(point: Vec2): void;
  /** 拖拽状态翻转（用于 inputBusy 上报、光标样式等） */
  onDragStateChange(dragging: boolean): void;
}

/** 弹簧跟随的每帧最大积分步长（秒）：与物理层 MAX_STEP_DT 同一量级，防卡顿后巨帧跳变 */
const MAX_SPRING_DT = 0.05;

/**
 * 跟手弹簧刚度。**与上游不同的唯一一处物理常量**（上游 `SPRING_K = 200`）。
 *
 * 为什么调它：弹簧跟随的**稳态滞后**是 `v · C / K`——与刚度无关、只由 C/K 决定。
 * 上游 C/K = 30/200 = 0.15s，等于"宠物永远比光标晚 0.15 秒"：快速拖动（1000px/s）时
 * 能落后 150px，用户实测的观感就是"跟得太松、飘"（`?autotest=10` 量到的平均滞后 41~55px
 * 是 560px/s 匀速拖动下的值）。
 *
 * 取值：K = 600，并按**临界阻尼**配 C = 2√K ≈ 49（ζ = 1.0，不 overshoot）：
 *   C/K = 2/√600 ≈ 0.0817s → 同一拖动速度下滞后只有原来的 **55%**；
 *   刚度提高 3 倍仍然安全：显式欧拉稳定的步长上限是 2/√K ≈ 82ms，而单帧步长被
 *   `MAX_SPRING_DT = 50ms` 钳住（上游 K=200 时上限 141ms，余量更大，所以这是"更贴手"
 *   与"卡顿后不抖"之间的折中，不要再往上翻倍）。
 *
 * 想更贴/更松就只改 K 这一个数（C 会跟着重算，阻尼形态保持不变）。
 */
const DRAG_FOLLOW_K = 600;
/** 跟手弹簧阻尼：临界阻尼 ζ=1（上游是 ζ≈1.06 的轻微过阻尼，这里不要 overshoot） */
const DRAG_FOLLOW_C = 2 * Math.sqrt(DRAG_FOLLOW_K);

/** 拖拽轨迹保留窗口（ms）：只留最近这一段做初速估算 */

/** 按下来源：决定"松手算不算点击"，以及运动是否允许脱离身体命中区 */
export type PressOrigin = 'dom' | 'sampler';

export class DragController {
  /** 是否处于"按下"状态（含尚未超过阈值的按压） */
  private pressed = false;
  /** 是否已经判定为拖拽（位移超过 DRAG_THRESHOLD） */
  private dragging = false;
  /** 本次按下的来源（DOM 快路径 / 全局采样兜底） */
  private pressOrigin: PressOrigin = 'dom';
  /**
   * 本次按下绑定的 DOM 指针 id（`-1` = 采样兜底，不绑定具体指针）。
   * 用途：多指针（触摸）时只有发起按下的那根手指能把拖拽继续下去或收尾，
   * 避免第二根手指的 pointermove/pointerup 让宠物乱跳。
   */
  private pressPointerId = -1;
  /** 按下时指针相对宠物包围盒左上角的偏移（保证拖拽不"跳一下"） */
  private grabOffset: Vec2 = { x: 0, y: 0 };
  /** 指针按下时的屏幕坐标（用于位移阈值判定） */
  private pressedAt: Vec2 = { x: 0, y: 0 };
  /** 拖拽轨迹采样（估算松手初速用） */
  private trail: DragSample[] = [];
  /** 弹簧跟随的当前位置与速度（宠物包围盒左上角，屏幕坐标） */
  private box: Vec2 = { x: 0, y: 0 };
  private velocity: Vec2 = { x: 0, y: 0 };
  /** 本帧指针目标位置（弹簧的输入） */
  private target: Vec2 = { x: 0, y: 0 };
  /** 上一次弹簧积分的时刻（performance.now 基准，毫秒） */
  private lastStepAt = 0;
  /** 物理参数（由配置注入；默认值仅在配置缺失时兜底） */
  private physics: PhysicsParams = DEFAULT_PHYSICS;

  constructor(private readonly callbacks: DragCallbacks) {}

  /** 注入/更新物理参数（配置热重载时调用） */
  setPhysics(physics: PhysicsParams): void {
    this.physics = physics;
  }

  /** 同步宠物当前包围盒位置（非拖拽期间由窗口位置驱动，保证按下瞬间的抓取偏移正确） */
  syncBox(box: Vec2): void {
    if (this.pressed) return; // 拖拽中位置由弹簧自治，不被外部覆盖
    this.box = { ...box };
  }

  /** 当前是否正在拖拽（对外查询：inputBusy 判定用） */
  get isDragging(): boolean {
    return this.dragging;
  }

  /** 当前是否被按下（含未成拖拽的按压） */
  get isPressed(): boolean {
    return this.pressed;
  }

  /** 本次按下的来源（runtime 用它决定是否套用牵引绳） */
  get origin(): PressOrigin {
    return this.pressOrigin;
  }

  /**
   * 给定 DOM 指针 id 是否就是发起本次按下的那根指针。
   * 采样兜底的按下（pointerId = -1）不绑定指针，因此对任何 id 都返回 true——
   * 那种情况下页面本来就没在收 DOM 事件，不会有多指针串台问题。
   */
  matchesPointer(pointerId: number): boolean {
    if (this.pressPointerId < 0) return true;
    return pointerId === this.pressPointerId;
  }

  /**
   * 本次按下时指针偏离"按下点"的距离（px）。
   * 牵引绳阈值由调用方（runtime）持有：那里才知道命中几何与窗口状态。
   */
  leashDistance(pointerScreen: Vec2): number {
    return Math.hypot(pointerScreen.x - this.pressedAt.x, pointerScreen.y - this.pressedAt.y);
  }

  /** 最近一次按下的指针屏幕坐标（判断"按下时是否在身体上"用） */
  get pressedPoint(): Vec2 {
    return { ...this.pressedAt };
  }

  /**
   * 指针按下（只在身体命中区内触发，见 runtime.ts 的命中判定）。
   *
   * @param pointerScreen 指针的**屏幕坐标**（由光标采样通道给出）
   * @param boxScreen 当前宠物包围盒左上角的屏幕坐标
   * @param origin 本次按下的来源；`sampler` 表示 DOM 事件没到（穿透期间按下），
   *               此时"运动"会被限制在身体命中区内，避免状态失同步时宠物跟着指针乱跑
   * @param pointerId 绑定到本次按下的 DOM 指针 id；采样兜底传 -1
   */
  onPointerDown(pointerScreen: Vec2, boxScreen: Vec2, origin: PressOrigin = 'dom', pointerId = -1): void {
    this.pressed = true;
    this.dragging = false;
    this.pressOrigin = origin;
    this.pressPointerId = pointerId;
    this.pressedAt = { ...pointerScreen };
    this.grabOffset = { x: pointerScreen.x - boxScreen.x, y: pointerScreen.y - boxScreen.y };
    this.box = { ...boxScreen };
    this.target = { ...boxScreen };
    this.velocity = { x: 0, y: 0 };
    this.lastStepAt = performance.now();
    this.trail = [{ t: this.lastStepAt, x: pointerScreen.x, y: pointerScreen.y }];
  }

  /**
   * 指针移动（拖拽中由光标采样通道高频调用）。
   *
   * 说明：不直接用 DOM 的 `pointermove`，而是吃"全局光标采样"，原因见 cursor.ts：
   *   窗口在点击穿透状态下收不到任何鼠标事件，只有全局采样是连续可信的。
   *
   * @param pointerScreen 指针屏幕坐标（**已在 runtime 里按"牵引绳"过滤过**——
   *   采样兜底的按下若偏离按下点超过 MAX_DRAG_LEASH 就不再跟随，避免失同步时宠物跑飞。
   *   过滤放在 runtime 是因为它掌握命中判定与窗口状态。）
   */
  onPointerMove(pointerScreen: Vec2): void {
    if (!this.pressed) return;
    const moved = Math.hypot(pointerScreen.x - this.pressedAt.x, pointerScreen.y - this.pressedAt.y);
    if (!this.dragging) {
      if (moved < DRAG_THRESHOLD) return; // 阈值内仍视为点击
      this.dragging = true;
      this.callbacks.onDragStateChange(true);
    }
    // 目标位置 = 指针位置 - 抓取偏移（宠物不会在按下瞬间跳到指针中心）
    this.target = { x: pointerScreen.x - this.grabOffset.x, y: pointerScreen.y - this.grabOffset.y };
    const now = performance.now();
    this.trail = trimTrail([...this.trail, { t: now, x: pointerScreen.x, y: pointerScreen.y }], now);
    this.stepSpring(now);
  }

  /**
   * 指针抬起：判定点击或松手抛掷。
   *
   * @param now 当前时刻（performance.now，毫秒）；默认取实时值
   */
  onPointerUp(now = performance.now()): void {
    if (!this.pressed) return;
    const wasDragging = this.dragging;
    this.pressed = false;
    this.dragging = false;

    if (!wasDragging) {
      // 位移未超阈值 = 点击。
      // 无论来自 DOM 快路径还是全局采样兜底，语义完全一致：
      // 采样兜底的按压同样是在身体命中区内开始的（runtime 只放行体内按下），
      // 所以"点一下"的反馈不会因为事件来源不同而分叉。
      this.callbacks.onDragStateChange(false);
      this.callbacks.onClick({ ...this.pressedAt });
      this.trail = [];
      return;
    }

    // 松手初速：由纯逻辑估算（停顿过久 / 轨迹太短 / 低于死区 → null = 温柔放下）
    // 注意两套命名：shared 的物理量用 (vx, vy)，本项目的桥接契约用 (x, y)，这里显式转换一次。
    const estimated = estimateReleaseVelocity(this.trail, now, this.physics);
    this.trail = [];
    this.callbacks.onDragStateChange(false);
    this.callbacks.onRelease({
      box: { x: this.box.x, y: this.box.y },
      velocity: estimated ? { x: estimated.vx, y: estimated.vy } : null,
    });
  }

  /**
   * 弹簧跟手单步积分。
   *
   * 用**过阻尼弹簧**而不是"位置直接等于指针"：后者在快速甩动时会让宠物瞬间贴合指针，
   * 失去"被拎着甩"的重量感；弹簧则保留一点自然的滞后与回正。
   *
   * 公式与上游 `springStep` 完全一致：
   *   `v' = v + ((target − x)·K − v·C)·power·dt`，随后 `x' = x + v'·dt`
   * `power` 仍然取配置里的 `throwPower`（它把 K/C 同乘，只改变收敛**速度**、
   * 不改变稳态滞后 `v·C/K`）。唯一差别是 K/C 的取值，理由见 `DRAG_FOLLOW_K` 的注释。
   */
  private stepSpring(now: number): void {
    const dt = Math.min(Math.max((now - this.lastStepAt) / 1000, 0), MAX_SPRING_DT);
    this.lastStepAt = now;
    if (dt <= 0) return;

    const power = this.physics.throwPower;
    const k = DRAG_FOLLOW_K * power;
    const c = DRAG_FOLLOW_C * power;
    this.velocity.x += ((this.target.x - this.box.x) * k - this.velocity.x * c) * dt;
    this.velocity.y += ((this.target.y - this.box.y) * k - this.velocity.y * c) * dt;
    this.box.x += this.velocity.x * dt;
    this.box.y += this.velocity.y * dt;
    this.callbacks.onDragMove({ ...this.box });
  }
}
