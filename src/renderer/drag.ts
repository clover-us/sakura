/**
 * 拖拽交互：过阻尼弹簧跟手 + 松手甩抛。
 *
 * 与上游 dsh-pet 的手感严格对齐，靠的是**复用同一份纯逻辑**：
 *   - 弹簧步进        → `@shared/physics.ts` 的 `springStep`（K=200 / C=30，ζ≈1.06 过阻尼，不 overshoot）
 *   - 松手初速估算    → 同文件的 `estimateReleaseVelocity`（端点均值 + 峰值加权 + 停顿判定 + 软钳速）
 * 本文件只负责"什么时候调用它们"以及输入/边界判定，不做任何物理公式的二次实现。
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
  springStep,
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
   * 失去"被拎着甩"的重量感；弹簧则有一点自然的滞后与回正（K=200/C=30 由上游调定）。
   * 目标位置是窗口级的，因此这里算出来的 `box` 直接就是窗口该去的位置。
   */
  private stepSpring(now: number): void {
    const dt = Math.min(Math.max((now - this.lastStepAt) / 1000, 0), MAX_SPRING_DT);
    this.lastStepAt = now;
    if (dt <= 0) return;

    const power = this.physics.throwPower;
    this.velocity.x = springStep(this.velocity.x, this.box.x, this.target.x, dt, power);
    this.velocity.y = springStep(this.velocity.y, this.box.y, this.target.y, dt, power);
    this.box.x += this.velocity.x * dt;
    this.box.y += this.velocity.y * dt;
    this.callbacks.onDragMove({ ...this.box });
  }
}
