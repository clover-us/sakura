/**
 * 动画链：宠物"永不停歇"的行为循环。
 *
 * 与上游语义完全一致（选择逻辑复用 `@shared/pickers.ts`，不重写一份）：
 *   每段动画播完 → 按权重掷骰决定下一类 → 从对应池里抽一个 → 立刻播下一段。
 *
 *   掷骰（`rollKind`，权重来自 `animationWeights`）：
 *     idle 10% / turn 5% / move 5% / 随机动作 80%（= 各分类 weight 之和）
 *   - idle   ：待机池随机抽一个（避免与当前重复）
 *   - turn   ：转向池随机抽一个，**播完翻转朝向**
 *   - move   ：移动池随机抽一个，**真的走一段**（由 runtime 驱动位移）
 *   - action ：按分类 weight 抽一个分类（noMirror 分类在朝右时跳过），再从分类里抽动作
 *
 * 三条容易踩的规则（都在上游有对应实现，这里逐条对齐）：
 *   1. **避免连续重复**：抽动作时把"当前正在播的"作为 exclude（`pick` / `pickCategoryAction` 的语义）；
 *   2. **镜像**：宠物朝右时整体水平镜像（`scaleX(-1)`），但 `noMirror` 分类（带文字的动作）
 *      在朝右时**不参与抽取**——否则文字会左右颠倒；
 *   3. **事件动画不进随机链**：余额/碎碎念等由代码显式触发（M3 接入），随机链不会抽到它们。
 */
import {
  isEventAnim,
  pick,
  pickCategoryAction,
  pickSlot,
  rollKind,
  type RollKind,
} from '@shared/pickers.ts';
import type { AnimationsConfig, AnimationWeights } from '../bridge/contract.ts';
import { petLog } from '../bridge/log.ts';

/** 朝向（与上游一致：`left` = 原始朝向，`right` = 镜像后的朝向） */
export type Facing = 'left' | 'right';

/** 一次移动的几何计划（由 `@shared/motion.ts` 的 planMove 产出） */
export interface WalkPlan {
  /** 动画名（用于播放走路动画） */
  animation: string;
  /** 动画首尾各多少秒原地不动（秒） */
  leadSec: number;
  tailSec: number;
  /** 起止包围盒中心（**屏幕坐标**） */
  fromX: number;
  toX: number;
  /** 包围盒中心的 y（屏幕坐标；走路不改 y） */
  centerY: number;
  /** 走路方向（决定朝向） */
  dir: 1 | -1;
}

/** 动画链的对外回调 */
export interface ChainCallbacks {
  /** 需要真的走一段（runtime 负责位移与逐帧窗口跟随） */
  onWalk(plan: WalkPlan): void;
  /** 朝向变化（runtime 负责把镜像写进 DOM） */
  onFacingChange(facing: Facing): void;
  /** 需要播放某段动画（由 runtime 交给 MediaBuffer；返回 Promise 便于串行） */
  play(animation: string, mode: 'loop' | 'once'): Promise<void>;
  /**
   * 询问"能否朝某方向走一段"并给出计划；返回 null = 走不了（空间不足）。
   * 由 runtime 实现（它掌握显示器几何与宠物包围盒）。
   */
  planWalk(dir: 1 | -1, spec: { minDist: number; maxDist: number; margin: number }): WalkPlan | null;
}

export class AnimationChain {
  /** 当前正在播的动画名（避免连续重复用） */
  private current = '';
  /** 当前朝向 */
  private facing: Facing = 'left';
  /** 是否暂停（拖拽中会暂停"选下一个"，避免拖拽时宠物突然切动作） */
  private paused = false;
  /** 链是否在运行（dispose 后停止） */
  private running = false;

  constructor(
    private readonly animations: AnimationsConfig,
    private readonly weights: AnimationWeights,
    private readonly callbacks: ChainCallbacks,
  ) {}

