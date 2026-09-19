/**
 * 视觉动效：Q 弹挤压（squash & stretch）与甩抛飞行。
 *
 * 与上游的手感一致性靠复用纯逻辑保证：
 *   - 挤压曲线 `squashScale` 与落地冲击映射 `landingSquash` 都取自 `@shared/physics.ts`；
 *   - 抛掷单步 `throwStepRegion` 同样取自 shared（**逐屏 AABB 版本**：多显示器不规则布局下
 *     只有它不会把宠物甩进"外接矩形里的空洞"）。
 * 本文件只做三件事：驱动 rAF、把结果写到 DOM transform、把结果上报给 Rust 移动窗口。
 */
import {
  DEFAULT_PHYSICS,
  landingSquash,
  squashScale,
  throwSpace,
  throwStepRegion,
  SQ_DURATION_MS,
  SQ_SQUASH,
  type ThrowSpace,
} from '@shared/physics.ts';
import type { PhysicsParams } from '@shared/types.ts';
import type { Rect, Vec2 } from '../bridge/contract.ts';

/** 只写 transform 的最小元素接口（便于单测时传入假元素） */
interface TransformTarget {
  style: { transform: string };
}

/**
 * Q 弹挤压：以脚底为锚点做垂直压扁 → easeOutBack 回弹过冲。
 *
 * 为什么锚点必须是脚底：宠物"站"在画面底部，如果以中心缩放，压扁时脚会浮起来，
 * 看起来像被拎起而不是被点击。CSS 侧 `#pet-box` 已设 `transform-origin: 50% 100%`。
 */
export class SquashAnimator {
  private rafId = 0;
  private startedAt = 0;
  /** 本次挤压的下压幅度（scaleY 最小值） */
  private depth = SQ_SQUASH;

  constructor(private readonly target: TransformTarget) {}

  /** 是否正在播放（避免点击时打断拖拽时的挤压） */
  get running(): boolean {
    return this.rafId !== 0;
  }

  /** 立即开始一次挤压（已有动画在跑时直接重启，符合"连点反馈"预期） */
  start(depth = SQ_SQUASH): void {
    this.cancel();
    this.depth = depth;
    this.startedAt = performance.now();
    this.tick();
  }

  private tick = (): void => {
    const elapsed = performance.now() - this.startedAt;
    const u = Math.min(elapsed / SQ_DURATION_MS, 1);
    // scaleX 按面积守恒反向放大：压扁时略微变宽，避免"变小"的观感（上游同一处理）
    const scaleY = squashScale(u, this.depth);
    const scaleX = 1 + (1 - scaleY) * 0.35;
    this.target.style.transform = `perspective(600px) scale(${scaleX.toFixed(4)}, ${scaleY.toFixed(4)})`;
    if (u >= 1) {
      this.rafId = 0;
      this.target.style.transform = 'none';
      return;
    }
    this.rafId = requestAnimationFrame(this.tick);
  };

  /** 取消并复位（拖拽开始时调用，避免倾斜/压扁状态下被拖走） */
  cancel(): void {
    if (this.rafId !== 0) cancelAnimationFrame(this.rafId);
    this.rafId = 0;
    this.target.style.transform = 'none';
  }
}

/** 抛掷飞行参数（由调用方在 start 时给出） */
export interface ThrowOptions {
  /** 起始包围盒位置（屏幕坐标） */
  box: Vec2;
  /** 初速（px/s） */
  velocity: Vec2;
  /** 宠物宽度（用于计算逐屏边界） */
  size: number;
  /** 身体相对包围盒左右各内缩的量（贴边反弹语义：允许身侧贴边而非透明边贴边） */
  sideAllow: number;
  /** 显示器工作区并集（屏幕坐标） */
  areas: Rect[];
  /**
   * 逐屏**完整面板**（含任务栏，与 areas 同序）：屏缝处"有没有邻屏"的探测用它。
   * 缺省 = areas（等价于接缝处没有邻屏，与上游 throwSpace 的兜底语义一致）。
   */
  panels?: Rect[];
  /** 每帧上报当前包围盒位置（屏幕坐标）→ 调用方驱动窗口跟随 */
  onFrame(box: Vec2): void;
  /** 落地静止/飞行结束时回调（携带最终包围盒位置与落地冲击速度，供 Q 弹使用） */
  onRest(box: Vec2, impactSpeed: number): void;
}