  /** 启动动画链：先播一段待机 */
  async start(): Promise<void> {
    this.running = true;
    const first = pick(this.animations.idle);
    await this.switchTo(first);
  }

  /** 当前朝向 */
  get currentFacing(): Facing {
    return this.facing;
  }

  /** 当前动画名 */
  get currentAnimation(): string {
    return this.current;
  }

  /** 暂停/恢复（拖拽期间暂停选择，松手后恢复） */
  setPaused(paused: boolean): void {
    this.paused = paused;
  }

  /** 停止（取消后续调度） */
  stop(): void {
    this.running = false;
  }

  /**
   * 由 MediaBuffer 在"单次播放结束"时调用：选并播放下一段。
   *
   * 之所以由播放器回调驱动而不是定时器：动画长度不一（3~12 秒），
   * "播完即接"才能做到无缝；这也是上游"永不停止的动画链"的实现方式。
   */
  async onAnimationEnded(endedAnimation: string): Promise<void> {
    if (!this.running || this.paused) return;
    // 只有"当前正在播"的那段结束才推进（双缓冲切换时旧层也会发 ended，要忽略）
    if (endedAnimation !== this.current) return;
    // 事件动画播完统一回待机（事件由代码显式触发，不进随机链）
    if (isEventAnim(this.animations.events, endedAnimation)) {
      await this.switchTo(pick(this.animations.idle, endedAnimation));
      return;
    }
    await this.advance();
  }

  /**
   * 推进一段：掷骰 → 抽动作 → 播放。
   *
   * `move` 需要真的位移，因此它先向 runtime 要一个"行走计划"：
   * 计划拿不到（空间不足 / 贴着屏幕边）就**退化为待机**——与上游"检查空间、
   * 走不了就不走"的语义一致（`planMove` 返回 null 的处理）。
   */
  async advance(): Promise<void> {
    if (!this.running) return;
    const kind: RollKind = rollKind(Math.random(), this.weights);
    petLog(`动画链: 掷骰=${kind}（当前 ${this.current || '(无)'}，朝向 ${this.facing}）`);

    if (kind === 'move') {
      const plan = this.pickMovePlan();
      if (plan) {
        this.callbacks.onWalk(plan);
        await this.switchTo(plan.animation);
        return;
      }
      // 走不了 → 退化为待机（不留空档）
      await this.switchTo(pick(this.animations.idle, this.current));
      return;
    }

    if (kind === 'turn') {
      const turn = this.animations.turn;
      if (turn.length > 0) {
        const next = pick(turn, this.current);
        // 转向动画播完要翻转朝向（与上游一致：翻转在"播放开始"时记下，结束时生效）
        this.pendingTurnFlip = true;
        await this.switchTo(next);
        return;
      }
      await this.switchTo(pick(this.animations.idle, this.current));
      return;
    }

    if (kind === 'idle') {
      await this.switchTo(pick(this.animations.idle, this.current));
      return;
    }

    // 随机动作：按分类权抽取（noMirror 分类在朝右时被过滤）
    const picked = pickCategoryAction(this.animations.categories, this.animations.idle, this.facing, this.current);
    petLog(`动画链: 随机动作 分类=${picked.id} 动作=${picked.name}`);
    await this.switchTo(picked.name);
  }

  /** 本次 `turn` 动画结束后是否要翻转朝向 */
  private pendingTurnFlip = false;

  /** 播放一段动画并记录状态 */
  private async switchTo(animation: string): Promise<void> {
    if (!this.running) return;
    // 素材缺失时不要反复请求 404：退化到待机池里第一个可用的
    if (!this.hasAsset(animation)) {
      petLog(`动画链: 素材缺失，跳过 ${animation}`);
      const fallback = this.animations.idle.find((name) => this.hasAsset(name));
      if (!fallback || fallback === animation) return;
      animation = fallback;
    }
    this.current = animation;
    // **一律按单次播**：`ended` 只在非循环时触发，而动画链的推进靠它。
    // 传进来的 mode 只表达"这段是否循环"的意图，实际由"播完立刻选下一段"实现循环感
    // （待机段通常会被反复抽到，看起来就是循环）。
    await this.callbacks.play(animation, 'once');
  }

  /** 素材清单是否包含该动画（`availableAnimations` 存的是基名） */
  private hasAsset(animation: string): boolean {
    return this.assets.has(animation);
  }

  /** 素材基名集合（由 runtime 注入；避免每次线性查找） */
  private assets = new Set<string>();

  /** 注入素材清单 */
  setAvailableAnimations(list: string[]): void {
    this.assets = new Set(list);
  }

  /**
   * 抽一个可行的移动计划（抽不出来就返回 null，由调用方退化为待机）。
   *
   * 迁移原语义：尝试若干次随机抽取，直到 `planWalk` 给出可行计划——
   * 因为 `moves.actions` 里每个动作的距离范围不同，某些动作在当前位置走不了。
   */
  private pickMovePlan(): WalkPlan | null {
    const specs = this.animations.moves.actions;
    if (specs.length === 0) return null;
    const defaults = this.animations.moves.default;
    for (let attempt = 0; attempt < 5; attempt += 1) {
      const spec = specs[Math.floor(Math.random() * specs.length)];
      const merged = { ...defaults, ...(spec.params ?? {}) };
      const dir: 1 | -1 = Math.random() < 0.5 ? -1 : 1;
      const plan = this.callbacks.planWalk(dir, {
        minDist: numberOr(merged.minDist, 60),
        maxDist: numberOr(merged.maxDist, 240),
        margin: numberOr(merged.margin, 20),
      });
      if (plan) {
        return { ...plan, animation: spec.name, leadSec: numberOr(merged.leadSec, 2), tailSec: numberOr(merged.tailSec, 2), dir };
      }
    }
    return null;
  }

  /**
   * 应用"朝向翻转"副作用（由 runtime 在动画结束时调用）。
   *
   * 分成显式方法而不是在 switchTo 里做：镜像要写到 DOM，而 chain 不碰 DOM
   * （保持它与 shared 纯逻辑同层的可测性）。
   */
  applyTurnFlipIfPending(): void {
    if (!this.pendingTurnFlip) return;
    this.pendingTurnFlip = false;
    this.setFacing(this.facing === 'left' ? 'right' : 'left');
  }

  /** 设置朝向（幂等；变化时回调出去） */
  setFacing(facing: Facing): void {
    if (this.facing === facing) return;
    this.facing = facing;
    this.callbacks.onFacingChange(facing);
  }

  /** 显式触发一个事件动画（余额/碎碎念等；不进随机链） */
  async playEvent(name: string, slotIndex: number): Promise<void> {
    const pool = this.animations.events[name];
    if (!pool || pool.length === 0) {
      petLog(`动画链: 事件 ${name} 没有配置档位`);
      return;
    }
    const index = Math.min(Math.max(slotIndex, 0), pool.length - 1);
    const animation = pickSlot(pool[index], this.current);
    petLog(`动画链: 事件 ${name} 档位 ${index} → ${animation}`);
    await this.switchTo(animation);
  }
}

/** 从可能缺失/非法的配置值里取一个有限数（配置校验已在上游做过，这里只兜底） */
function numberOr(value: unknown, fallback: number): number {
  return typeof value === 'number' && Number.isFinite(value) ? value : fallback;
}

/** 走路的逐帧中心位置（线性插值；lead/tail 期间原地不动，与上游一致） */
export function walkCenterAt(plan: WalkPlan, elapsedMs: number, durationMs: number): number {
  const leadMs = plan.leadSec * 1000;
  const tailMs = plan.tailSec * 1000;
  const moveMs = Math.max(durationMs - leadMs - tailMs, 1);
  const t = Math.min(Math.max((elapsedMs - leadMs) / moveMs, 0), 1);
  return plan.fromX + (plan.toX - plan.fromX) * t;
}