/**
 * 甩抛飞行：重力 + 碰壁反弹 + 地面摩擦 + 落地静止判定。
 *
 * 状态全部在屏幕坐标系里推进（宠物包围盒左上角），窗口只是它的"投影"。
 * 这样做的好处：多显示器跨屏飞行、落地判定、边界反弹与窗口无关，逻辑一次写对；
 * 窗口跟随只是每帧一次 `set_pet_bounds`，不影响物理结果。
 *
 * 关于 `atRest`：纯逻辑会给出"贴地且低速"或"碰边后整体低速"的收敛判定；
 * 这里额外加一层**演出超时**，防止极端参数（如 gravity=0 或极低摩擦）下无限飞行。
 */
export class ThrowAnimator {
  private rafId = 0;
  private lastAt = 0;
  private state: { x: number; y: number; vx: number; vy: number } | null = null;
  private space: ThrowSpace | null = null;
  private options: ThrowOptions | null = null;
  private physics: PhysicsParams = DEFAULT_PHYSICS;
  /** 落地冲击速度（取最近一次 bounced 时的 |vy|，用于落地 Q 弹的力度映射） */
  private lastImpactSpeed = 0;
  /** 本次飞行的开始时刻，用于演出超时兜底 */
  private startedAt = 0;

  /** 演出超时（ms）：超过它强制落地静止，避免参数异常时宠物永远飞 */
  private static readonly MAX_FLIGHT_MS = 20_000;

  setPhysics(physics: PhysicsParams): void {
    this.physics = physics;
  }

  /** 是否正在飞行 */
  get running(): boolean {
    return this.rafId !== 0;
  }

  /** 开始一次飞行（重复调用会以新初速重启，与"被撞飞"的语义一致） */
  start(options: ThrowOptions): void {
    this.stop();
    this.options = options;
    this.state = { x: options.box.x, y: options.box.y, vx: options.velocity.x, vy: options.velocity.y };
    // 逐屏抛掷空间：每块屏一套 AABB + 完整面板（用于"越界侧有没有邻屏"的探测）
    this.space = throwSpace({
      areas: options.areas,
      panels: options.panels,
      size: options.size,
      sideAllow: options.sideAllow,
    });
    this.lastAt = performance.now();
    this.startedAt = this.lastAt;
    this.lastImpactSpeed = 0;
    this.tick();
  }

  private tick = (): void => {
    const opts = this.options;
    const sp = this.space;
    const s = this.state;
    if (!opts || !sp || !s) {
      this.rafId = 0;
      return;
    }

    const now = performance.now();
    // 单步 dt 由 shared 侧钳制（MAX_STEP_DT），这里直接传原始 dt
    const dt = (now - this.lastAt) / 1000;
    this.lastAt = now;

    const next = throwStepRegion(s, dt, sp, this.physics);
    this.state = { x: next.x, y: next.y, vx: next.vx, vy: next.vy };
    if (next.bounced) {
      // 反弹瞬间的纵向速度绝对值即"落地冲击速度"（弹起后 vy 已反向，故用状态里的原始值近似）
      this.lastImpactSpeed = Math.max(this.lastImpactSpeed, Math.abs(s.vy));
    }
    opts.onFrame({ x: next.x, y: next.y });

    const timeout = now - this.startedAt > ThrowAnimator.MAX_FLIGHT_MS;
    if (next.atRest || timeout) {
      this.rafId = 0;
      this.options = null;
      this.space = null;
      this.state = null;
      opts.onRest({ x: next.x, y: next.y }, this.lastImpactSpeed);
      return;
    }
    this.rafId = requestAnimationFrame(this.tick);
  };

  /** 停止飞行（不回调 onRest；用于用户重新抓住宠物等打断场景） */
  stop(): void {
    if (this.rafId !== 0) cancelAnimationFrame(this.rafId);
    this.rafId = 0;
    this.options = null;
    this.space = null;
    this.state = null;
    this.lastImpactSpeed = 0;
  }
}

/**
 * 落地 Q 弹的幅度：把冲击速度映射成下压深度。
 * 抽成函数是为了让"轻落 0.8 ~ 重砸 0.55"的规则在代码里只有一处（上游同一规则）。
 */
export function squashDepthForImpact(impactSpeed: number): number {
  return landingSquash(impactSpeed);
}
