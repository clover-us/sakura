/**
 * 宠物运行时装配：把「配置 / 媒体 / 命中判定 / 拖拽 / 抛掷 / 窗口跟随」接成一个闭环。
 *
 * 数据流（单一真相 = Rust 侧的窗口位置）：
 *
 *   Rust 窗口位置 ──emit(pet://cursor / 本窗口 bounds 变化)──▶ 本模块
 *        ▲                                                      │
 *        │                                              命中判定（本地坐标）
 *        │                                                      ▼
 *        └──invoke(set_pet_interactive)── 可交互 / 点击穿透 ◀── 是否在身体上
 *
 *   拖拽：光标采样 → DragController（屏幕坐标弹簧）→ 目标包围盒 →
 *         invoke(set_pet_bounds) → Rust 移动窗口 → 回传真实位置 → 更新本地状态
 *
 * 关键不变量：
 *   - **物理与命中一律在屏幕坐标系**运算，只有写 DOM 布局时才用本地坐标；
 *   - 窗口位置永远以 Rust 回传的值为准，前端不自行累积位置（避免漂移）。
 */
import { DEFAULT_PHYSICS } from '@shared/physics.ts';
import { anchorPixel, planMove } from '@shared/motion.ts';
import { pick, pickSlot } from '@shared/pickers.ts';
import type { PhysicsParams } from '@shared/types.ts';
import { petLog, petLogError } from '../bridge/log.ts';
import type { DisplaysSample, PetConfig, PetRuntime as PetRuntimeState, Rect, Vec2 } from '../bridge/contract.ts';
import { invoke } from '../bridge/tauri.ts';
import { prefersReducedMotion } from './dom.ts';
import { isInsideHitBox, applyHitBoxStyle, scaleHitBox } from './hitbox.ts';
import { MediaBuffer } from './media-buffer.ts';
import { CursorChannel, type CursorFrame } from './cursor.ts';
import { DragController, type PressOrigin } from './drag.ts';
import { AnimationChain, walkCenterAt, type Facing, type WalkPlan } from './chain.ts';
import { isNoMirrorAnimation } from '@shared/menu.ts';
import { SquashAnimator, ThrowAnimator, squashDepthForImpact } from './anim.ts';
import { screenToLocal, windowMargin } from './coords.ts';

/** 拖拽时身体左右允许越出的量（贴边语义：身侧可贴屏幕边，透明边不算） */
const SIDE_ALLOW_RATIO = 0.15;

/**
 * 采样兜底按下的牵引绳长度（px）：与 drag.ts 的 MAX_DRAG_LEASH 保持同一语义。
 * 放在 runtime 是因为判定需要命中几何与窗口状态，drag 控制器只提供"偏离按下点多远"。
 */
const MAX_DRAG_LEASH = 400;

/**
 * 甩出速度的整体折扣（1.0 = 上游原样）。
 *
 * 上游 `shared/physics.ts` 的初速估算本身是准的（实测：指针 639px/s 的匀速拖拽估出 601px/s），
 * 但它的**死区只有 500px/s**、峰值还加权 50%、末段加速最多再放大 60%——
 * 这套参数是按"甩得很凶"的手感调的，实际用起来偏灵敏：随手一抛就飞很远，
 * 而且飞行+弹跳要好几秒才停（用户反馈"过于灵敏、甩飞出去"）。
 *
 * 这里把估算结果整体乘 0.6：等价于把手感从"轻甩即飞"拉回"用力才飞"，
 * 同时保留了"越用力飞越远"的单调性（软上限也一起按比例缩小）。
 * 改这一个数就能调整体手感；要更钝就继续往下调。
 */
const RELEASE_SPEED_SCALE = 0.6;

/**
 * 看门狗收尾所需的 **DOM 静默时长**（ms）。
 *
 * 为什么看门狗不能只看"按键位"（实测教训）：
 *   本机的全局输入查询并不可靠——`GetAsyncKeyState(VK_LBUTTON)` 会报"按键已松开"，
 *   尽管用户正按着拖拽。把它单独当作收尾依据，真实按下会在**2ms 内**被误收尾
 *   （日志实测：`按下 16.972` → `由全局按键采样收尾 16.974`），表现为"刚按住就脱手"，
 *   紧接着宠物按同一方向飞出去再弹回（松手瞬间的初速被当成了甩抛）。
 *
 * 正确判据 = **双条件**：
 *   按键位报"松开" **且** DOM 指针事件已经静默 ≥ 本阈值。
 *   DOM 事件在窗口可交互时一定有；它静默说明"窗口已经回到穿透态、事件确实到不了页面"，
 *   这时再看门狗才是有意义的补位。按键位错报、或 DOM 事件稍慢，都不会再误杀拖拽。
 */
const DOG_SILENCE_MS = 250;

/**
 * 气泡锚点相对**宠物包围盒顶边**下压的比例（按包围盒**高度**算）。
 *
 * 包围盒是整段视频的外框（`size × size*aspectRatio`），角色头顶通常在框内还空着几十像素——
 * 直接按框顶边放气泡，"尖角"离头发还有一段距离，看起来像飘在天上（用户实测反馈）。
 * 0.13 × 236 ≈ 31px，尖角正好落在头发上；换素材/换尺寸时只调这一个系数。
 */
const BUBBLE_HEAD_RATIO = 0.13;

/** 运行时所需的 DOM 句柄 */
export interface PetDom {
  /** 包围盒根节点（Q 弹 transform 写在它上面） */
  box: HTMLElement;
  /** 双缓冲视频层 A */
  videoA: HTMLVideoElement;
  /** 双缓冲视频层 B */
  videoB: HTMLVideoElement;
  /** 身体命中区（唯一可交互区域） */
  hit: HTMLElement;
}

export class PetRuntime {
  // ---- 不可变配置 ----
  /** 当前宠物配置 */
  private readonly petConfig: PetConfig;
  /** 命中框（包围盒内坐标） */
  private readonly hitBox: Rect;
  /** 双缓冲播放器 */
  private readonly media: MediaBuffer;
  /** Q 弹挤压 */
  private readonly squash: SquashAnimator;
  /** 甩抛飞行 */
  private readonly thrower: ThrowAnimator;
  /** 拖拽控制器 */
  private readonly drag: DragController;
  /** 动画链（掷骰选下一段、转向、随机动作、走路计划） */
  private readonly chain: AnimationChain;
  /** 物理参数（配置注入） */
  private physics: PhysicsParams;

  // ---- 几何与位置状态 ----
  /** Rust 回传的窗口内容区左上角（屏幕坐标） */
  private windowOrigin: Vec2 = { x: 0, y: 0 };
  /** 窗口内容区尺寸（物理像素） */
  private windowSize: { width: number; height: number };
  /**
   * 宠物包围盒相对窗口内容区左上角的偏移。
   *
   * 常态就是 `windowMargin(size)`（本项目 `WINDOW_MARGIN_RATIO = 0`，所以是 0）。
   * 早期菜单靠"临时外扩宠物窗"腾地方，这个偏移会在开合菜单时变化；
   * **现在菜单是独立小窗**（`menu_window.rs`），宠物窗从头到尾不改几何，偏移恒为常态值。
   */
  private boxOffset: Vec2 = { x: 0, y: 0 };
  /** 宠物包围盒左上角（屏幕坐标）——物理运算的唯一位置量 */
  private boxOrigin: Vec2 = { x: 0, y: 0 };
  /** 显示器工作区列表（Rust 事件推送；决定抛掷边界与跨屏放行） */
  private areas: Rect[] = [];
  /** 显示器完整面板列表（含任务栏，与 areas 同序）：屏缝处的邻屏探测用它 */
  private panels: Rect[] = [];
  /** 主屏工作区（角落定位用） */
  private primary: Rect | null = null;

  // ---- 输入状态 ----
  /** 最近一次光标采样的屏幕坐标 */
  private cursorScreen: Vec2 | null = null;
  /** 光标是否落在身体命中区内（本模块的判定结果） */
  private cursorInsideBody = false;
  /** 是否已把"可交互"状态上报给 Rust（幂等：值未变不重复 invoke） */
  private reportedInteractive = false;
  /**
   * 是否正在飞行。
   *
   * **刻意做成"从动画器推导"的只读属性，而不是自己维护一个布尔量**（这里踩过坑）：
   * 早先有一个 `flying` 字段 + `startThrow` 里的"已在飞行就跳过"守卫，而"打断飞行"
   * 走的是 `ThrowAnimator.stop()`——它按设计**不回调 `onRest`**，于是那个字段漏了复位，
   * 守卫从此永远为真：**只有第一次能甩出去，之后怎么拖都不飞**（用户实测）。
   * 唯一真相放在 `ThrowAnimator` 里（它的 rAF 句柄在 start/stop/atRest 三处都被正确维护），
   * 就不存在"两处状态不同步"这种可能了。
   */
  private get flying(): boolean {
    return this.thrower.running;
  }
  /** 是否已经记录过"首次命中"日志（避免每帧刷屏，同时证明命中判定确实生效） */
  private loggedFirstHit = false;
  /** 是否已经记录过"初始窗口位置"日志（同上，只记一次） */
  private loggedWindowState = false;
  /**
   * **对话输入闸门**（宿主下发，见 `contract.ts::PetRuntime.chatInputHeld`）。
   *
   * 对话输入条正在占用鼠标时，本页必须**整段让开**：不做命中判定、不翻可交互、
   * 更不能用光标采样起手——否则用户拖动输入条经过宠物时，宠物会把这次拖动
   * 当成"抓住我"跟着一起走（用户实测反馈的 bug）。
   *
   * 这里只存"宿主最近一次告诉我的值"：真正的真相在宿主（`chat_input_until` 会到期自愈），
   * 本页每帧重新读，不做任何自己的计时。
   */
  private chatInputHeld = false;
  /**
   * 自测模式（`?autotest=N`）：开启后**按键位改由自测状态机提供**（`syntheticPrimaryDown`），
   * 光标位置仍来自宿主的真实采样。
   *
   * 为什么需要它：宿主每 16ms 推一帧真实采样，而自测运行期间真实按键是松开的——
   * 如果两条来源各自携带按键位，自测刚按下就会被真实帧立刻收尾。
   * 覆盖之后仍然是**同一条代码路径**（`onCursorFrame`），只是状态来源换成自测。
   */
  private syntheticControl = false;
  /** 自测提供的按键位 */
  private syntheticPrimaryDown = false;
  /** 排障探针剩余帧数（> 0 时按间隔记录采样/几何/命中三元组，见 onCursorFrame） */
  private probeFramesLeft = 0;
  /**
   * 最近一次收到 **DOM 指针事件**（按下/移动/抬起，或任何 `buttons` 非 0 的事件）的时刻。
   *
   * 用途：看门狗收尾要求"按键位报松开"与"DOM 事件已静默"同时成立——
   * 只有 DOM 真静默才说明窗口确实收不到事件了（理由见 DOG_SILENCE_MS）。
   */
  private lastDomPointerAt = 0;
  /**
   * 主键"报松开"的起始时刻（`0` = 最近一次采样报的是按下）。
   *
   * 与 `lastDomPointerAt` 一起构成看门狗的双条件：两者都必须比"本次按下"更早，
   * 才能断定"用户在按下之前就已经松开了"——这条判据专门用来消除
   * "按键位误报松开"导致的按下即被收尾（实测过的事故）。
   */
  private buttonUpSince = 0;
  /** 本次按下开始的时刻（看门狗用它排除"按下之前的陈旧状态"） */
  private pressStartedAt = 0;
  /**
   * 累计收到的窗口位置回传次数。
   *
   * 用途：**逐帧窗口跟随的客观证据**。每一次 `set_pet_bounds` 都会让 Rust 回传一次
   * `pet://window-state`，因此这个计数就是"窗口真的被移动了多少次"。
   * 拖拽/飞行期间的增量必须与帧数同量级——如果只是"结束时跳一下"，这个数会停在个位数。
   */
  private windowStateEvents = 0;
  /** 首次收到的窗口位置回传时间（用于算"回传频率"） */
  private firstWindowStateAt = 0;

  // ---- 动画链与走路状态 ----
  /** 正在进行的行走计划（null = 没在走） */
  private walk: { plan: WalkPlan; startedAt: number; durationMs: number } | null = null;
  /** 是否已经记录过首次走路日志（避免每帧刷屏） */
  private loggedFirstWalk = false;
  /**
   * 主循环 rAF 句柄。
   *
   * 为什么把"命中判定 + 走路推进"放在 rAF 循环里，而不是等光标采样回调触发：
   *   采样回调依赖宿主线程按时推送（实测本机的推送在页面忙碌时会顿一下），
   *   而命中判定决定"窗口此刻吃不吃点击"——它必须**每帧都新鲜**，
   *   否则光标已经离开宠物，窗口还在吃点击，用户就会觉得"宠物把点击挡住了"。
   *   rAF 与显示刷新同频，是这里最可靠的节拍。
   */

  constructor(
    config: PetConfig,
    private readonly dom: PetDom,
    private readonly channels: { cursor: CursorChannel },
  ) {
    this.petConfig = config;
    this.physics = config.physics ?? DEFAULT_PHYSICS;
    this.hitBox = scaleHitBox(config.size, config.aspectRatio);
    this.windowSize = {
      width: config.size + windowMargin(config.size) * 2,
      height: Math.round(config.size * config.aspectRatio) + windowMargin(config.size) * 2,
    };
    // 宠物在窗口内的偏移（= 外扩余量；本项目为 0，且不再随菜单变化）
    this.boxOffset = { x: windowMargin(config.size), y: windowMargin(config.size) };
    this.media = new MediaBuffer(
      dom.videoA,
      dom.videoB,
      // 单次播放结束 → 交给动画链选下一段（"永不停止的动画链"的驱动点）
      () => void this.handleAnimationEnded(),
      (message) => petLog(message),
    );
    this.squash = new SquashAnimator(dom.box);
    this.thrower = new ThrowAnimator();
    this.thrower.setPhysics(this.physics);
    this.chain = new AnimationChain(config.animations, config.animationWeights, {
      play: (animation, mode) => this.playAnimation(animation, mode),
      onWalk: (plan) => this.startWalk(plan),
      onFacingChange: (facing) => this.applyFacing(facing),
      planWalk: (dir, spec) => this.planWalk(dir, spec),
    });
    this.chain.setAvailableAnimations(config.availableAnimations);
    this.drag = new DragController({
      onDragMove: (target) => this.moveWindowToBox(target),
      onRelease: (result) => this.handleRelease(result.box, result.velocity),
      onClick: () => this.handleClick(),
      onDragStateChange: (dragging) => {
        this.dom.hit.classList.toggle('is-dragging', dragging);
        petLog(`拖拽: ${dragging ? '开始（已打断飞行与 Q 弹）' : '结束'}`);
        // 拖拽开始：打断飞行与 Q 弹，并把 inputBusy 拉高（Rust 侧在此期间绝不翻回穿透）
        if (dragging) {
          this.interruptFlight();
        }
        void this.reportBusy(dragging);
      },
    });
    this.drag.setPhysics(this.physics);
  }

  /**
   * 打断飞行（用户重新抓住宠物 / 开始一次新拖拽时调用）。
   *
   * 只做两件事：停掉飞行的 rAF 与 Q 弹。**不需要复位任何"是否在飞"的标志**——
   * 那个状态现在是 `flying` 只读属性，直接读 `ThrowAnimator.running`（见字段注释）。
   */
  private interruptFlight(): void {
    this.thrower.stop();
    this.squash.cancel();
  }

  // ---------------------------------------------------------------------------
  // 启动
  // ---------------------------------------------------------------------------

  /** 启动运行时：布局 → 绑定输入 → 开始接收光标采样 → 启动动画链与主循环 */
  async start(): Promise<void> {
    this.layout();
    this.bindDomInput();
    await this.channels.cursor.start();
    // 主循环：命中判定 + 走路推进（每帧，见 frameTick 的注释）
    requestAnimationFrame(this.frameTick);
    // 动画链启动：先播一段待机，之后播完自动接下一段
    await this.chain.start();
    // 缩放/几何自检（一次性）：记录 WebView 的缩放因子与窗口内布局度量。
    // 这是"DPI 链路是否正确"的唯一现场证据——Rust 按物理像素建窗并按 1:1 假设，
    // 若 WebView 的 DPR 不是 1，CSS 布局会整体缩放，宠物在屏幕上的实际尺寸就与配置不符。
    this.logScaleDiagnostics();
  }

  /**
   * 主循环（每帧）：
   *   1. 推进走路（如果有行走计划）；
   *   2. 命中判定 → 决定窗口吃不吃点击；
   *   3. 看门狗收尾（按键已松开且 DOM 静默）。
   *
   * 把这三件事放在同一帧里做，是为了让"窗口是否吃点击"与"宠物在哪"永远一致——
   * 分两处驱动时曾出现过"光标已离开、窗口还在吃点击"的窗口期。
   */
  private frameTick = (): void => {
    if (this.disposed) return;
    this.updateWalk(performance.now());
    this.evaluateInput();
    if (!this.disposed) requestAnimationFrame(this.frameTick);
  };

  /** 是否已释放（rAF 循环的退出条件；dispose 后不再排队） */
  private disposed = false;

  /** 命中判定 + 看门狗（每帧一次，逻辑与原来一致，只是驱动源换成了 rAF） */
  private evaluateInput(): void {
    const cursor = this.cursorScreen;
    if (!cursor) return;
    const primaryDown = this.syntheticInputActive() ? this.syntheticPrimaryDown : this.lastSampledPrimaryDown;
    const local = screenToLocal(cursor, this.boxOrigin);

    // **对话输入闸门**：输入条在占用鼠标时，本窗口整段让开。
    //
    // 这里必须"整段跳过"而不是只把命中结果按成 false：`evaluateInput` 除了翻
    // 可交互之外，还会在 `primaryDown && insideBody && !wasInteractive` 时
    // **由光标采样起手一次按下**（`origin='sampler'`）。那条路绕过了窗口样式，
    // 只要放行，宠物就会在用户拖动输入条经过它时被拎起来（用户实测的 bug：
    // "拖动输入框到人物上时会带动人物一起移动"）。
    //
    // 已按下的拖拽仍然交给下面的看门狗收尾（用户在拖宠物的中途去按了输入条，
    // 松手一样要能正常落地），所以这里 return 的位置在按键位计算之后。
    if (this.chatInputHeld) {
      if (this.drag.isPressed) {
        const now = performance.now();
        const dogReady =
          !primaryDown &&
          this.buttonUpSince > 0 &&
          this.buttonUpSince < this.pressStartedAt &&
          this.lastDomPointerAt < this.pressStartedAt &&
          now - this.lastDomPointerAt >= DOG_SILENCE_MS;
        if (dogReady) {
          petLog('松手: 看门狗收尾（对话输入闸门期间，按键与 DOM 均已静默）');
          this.drag.onPointerUp();
        }
      }
      return;
    }

    // 命中判定：只有身体命中区可交互。
    // （菜单已改成**独立小窗**，不再需要"菜单打开时整窗放行"这个例外。）
    const insideBody = isInsideHitBox(this.hitBox, local.x, local.y);

    // 排障探针：`?autotest=3` 时按固定间隔记录"采样坐标 / 窗口原点 / 命中判定"三元组
    if (this.probeFramesLeft > 0) {
      this.probeFramesLeft -= 1;
      if (this.probeFramesLeft % 6 === 0) {
        petLog(
          `探针: 采样=(${cursor.x.toFixed(0)},${cursor.y.toFixed(0)}) ` +
            `包围盒=(${this.boxOrigin.x.toFixed(0)},${this.boxOrigin.y.toFixed(0)}) ` +
            `盒内=(${local.x.toFixed(0)},${local.y.toFixed(0)}) 在命中框内=${insideBody} ` +
            `按键=${primaryDown} 按下中=${this.drag.isPressed} 可交互=${this.reportedInteractive}`,
        );
      }
    }

    if (this.drag.isPressed) {
      // 看门狗：三个条件同时成立才收尾（理由见 DOG_SILENCE_MS）
      const now = performance.now();
      const dogReady =
        !primaryDown &&
        this.buttonUpSince > 0 &&
        this.buttonUpSince < this.pressStartedAt &&
        this.lastDomPointerAt < this.pressStartedAt &&
        now - this.lastDomPointerAt >= DOG_SILENCE_MS;
      if (dogReady) {
        petLog(
          `松手: 看门狗收尾（按键松开于 ${(this.pressStartedAt - this.buttonUpSince).toFixed(0)}ms 前、` +
            `DOM 静默 ${(now - this.lastDomPointerAt).toFixed(0)}ms，均早于本次按下）`,
        );
        this.drag.onPointerUp();
      }
      return;
    }

    // 未按下：命中判定决定穿透/可交互
    if (insideBody !== this.cursorInsideBody) {
      this.cursorInsideBody = insideBody;
      if (insideBody && !this.loggedFirstHit) {
        this.loggedFirstHit = true;
        petLog(
          `命中: 首次进入身体 光标=(${cursor.x.toFixed(0)},${cursor.y.toFixed(0)}) ` +
            `包围盒=(${this.boxOrigin.x.toFixed(0)},${this.boxOrigin.y.toFixed(0)})`,
        );
      }
    }
    // 这一帧**开始**时窗口的实际交互状态。必须在 `setInteractive` 之前取，
    // 因为它会就地改写 `reportedInteractive`（见下面的兜底起手）。
    const wasInteractive = this.reportedInteractive;
    void this.setInteractive(insideBody);

    // 兜底起手：判据是"**这一帧开始时**窗口还是穿透态"。
    //
    // 语义：穿透态下 DOM 事件到不了页面，所以这次按下只可能来自采样通道 → 由采样起手；
    // 反之，若开始时已经可交互，按下就该走 DOM（零延迟、坐标最准），这里绝不能再插一脚
    //（两边都起手会把抓取偏移重算一遍，宠物跳一下；DOM 那边已有 `drag.isPressed` 去重）。
    //
    // 这里曾经写成 `!this.reportedInteractive`——读的是 `setInteractive` **之后**的值，
    // 于是条件恒为假、整个分支是死代码：**穿透态下的第一次按下会被无声吞掉**
    //（用户感受就是"点了没反应，得再点一次"），`?autotest=1/2/10` 这些合成自测也因此全线失效。
    if (primaryDown && insideBody && !wasInteractive) {
      this.beginPress(cursor, 'sampler');
    }
  }

  /**
   * 记录缩放与布局度量（只做一次）。
   *
   * 判读方法：
   *   - `devicePixelRatio` 应为 1（Rust 建窗用的是物理像素值，前提是 1 逻辑像素 = 1 物理像素）；
   *   - `窗口CSS` 应等于配置的窗口尺寸（`size + 2×margin`）；
   *   - `stageCSS` 应等于 `size × size*9/16`。
   *   三者一致 → 宠物在屏幕上的实际像素尺寸与配置严格相等。
   */
  private logScaleDiagnostics(): void {
    const css = this.dom.box.getBoundingClientRect();
    petLog(
      `缩放: devicePixelRatio=${window.devicePixelRatio} ` +
        `innerCSS=${window.innerWidth}x${window.innerHeight} ` +
        `窗口CSS=${this.windowSize.width}x${this.windowSize.height} ` +
        `包围盒CSS=${css.width.toFixed(1)}x${css.height.toFixed(1)} ` +
        `命中区CSS=${this.dom.hit.getBoundingClientRect().width.toFixed(1)}x${this.dom.hit.getBoundingClientRect().height.toFixed(1)}`,
    );
  }

  /** 按配置把宠物摆到窗口内（窗口本身的位置由 Rust 负责） */
  private layout(): void {
    const { size, aspectRatio } = this.petConfig;
    this.dom.box.style.width = `${size}px`;
    this.dom.box.style.height = `${Math.round(size * aspectRatio)}px`;
    // 宠物在窗口内的位置 = **当前**包围盒偏移。常态下 `WINDOW_MARGIN_RATIO = 0` 所以是 0；
    // 菜单外扩期间它会变大——窗口向左上长大的同时宠物在窗口内向右下挪同样的量，
    // 于是宠物在屏幕上原地不动（这条不变量见 renderer/menu.ts 的说明）。
    // 早期这里写死 `windowMargin(size)`，等于让 DOM 忽略偏移：窗口一长大宠物就跟着跑。
    this.dom.box.style.left = `${this.boxOffset.x}px`;
    this.dom.box.style.top = `${this.boxOffset.y}px`;
    applyHitBoxStyle(this.dom.hit, this.hitBox, size, aspectRatio);
  }

  /**
   * 绑定 DOM 输入。
   *
   * **坐标来源的第一原则：跟手/拖拽一律用 DOM 事件自带的坐标**：
   * 它天然与命中判定处于同一坐标系（窗口内容区），也是鼠标按下那一刻的真实位置，
   * 而且**零延迟**——采样通道要经过"宿主轮询 → IPC → 页面"三段，跟手会明显发飘。
   *
   * 关于采样坐标的一次误判（留作记录）：早期这里写着"`AppHandle::cursor_position()`
   * 在本机不可信"，依据是"同一次按下里 DOM 坐标 `(2320,226)`、采样 `(1188,181)`、
   * 真实光标 `(1000,1200)` 三个值互不相同"。后来用 `WHALE_PET_DIAG_CURSOR=1`
   * 把"宿主读到的值"与"页面收到的值"逐帧对照，发现**两边完全一致**，
   * 且与进程外直接 `GetCursorPos()` 也一致——真正不成立的是那个前提：
   * 本机的指针被桌面环境持续驱动，`SetCursorPos` 设过去的位置**几百毫秒内就会被挪走**
   * （实测：设定 `(300,300)`，250ms 后读回 `(1844,627)`）。
   * 也就是说"真实光标位置"从来不是那个假定值，采样一直是准的。
   *
   * 采样通道的职责因此不变（但理由要写对）：
   *   1. **按键看门狗**——收尾那些永远到不了页面的 `pointerup`（见 onCursorFrame 规则 1）；
   *   2. **穿透期间的兜底位置**——窗口处于穿透态时页面收不到任何鼠标事件，
   *      而"该不该翻回可交互"只能靠全局光标位置判断。
   */
  private bindDomInput(): void {
    this.dom.hit.addEventListener('pointerdown', (event) => {
      if (event.button !== 0) return; // 只处理左键；右键留给后续的菜单里程碑
      event.preventDefault();
      // **必须先刷新 DOM 事件时刻**：看门狗凭它判断"DOM 是否仍在投递事件"。
      // 漏掉这一行会导致按下瞬间用上一次的陈旧时刻（实测 743ms 前）去做判断，
      // 于是刚按下就被收尾——这正是"第一次拖拽异常、之后正常"的由来。
      this.lastDomPointerAt = performance.now();
      if (this.drag.isPressed) return; // 采样兜底可能已经先按下（去重）
      // 抓取点用 DOM 推出的屏幕坐标：与命中判定同一坐标系，按下瞬间不会算错偏移
      const screen = this.domScreenPoint(event);
      petLog(
        `按下: 来源=dom client=(${event.clientX.toFixed(0)},${event.clientY.toFixed(0)}) ` +
          `→屏幕=(${screen.x.toFixed(0)},${screen.y.toFixed(0)}) ` +
          `窗口原点=(${this.windowOrigin.x.toFixed(0)},${this.windowOrigin.y.toFixed(0)}) ` +
          `包围盒=(${this.boxOrigin.x.toFixed(0)},${this.boxOrigin.y.toFixed(0)})`,
      );
      this.beginPress(screen, 'dom', event.pointerId);
    });
    // 指针移动：窗口处于可交互态时这是**最高频、最可信**的位置来源
    this.dom.hit.addEventListener('pointermove', (event) => this.feedDomPointer(event));
    window.addEventListener('pointermove', (event) => this.feedDomPointer(event));
    // 指针抬起：优先用 DOM；窗口已回到穿透态时该事件不会到达，
    // 由采样通道的"按键看门狗"收尾（见 onCursorFrame 规则 1）。
    window.addEventListener('pointerup', (event) => this.endPressFromDom(event));
    // 指针被系统取消（触摸被手势接管等）同样要收尾
    window.addEventListener('pointercancel', () => {
      if (this.drag.isPressed) this.drag.onPointerUp();
    });
    // 右键：打开级联菜单。
    //
    // 挂在 window 上而不是命中区上：菜单本身也在这个窗口里，右键点菜单项不应该再弹一层菜单；
    // `#pet-hit` 只覆盖身体命中区，而菜单区域在命中区之外——所以两者都要拦住。
    window.addEventListener('contextmenu', (event) => {
      event.preventDefault();
      // 传**屏幕坐标**而不是窗口内坐标：菜单打开期间窗口是外扩过的，
      // 这时候量到的 clientX/Y 属于"外扩帧"；菜单控制器会先关掉旧菜单、等窗口缩回，
      // 再按基准帧换算——用事件里的窗口内坐标会在第二次右键时整体偏一个外扩量（实测 bug）。
      const screen = this.domScreenPoint(event);
      petLog(
        `菜单: 右键触发 at 窗口内 (${event.clientX.toFixed(0)},${event.clientY.toFixed(0)})` +
          ` →屏幕 (${screen.x.toFixed(0)},${screen.y.toFixed(0)})`,
      );
      void this.openContextMenu(screen);
    });
    // 窗口失焦（Alt+Tab 切走等）也可能丢失 pointerup：按"放下"处理。
    window.addEventListener('blur', () => {
      if (this.drag.isPressed) this.drag.onPointerUp();
    });
  }

  /** DOM 坐标 → 屏幕坐标（窗口内容区原点 + 事件的 client 坐标） */
  private domScreenPoint(event: { clientX: number; clientY: number }): Vec2 {
    return { x: this.windowOrigin.x + event.clientX, y: this.windowOrigin.y + event.clientY };
  }

  /** 吃一帧 DOM 指针移动（仅在已按下时处理；pointerId 校验防止多指针串台） */
  private feedDomPointer(event: PointerEvent): void {
    // 无论是否已按下都记时间：看门狗用它判断"DOM 事件是否已经静默"
    this.lastDomPointerAt = performance.now();
    if (!this.drag.isPressed) return;
    if (!this.drag.matchesPointer(event.pointerId)) return;
    this.drag.onPointerMove(this.domScreenPoint(event));
  }

  /** 用 DOM 的 pointerup 收尾（校验 pointerId，避免被别的指针误触发） */
  private endPressFromDom(event: PointerEvent): void {
    this.lastDomPointerAt = performance.now();
    if (event.button !== 0) return;
    if (!this.drag.isPressed) return;
    if (!this.drag.matchesPointer(event.pointerId)) return;
    this.drag.onPointerUp();
  }

  /**
   * 开始一次按压（DOM 与采样兜底共用）。
   *
   * @param cursor 指针屏幕坐标
   * @param origin 来源；`sampler` 表示 DOM 事件没到（例如穿透期间按下）
   * @param pointerId DOM 指针 id（采样兜底时传 -1：不绑定具体指针）
   */
  private beginPress(cursor: Vec2, origin: PressOrigin, pointerId = -1): void {
    // 按下即打断飞行与 Q 弹（onDragStateChange 里也会做，这里覆盖"按下但未成拖拽"）
    this.interruptFlight();
    this.pressStartedAt = performance.now();
    // 指针捕获：把后续指针事件**锁定在本窗口**，即使光标滑出窗口边界也照样收到。
    // 这是"拖拽中光标离开命中区 → 窗口翻回穿透 → pointerup 丢失"那条老问题的正道解法：
    // 捕获期间 DOM 数据流不会中断，因此不需要依赖任何全局输入查询。
    if (origin === 'dom' && pointerId >= 0) {
      try {
        this.dom.hit.setPointerCapture(pointerId);
      } catch {
        // 捕获失败不影响主流程（采样与兜底仍在）；最坏情况退化为"需要光标留在窗口内"
      }
    }
    this.drag.onPointerDown(cursor, this.boxOrigin, origin, pointerId);
    petLog(
      `按下: 来源=${origin} 指针=(${cursor.x.toFixed(0)},${cursor.y.toFixed(0)}) ` +
        `包围盒=(${this.boxOrigin.x.toFixed(0)},${this.boxOrigin.y.toFixed(0)})`,
    );
  }

  // ---------------------------------------------------------------------------
  // 状态同步（由 Rust 侧事件驱动）
  // ---------------------------------------------------------------------------

  /**
   * Rust 回传的窗口位置/尺寸发生变化。
   *
   * 这是**位置的唯一真相**：前端所有依赖位置的计算（命中判定、窗口移动增量）都从这里出发。
   */
  setWindowState(state: PetRuntimeState): void {
    this.windowStateEvents += 1;
    if (this.firstWindowStateAt === 0) this.firstWindowStateAt = performance.now();
    this.windowOrigin = { ...state.windowOrigin };
    this.windowSize = { ...state.windowSize };
    // 包围盒原点 = 窗口原点 + 状态里给的偏移（偏移恒为外扩余量；菜单不再改它）
    this.boxOrigin = {
      x: this.windowOrigin.x + state.boxOffset.x,
      y: this.windowOrigin.y + state.boxOffset.y,
    };
    this.boxOffset = { ...state.boxOffset };
    // 对话输入闸门：宿主那边的到期时间是唯一真相，这里只做镜像（见字段注释）
    const held = state.chatInputHeld === true;
    if (held !== this.chatInputHeld) {
      this.chatInputHeld = held;
      petLog(`对话输入闸门: ${held ? '宠物让开鼠标（输入条正在使用中）' : '已恢复（命中判定重新生效）'}`);
      // 闸门打开时窗口已被宿主落成穿透；收闸后这一帧的命中判定会自己把它翻回来，
      // 不需要在这里额外做任何窗口操作。
    }
    if (!this.loggedWindowState) {
      // 只记首次：证明"Rust 建窗 → 位置下发 → 前端换算包围盒"这条链路通了
      this.loggedWindowState = true;
      // **对齐宿主的"可交互"记账**（只做这一次，之后由本页的命中判定接管）。
      // 窗口是"可交互"创建的，而本页的 `reportedInteractive` 从 false 起步——
      // 不对齐的话，第一帧算出的 `setInteractive(false)` 会被去重吞掉，
      // 窗口就一直吃点击（连包围盒里的透明区域一起吃），直到用户第一次悬停宠物。
      this.reportedInteractive = state.interactive;
      petLog(
        `窗口: 初始 原点=(${this.windowOrigin.x.toFixed(0)},${this.windowOrigin.y.toFixed(0)}) ` +
          `尺寸=${this.windowSize.width.toFixed(0)}x${this.windowSize.height.toFixed(0)} ` +
          `→ 包围盒=(${this.boxOrigin.x.toFixed(0)},${this.boxOrigin.y.toFixed(0)}) ` +
          `宿主可交互=${state.interactive}`,
      );
    }
    // 非拖拽期间同步给弹簧，保证下次按下时抓取偏移正确
    this.drag.syncBox(this.boxOrigin);
  }

  /**
   * 光标采样帧：**只记录状态**（位置 + 按键位），不做任何判定。
   *
   * 判定统一放在主循环 `frameTick` 里做（见 `frameTick` 的注释）：采样在页面忙碌时
   * 可能顿一下，而"窗口此刻吃不吃点击"必须每帧新鲜。
   *
   * 采样通道的职责：
   *   1. 提供**全局按键状态**→ 看门狗（收尾那些永远到不了页面的 `pointerup`）；
   *   2. 提供**穿透期间的兜底位置**（此时页面收不到任何 DOM 事件）；
   *   3. 提供宿主的光标位置供命中判定使用——**跟手不用它**（实测坐标是准的，
   *      但要走"轮询 → IPC → 页面"三段，跟手会明显发飘，DOM 坐标零延迟）。
   */
  onCursorFrame(frame: CursorFrame): void {
    // 自测期间自测负责喂位置（否则真实采样会把合成轨迹冲掉）——只取按键位
    if (!this.syntheticInputActive()) this.cursorScreen = { x: frame.x, y: frame.y };
    this.lastSampledPrimaryDown = frame.primaryDown;
    // 记录"按键位报松开"的起始时刻（看门狗用它排除按下之前的陈旧状态）
    if (frame.primaryDown) {
      this.buttonUpSince = 0;
    } else if (this.buttonUpSince === 0) {
      this.buttonUpSince = performance.now();
    }
    // 采样兜底按下的跟手（DOM 来源的按下由 pointermove 驱动，不用采样坐标）
    const cursor = this.cursorScreen;
    if (cursor && this.drag.isPressed && this.drag.origin === 'sampler') {
      // 牵引绳：偏离按下点过远就不再跟随——正常拖拽用不到，失同步时也只是"拖不动"
      if (this.drag.leashDistance(cursor) <= MAX_DRAG_LEASH) {
        this.drag.onPointerMove(cursor);
      }
    }
  }

  /** 最近一次采样里宿主的按键状态（看门狗与兜底起手用它） */
  private lastSampledPrimaryDown = false;

  /**
   * 自测是否正在接管输入（合成光标 + 合成按键位）。
   *
   * 三种自测模式都算：`?autotest=1/2`（合成轨迹驱动状态机）与 `?autotest=4`
   * （让状态机认为按键一直按着，供宿主注入的合成拖拽跑完）。
   * `?autotest=3` 只是探针，用真实光标，所以不算接管。
   */
  private syntheticInputActive(): boolean {
    return this.syntheticControl;
  }

  /** 上报"可交互"状态（幂等） */
  private async setInteractive(interactive: boolean): Promise<void> {
    if (interactive === this.reportedInteractive) return;
    this.reportedInteractive = interactive;
    petLog(`交互: 窗口切换为 ${interactive ? '可交互' : '点击穿透'}`);
    try {
      await invoke('set_pet_interactive', { label: this.petConfig.label, interactive });
    } catch (err) {
      petLogError('交互: 切换失败', err);
    }
  }

  /**
   * 上报 inputBusy（"前端正在使用本窗口的鼠标输入"）。
   *
   * Rust 侧的兜底命中通道在 busy 期间不做任何穿透翻转：
   * 拖拽时宠物由弹簧追赶光标、**滞后**于光标，光标可能已经跑到包围盒之外，
   * 若按几何盲判就会误判成"用户离开了"而翻回穿透，拖拽当场断掉。
   */
  private async reportBusy(busy: boolean): Promise<void> {
    petLog(`输入占用: ${busy ? '开始（Rust 侧停止兜底翻转）' : '结束（恢复兜底翻转）'}`);
    try {
      await invoke('set_pet_input_busy', { label: this.petConfig.label, busy });
    } catch (err) {
      petLogError('输入占用: 上报失败', err);
    }
  }

  // ---------------------------------------------------------------------------
  // 动画（动画链的落地实现）
  // ---------------------------------------------------------------------------

  /** 资源 URL 拼装（`${assetBaseUrl}/webm/<名称>.webm`）。名称是**基名**，不带扩展名。 */
  private animationUrl(name: string): string {
    return `${this.petConfig.assetBaseUrl}/webm/${encodeURIComponent(name)}.webm`;
  }

  /**
   * 播放一段动画（动画链的 `play` 回调）。
   *
   * **全部按单次播**（包括待机）：`ended` 事件只在非循环时触发，而动画链的推进
   * 正是靠它——"循环感"来自链反复抽到待机，而不是让 video 自己 loop
   * （上游浏览器端就是同一做法）。
   *
   * 同时起一个**看门狗定时器**兜底：万一个别素材不触发 `ended`
   * （时长元数据异常 / 解码器边界情况），也保证动画链不会永久停在那一段。
   */
  private async playAnimation(name: string, mode: 'loop' | 'once'): Promise<void> {
    this.playGeneration += 1;
    const generation = this.playGeneration;
    if (this.playWatchdog) {
      window.clearTimeout(this.playWatchdog);
      this.playWatchdog = 0;
    }
    await this.media.switchTo(this.animationUrl(name), mode);
    const durationSec = this.media.currentDurationSec;
    if (durationSec > 0) {
      // 多给 800ms 余量：`ended` 通常先到，本定时器只是保险
      this.playWatchdog = window.setTimeout(() => {
        if (generation !== this.playGeneration) return;
        petLog(`动画链: 看门狗兜底推进（${name} 未触发 ended，时长 ${durationSec.toFixed(1)}s）`);
        void this.handleAnimationEnded();
      }, durationSec * 1000 + 800);
    }
  }

  /** 播放世代号：用于让过期的看门狗定时器失效 */
  private playGeneration = 0;
  /** 播放看门狗定时器句柄（0 = 未起） */
  private playWatchdog = 0;

  /**
   * 一段动画播完（媒体层回调）。
   *
   * 两条分支：
   *   - 若这段是"点击回应"这类**由交互触发**的动画（调用方已让链暂停），
   *     播完就回待机、不推进随机链——否则用户点一下会导致宠物接着做一串动作；
   *   - 否则交给动画链正常推进（掷骰选下一段）。
   */
  private async handleAnimationEnded(): Promise<void> {
    if (this.playWatchdog) {
      window.clearTimeout(this.playWatchdog);
      this.playWatchdog = 0;
    }
    if (this.chainPausedByInteraction) {
      this.chainPausedByInteraction = false;
      this.chain.setPaused(false);
      return;
    }
    // 转向动画（animations.turn）播完要翻转朝向：这是它的核心副作用，
    // 漏掉这一步的表现是"宠物会转向的动作播了，但朝向始终没变"。
    this.chain.applyTurnFlipIfPending();
    await this.chain.onAnimationEnded(this.chain.currentAnimation);
  }

  /** 是否因为"点击回应动画"而临时暂停了动画链 */
  private chainPausedByInteraction = false;

  /** 播放点击回应动画（随机抽 1）+ Q 弹；无该动画时只做 Q 弹 */
  private async playClickReaction(): Promise<void> {
    const clicks = this.petConfig.animations.clicks;
    this.squash.start();
    if (clicks.length === 0) return;
    const name = clicks[Math.floor(Math.random() * clicks.length)];
    // 暂停动画链：点击回应属于"交互触发"，播完回待机而不是继续推进随机链
    this.chain.setPaused(true);
    this.chainPausedByInteraction = true;
    await this.playAnimation(name, 'once');
  }

  // ---------------------------------------------------------------------------
  // 屏幕漫游（walk）
  // ---------------------------------------------------------------------------

  /**
   * 规划一次行走（动画链的 `planWalk` 回调）。
   *
   * 复用 `@shared/motion.ts` 的 `planMove`：它按**所在屏的工作区**判定落点可达性
   * （墙外/空洞里走不过去），并按"身体贴边"语义留边距——与本项目
   * "边界用并集而不是外接矩形"的既有约定一致。
   *
   * 坐标系换算：`planMove` 用**所属屏的局部坐标系**（原点 = 该屏工作区左上角），
   * 因此这里把宠物包围盒中心换算进去，再把结果换算回屏幕坐标。
   */
  private planWalk(dir: 1 | -1, spec: { minDist: number; maxDist: number; margin: number }): WalkPlan | null {
    const size = this.petConfig.size;
    const height = size * this.petConfig.aspectRatio;
    const center: Vec2 = { x: this.boxOrigin.x + size / 2, y: this.boxOrigin.y + height / 2 };
    const area = this.areaContaining(center) ?? this.primaryArea;

    const plan = planMove({
      cx: center.x - area.x,
      cy: center.y - area.y,
      W: area.width,
      H: area.height,
      dir,
      minDist: spec.minDist,
      maxDist: spec.maxDist,
      margin: spec.margin,
      halfW: size / 2,
      sideAllow: 0,
    });
    if (!plan) return null;
    return {
      animation: '',
      leadSec: 0,
      tailSec: 0,
      fromX: area.x + plan.startRatio * area.width,
      toX: area.x + plan.targetRatio * area.width,
      centerY: center.y,
      dir,
    };
  }

  /** 找到包含该点的显示器工作区（找不到返回 null：点在空洞里或屏外） */
  private areaContaining(point: Vec2): Rect | null {
    return (
      this.areas.find(
        (a) => point.x >= a.x && point.x < a.x + a.width && point.y >= a.y && point.y < a.y + a.height,
      ) ?? null
    );
  }

  /** 开始一次行走（动画链的 `onWalk` 回调） */
  private startWalk(plan: WalkPlan): void {
    // 走路方向决定朝向（先转身再走，观感更自然）
    this.chain.setFacing(plan.dir === 1 ? 'right' : 'left');
    this.walk = { plan, startedAt: performance.now(), durationMs: this.currentAnimationDurationMs() };
    if (!this.loggedFirstWalk) {
      this.loggedFirstWalk = true;
      petLog(
        `漫游: 首次行走 ${plan.animation} ${plan.fromX.toFixed(0)} → ${plan.toX.toFixed(0)} ` +
          `（${plan.leadSec}s 起 / ${plan.tailSec}s 止，时长 ${(this.currentAnimationDurationMs() / 1000).toFixed(1)}s）`,
      );
    }
  }

  /**
   * 当前动画时长（毫秒）。
   *
   * 走路动画的时长决定位移节奏：`leadSec`/`tailSec` 期间原地不动，中间线性移动。
   * 媒体元数据还没就绪时退化为 4 秒（典型动作长度），避免一次走位瞬间完成。
   */
  private currentAnimationDurationMs(): number {
    const duration = this.media.currentDurationSec;
    return duration > 0 ? duration * 1000 : 4000;
  }

  /** 每帧推进走路（由主循环调用） */
  private updateWalk(now: number): void {
    const walk = this.walk;
    if (!walk) return;
    // 被打断（拖拽/飞行）时放弃本次行走：位置改由输入/物理驱动
    if (this.drag.isPressed || this.flying) {
      this.walk = null;
      return;
    }
    const elapsed = now - walk.startedAt;
    const centerX = walkCenterAt(walk.plan, elapsed, walk.durationMs);
    const nextBox: Vec2 = {
      x: centerX - this.petConfig.size / 2,
      y: walk.plan.centerY - (this.petConfig.size * this.petConfig.aspectRatio) / 2,
    };
    this.setBoxOrigin(nextBox);
    if (elapsed >= walk.durationMs) this.walk = null;
  }

  /**
   * 应用朝向（水平镜像）。
   *
   * 镜像写在**包裹整只宠物的元素**上：视频与命中区一起镜像，命中判定不需要额外的
   * 坐标翻转（命中框水平方向居中，镜像后仍在同一位置）。带文字的动作靠
   * `noMirror` 分类进制来避免左右颠倒，而不是靠不镜像。
   */
  private applyFacing(facing: Facing): void {
    this.dom.box.style.transform = facing === 'right' ? 'scaleX(-1)' : 'none';
    petLog(`朝向: ${facing}${facing === 'right' ? '（镜像）' : ''}`);
  }
  // ---------------------------------------------------------------------------
  // 交互结果
  // ---------------------------------------------------------------------------

  /** 点击（位移未超阈值）：播点击回应 + Q 弹（系统要求减少动效时跳过 Q 弹） */
  private handleClick(): void {
    petLog('点击: 位移未超阈值，按点击处理');
    if (prefersReducedMotion()) {
      void this.playClickReaction().then(() => this.squash.cancel());
      return;
    }
    void this.playClickReaction();
  }

  /**
   * 松手：有初速则甩出去，否则原地放下（不抛）。
   *
   * 初速估算在 DragController 内部完成（复用上游 shared 的纯逻辑），这里做三件事：
   *   1. **落点夹回工作区**（见 `clampBoxToWorkArea`）；
   *   2. 甩出速度打折（见 `RELEASE_SPEED_SCALE`）；
   *   3. 分派：有初速 → 抛掷，无初速 → 原地放下。
   */
  private handleRelease(box: Vec2, velocity: Vec2 | null): void {
    this.cursorInsideBody = false; // 松手后重新判定（指针可能已不在身体上）
    const landBox = this.clampBoxToWorkArea(box);
    if (landBox.x !== box.x || landBox.y !== box.y) {
      petLog(
        `松手: 落点在屏外，夹回工作区 (${box.x.toFixed(0)},${box.y.toFixed(0)})` +
          ` → (${landBox.x.toFixed(0)},${landBox.y.toFixed(0)})`,
      );
    }
    this.setBoxOrigin(landBox);
    if (!velocity) {
      petLog(`松手: 温柔放下 位置=(${landBox.x.toFixed(0)},${landBox.y.toFixed(0)})`);
      return;
    }
    // 甩出强度打折：上游的"死区 + 峰值加权"是按"甩得很凶"的手感调的，
    // 实际用起来偏灵敏（用户反馈"过于灵敏、甩飞出去"）——见 RELEASE_SPEED_SCALE。
    const scaled: Vec2 = {
      x: velocity.x * RELEASE_SPEED_SCALE,
      y: velocity.y * RELEASE_SPEED_SCALE,
    };
    petLog(
      `松手: 甩出 速度=(${velocity.x.toFixed(0)},${velocity.y.toFixed(0)})px/s` +
        ` →折扣后 (${scaled.x.toFixed(0)},${scaled.y.toFixed(0)})px/s ` +
        `位置=(${landBox.x.toFixed(0)},${landBox.y.toFixed(0)})`,
    );
    this.startThrow(landBox, scaled);
  }

  /**
   * 把落点夹回工作区（**只在松手那一瞬间夹一次**，拖拽过程中不夹）。
   *
   * 为什么需要：拖拽本身**不限制越界**——用户可以把宠物拎到屏幕上方去；
   * 而"温柔放下"是按松手位置直接落位的，于是宠物可能停在屏幕外或半挂在边缘，
   * 之后用户**再也抓不到它**（用户实测："停止后就无法操控了"）。
   * 抛掷路径自己有边界反弹，这里补的正是"原地放下"这条路径。
   *
   * 夹取规则与抛掷的边界语义保持一致：左右各允许越出 `sideAllow`（贴边手感），
   * 垂直方向必须完整落在工作区内。
   */
  private clampBoxToWorkArea(box: Vec2): Vec2 {
    const size = this.petConfig.size;
    const height = size * this.petConfig.aspectRatio;
    const sideAllow = size * SIDE_ALLOW_RATIO;
    const center: Vec2 = { x: box.x + size / 2, y: box.y + height / 2 };
    const area = this.areaContaining(center) ?? this.primaryArea;
    const minX = area.x - sideAllow;
    const maxX = Math.max(minX, area.x + area.width - size + sideAllow);
    const minY = area.y;
    const maxY = Math.max(minY, area.y + area.height - height);
    return {
      x: Math.min(Math.max(box.x, minX), maxX),
      y: Math.min(Math.max(box.y, minY), maxY),
    };
  }

  /** 开始一次飞行 */
  private startThrow(box: Vec2, velocity: Vec2): void {
    // 这里**刻意没有"已在飞行就跳过"的守卫**：那样的守卫一旦和"打断飞行"路径不同步，
    // 就会让之后每一次甩出都静默失效（用户实测："只有首次能拖出来甩，后面怎么拖都不飞"）。
    // `ThrowAnimator.start()` 内部本身会先 `stop()`，重复调用是安全且语义正确的
    //（"被重新抓住再甩一次"本来就该以新初速重启）。
    petLog(`飞行: 开始，边界 ${this.knownAreas().length} 块屏`);
    this.thrower.start({
      box,
      velocity,
      size: this.petConfig.size,
      sideAllow: this.petConfig.size * SIDE_ALLOW_RATIO,
      areas: this.knownAreas(),
      panels: this.knownPanels(),
      onFrame: (next) => this.setBoxOrigin(next),
      onRest: (restBox, impactSpeed) => {
        this.setBoxOrigin(restBox);
        petLog(`飞行: 结束于 (${restBox.x.toFixed(0)},${restBox.y.toFixed(0)})，落地冲击 ${impactSpeed.toFixed(0)}px/s`);
        // 落地 Q 弹：冲击速度越大压得越狠（轻落 0.8 ~ 重砸 0.55，规则在 shared 里）
        if (!prefersReducedMotion() && impactSpeed > 0) {
          this.squash.start(squashDepthForImpact(impactSpeed));
        }
      },
    });
  }

  /** 显示器几何变化（分辨率/缩放/插拔/旋转） */
  onDisplays(sample: DisplaysSample): void {
    this.areas = sample.areas;
    this.panels = sample.panels;
    this.primary = sample.primary;
  }

  /**
   * 可用的显示器工作区列表。
   * 首帧（几何事件尚未到达）时退化为一个以当前窗口为中心的大矩形，
   * 保证这一次抛掷不会因为空列表而失去边界。
   */
  private knownAreas(): Rect[] {
    if (this.areas.length > 0) return this.areas;
    const origin = this.windowOrigin;
    return [{ x: origin.x - 2000, y: origin.y - 2000, width: 4000, height: 4000 }];
  }

  /**
   * 可用的完整面板列表（与 knownAreas 同序同长；缺失时退化为工作区，
   * 语义等价于"接缝处没有邻屏"——与上游 `throwSpace` 的兜底一致）。
   */
  private knownPanels(): Rect[] {
    return this.panels.length === this.areas.length && this.panels.length > 0 ? this.panels : this.knownAreas();
  }

  /** 主屏工作区（角落定位用；尚未收到几何时退化为 0 起点的大矩形） */
  get primaryArea(): Rect {
    return this.primary ?? { x: 0, y: 0, width: 1920, height: 1080 };
  }

  /** 把宠物包围盒移到一个新位置（**屏幕坐标**）并驱动窗口跟随 */
  private setBoxOrigin(box: Vec2): void {
    this.boxOrigin = { ...box };
    this.moveWindowToBox(box);
  }

  /**
   * 窗口跟随：把包围盒位置换算成窗口内容区位置后交给 Rust。
   *
   * 换算必须减掉外扩余量（窗口 = 包围盒 + 四周 margin），否则宠物在屏幕上的实际位置
   * 会整体偏移半个身位——这是最典型也最难自查的错位 bug，故只在 coords.ts 里做一次。
   */
  private moveWindowToBox(box: Vec2): void {
    // 用**当前**偏移换算（菜单外扩期间偏移更大；用固定余量会让宠物位置错开）
    const origin: Vec2 = { x: Math.round(box.x) - this.boxOffset.x, y: Math.round(box.y) - this.boxOffset.y };
    void invoke('set_pet_bounds', {
      label: this.petConfig.label,
      x: origin.x,
      y: origin.y,
      width: this.windowSize.width,
      height: this.windowSize.height,
    }).catch((err: unknown) => {
      petLogError('窗口: 移动失败', err);
    });
  }
  /**
   * 打开右键菜单。
   *
   * 菜单是**独立小窗**（`src-tauri/src/menu_window.rs`）：这里只上报"右键点在屏幕坐标系里的
   * 位置"和宠物标签，宿主据此把菜单窗摆到鼠标处（放不下就整体内移，保证不被屏幕裁掉）。
   *
   * 早期版本是"临时扩大宠物窗、把菜单画在宠物窗里"，需要一整套外扩方向/偏移换算，
   * 还带来"重复右键偏移""换几何时人物虚影闪一下"等问题——现在宠物窗从头到尾不改几何，
   * 那一整类问题随之消失。
   */
  async openContextMenu(screen: Vec2): Promise<void> {
    await invoke('show_menu', {
      label: this.petConfig.label,
      x: screen.x,
      y: screen.y,
    }).catch((err: unknown) => petLogError('菜单: 弹出失败', err));
  }

  /**
   * 菜单项被点击（菜单页 → 宿主 → 这里执行）。
   *
   * 动作必须在**宠物页**执行：动画链、包围盒、气泡锚点都在这里，
   * 菜单页只知道"用户点了哪个动作"。
   */
  async onMenuAction(action: { anim: string | null; action: string | null }): Promise<void> {
    if (action.action === 'say-demo') {
      petLog('菜单: 显示测试气泡');
      this.say('气泡独立小窗已生效 ✓ 这条文字不会挡住下层点击', 6000);
      return;
    }
    if (action.action === 'chat') {
      // 对话输入窗由宿主创建/摆位（它要贴到命中区右上角，只有宿主知道那个几何）
      petLog('菜单: 打开对话输入窗');
      void invoke<void>('open_chat', { label: this.petConfig.label }).catch((err: unknown) =>
        petLogError('对话: 打开输入窗失败', err),
      );
      return;
    }
    if (action.action === 'balance') {
      // 余额查询在宿主侧（要读加密库里的 key、要走 20s 超时与退避重试）；
      // 页面只负责"说"——宿主查到后会经 pet://balance 事件回来
      petLog('菜单: 查余额');
      void invoke<string>('balance_query', { label: this.petConfig.label }).catch((err: unknown) =>
        petLogError('余额: 查询失败', err),
      );
      return;
    }
    if (action.action === 'home') {
      petLog('菜单: 回到初始位置');
      this.goHome();
      return;
    }
    if (action.anim) {
      if (isNoMirrorAnimation(this.petConfig.animations.categories, action.anim)) {
        petLog(`菜单: ${action.anim} 属于 noMirror 分类，先朝左再播`);
        this.chain.setFacing('left');
      }
      petLog(`菜单: 点播 ${action.anim}`);
      await this.playPicked(action.anim);
      return;
    }
    petLog(`菜单: 未处理的菜单项 anim=${action.anim ?? '(无)'} action=${action.action ?? '(无)'}`);
  }

  /** 点播一个动画（菜单调用；播放期间暂停动画链，避免和随机链抢） */
  private async playPicked(animation: string): Promise<void> {
    this.walk = null;
    this.chain.setPaused(true);
    this.chainPausedByInteraction = true;
    await this.playAnimation(animation, 'once');
  }

  /**
   * 碎碎念（M3）：宿主生成的一句话 → 弹气泡 +（可选）播一条 whisper 动画。
   *
   * 两件事**刻意解耦**（与上游 `client/pet.ts::triggerWhisper` 一致）：
   * 气泡是主要表现，动画是加分项——动画缺失（用户没配 `animations.events.whisper`）
   * 也照样说话，不会因为动画池没配就把碎碎念吞掉。
   *
   * 气泡停留 10 秒（上游 `BUBBLE_DURATION_MS` 同值）：太短来不及看，太长像卡住了。
   */
  /**
   * 余额（M3）：宿主查到余额 → 弹气泡 + 按**档位下标**播余额动画。
   *
   * 为什么传下标而不是动画名：动作池是"每只宠物可以不同"的，
   * 由页面从**自己的** `animations.events.balance` 里按下标取，才不会出现
   * "宿主挑了一条这只宠物没有的动画"。档位算法在宿主侧（`balance::animation_index`），
   * 与上游 `balanceEventIndex` 同一套：0 = 满溢 … 5 = 分文不剩。
   */
  async onBalance(text: string, animationIndex?: number): Promise<void> {
    const trimmed = text.trim();
    if (trimmed.length === 0) {
      petLogError('余额: 收到空文案，已忽略', null);
      return;
    }
    petLog(`余额: ${trimmed}（档位 ${animationIndex ?? '-'}）`);
    this.say(trimmed, 10_000);

    const slots = this.petConfig.animations.events?.['balance'];
    if (!slots || slots.length === 0) return;
    // 池子可能少于 6 条（用户自己裁剪过）：夹到池内，避免播一条不存在的动画
    const index = Math.min(Math.max(animationIndex ?? 0, 0), slots.length - 1);
    const animation = pickSlot(slots[index]);
    if (!animation) return;
    try {
      await this.playPicked(animation);
    } catch (err) {
      petLogError(`余额: 动画 ${animation} 播放失败`, err);
    }
  }

  async onWhisper(text: string, image?: string): Promise<void> {
    const trimmed = text.trim();
    if (trimmed.length === 0) {
      // 空文本不该走到这里（宿主侧已经按"模型未返回文本"拦下），真到了就记一笔别静默
      petLogError('碎碎念: 收到空文本，已忽略', null);
      return;
    }
    petLog(`碎碎念: ${trimmed}${image ? `（配图 ${image}）` : ''}`);
    this.say(trimmed, 10_000, undefined, image);

    // 档位有两种形状（单个名字 / 候选数组）：交给上游的 pickSlot 处理，避免自己再写一遍规则
    const slots = this.petConfig.animations.events?.['whisper'];
    const slot = slots && slots.length > 0 ? pick(slots) : null;
    const animation = slot ? pickSlot(slot) : '';
    if (!animation) return;
    try {
      await this.playPicked(animation);
    } catch (err) {
      // 动画失败不影响"已经说出来的那句话"
      petLogError(`碎碎念: 动画 ${animation} 播放失败`, err);
    }
  }

  /**
   * 回到配置的初始角落（菜单的"回到初始位置"）。
   *
   * 复用创建时用的角落 + 边距语义（`anchorPixel` 由 shared/motion 提供），
   * 取主屏工作区作为基准——与启动定位同一套几何，不会"回到别的地方"。
   */
  private goHome(): void {
    this.walk = null;
    const area = this.primaryArea;
    const box = anchorPixel({
      corner: this.petConfig.position.corner,
      marginX: this.petConfig.position.marginX,
      marginY: this.petConfig.position.marginY,
      size: this.petConfig.size,
      W: area.width,
      H: area.height,
      area,
    });
    petLog(`菜单: 回到初始位置 (${box.x.toFixed(0)},${box.y.toFixed(0)})（角落 ${this.petConfig.position.corner}）`);
    this.setBoxOrigin(box);
  }

  /**
   * 显示一句头顶气泡（**独立不可聚焦小窗**，见 `src-tauri/src/bubble.rs`）。
   *
   * 锚点 = 宠物包围盒**顶边中心**再往下压一点（`BUBBLE_HEAD_RATIO`）：
   * 包围盒是整段视频的外框，角色头顶通常还空着几十像素，按包围盒顶边放气泡会显得"飘在天上"。
   * 下压之后气泡底部的尖角正好戳在头发上，读起来才像"人物在说话"。
   *
   * 自动隐藏：默认 6 秒（与上游气泡展示时长同量级）。重复调用会重置计时器，
   * 不会出现"前一句的定时器把后一句提前关掉"。
   *
   * 气泡**会跟着宠物走**：锚点相对包围盒的偏移由宿主记下来（`bubble.rs` 的记账表），
   * 宠物每次移动（拖拽/抛掷/漫游）宿主都会顺手把气泡重摆一次，这里不需要逐帧上报。
   *
   * `anchor` 只供诊断自测使用：正常气泡永远摆在宠物**头顶**，而"气泡是否吃点击"这个
   * 问题只有让气泡与宠物**重叠**才测得出来（见 `runBubbleOverlapTest`）。
   */
  say(text: string, durationMs = 6000, anchor?: { x: number; y: number }, image?: string): void {
    const anchorX = anchor?.x ?? this.boxOrigin.x + this.petConfig.size / 2;
    const anchorY =
      anchor?.y ??
      this.boxOrigin.y + this.petConfig.size * this.petConfig.aspectRatio * BUBBLE_HEAD_RATIO;
    if (this.bubbleTimer) window.clearTimeout(this.bubbleTimer);
    void invoke('show_bubble', {
      label: this.petConfig.label,
      anchorX,
      anchorY,
      // 包围盒原点一起给：宿主侧那份在"菜单外扩/缩回"期间会短暂不一致，
      // 让宿主据此推算"锚点相对包围盒的偏移"会把气泡算错一个外扩量（见 BubbleRequest 注释）
boxX: this.boxOrigin.x,
      boxY: this.boxOrigin.y,
      text,
      // image = 可选配图（相对素材路径）；不传就是纯文字气泡
      image,
    }).catch((err: unknown) => petLogError('气泡: 显示失败', err));
    petLog(`气泡: 显示「${text}」锚点=(${anchorX.toFixed(0)},${anchorY.toFixed(0)}) 时长 ${durationMs}ms`);
    this.bubbleTimer = window.setTimeout(() => {
      this.bubbleTimer = 0;
      void invoke('hide_bubble', { label: this.petConfig.label }).catch((err: unknown) =>
        petLogError('气泡: 隐藏失败', err),
      );
      petLog('气泡: 自动隐藏');
    }, durationMs);
  }

  /** 气泡自动隐藏计时器（0 = 未计时） */
  private bubbleTimer = 0;

  /** 释放资源（页面卸载时调用） */
  dispose(): void {
    this.disposed = true;
    if (this.playWatchdog) {
      window.clearTimeout(this.playWatchdog);
      this.playWatchdog = 0;
    }
    this.chain.stop();
    this.squash.cancel();
    this.thrower.stop();
    this.media.dispose();
    this.channels.cursor.dispose();
  }

  // ---------------------------------------------------------------------------
  // 自测钩子（诊断用，仅在 URL 带 ?autotest=1 时由 main.ts 调用）
  // ---------------------------------------------------------------------------

  /**
   * 跑一遍**受控的**拖拽 + 甩抛序列，把结果写进诊断日志。
   *
   * 为什么需要它：
   *   真实鼠标注入在自动化环境里不可靠（桌面可能被别的窗口遮挡、SetCursorPos 会被干扰），
   *   而"拖拽跟手 + 甩抛落地"恰恰是桌宠最核心的手感逻辑，必须有可重复的验证手段。
   *
   * 这里走的是**与真实输入完全相同的入口**：用合成光标采样喂给 `onCursorFrame`，
   * 合成帧的 `primaryDown` 在拖拽段为 true、松手时那一帧为 false——因此连"按键收尾"
   * 这条路径也一并被覆盖（只有真的按着，状态机才允许继续跟手）。
   *
   * 不再依赖 DOM 的 pointerup 作为唯一收尾依据——这正是修复"拖到命中区外松手后失控"的关键：
   * 那次 `pointerup` 根本没到页面，状态机停在"按下中"，宠物继续跟着指针、窗口也不再恢复穿透。
   * 现在每一帧都带宿主的**物理按键状态**（`primaryDown`），按键一松就强制收尾。
   */
  async runSelfTest(): Promise<void> {
    this.syntheticControl = true;
    try {
      await this.runSelfTestSequence((message) => petLog(`自测: ${message}`));
    } finally {
      this.syntheticControl = false;
      this.syntheticPrimaryDown = false;
    }
  }

  /**
   * 构造一帧合成采样（屏幕坐标；**按键位由自测覆盖**，见 syntheticPrimaryDown）。
   *
   * 注意：这里刻意不接受 primaryDown 参数——自测运行期间宿主的真实按键是松开的，
   * 若让合成帧与宿主帧各自携带按键位，两条来源会互相打架（自测刚按下就被宿主帧收尾）。
   * 现在按键位统一由 `syntheticPrimaryDown` 提供，真实光标位置仍来自宿主，两条通道不再冲突。
   */
  private syntheticFrame(x: number, y: number): CursorFrame {
    // 自测期间 `onCursorFrame` 不会用宿主采样覆盖 cursorScreen（见其实现），
    // 但它会读 frame.x/y —— 因此这里把合成坐标写进 cursorScreen 再交回原始值，
    // 让"合成输入"与"真实输入"严格走同一条路径。
    this.cursorScreen = { x, y };
    this.lastSampledPrimaryDown = this.syntheticPrimaryDown;
    return { x, y, at: performance.now(), primaryDown: this.syntheticPrimaryDown };
  }

  /**
   * 推进一帧合成序列：喂入合成状态并**真实等一帧**。
   *
   * 为什么要等一帧：判定统一在主循环 `frameTick` 里做（拖拽起手、看门狗收尾都在那儿），
   * 所以状态变化要到下一帧才生效——这也正是真实运行的时序。
   */
  private async pumpSynthetic(x: number, y: number, primaryDown: boolean, delayMs: number): Promise<void> {
    this.syntheticPrimaryDown = primaryDown;
    this.syntheticFrame(x, y);
    await new Promise((resolve) => window.setTimeout(resolve, delayMs));
  }

  /** 自测主序列（与 runSelfTest 分离，便于用 try/finally 恢复被替换的判定函数） */
  private async runSelfTestSequence(log: (message: string) => void): Promise<void> {
    // 从当前包围盒中心按下（命中区中心 = 命中框中心）
    const start: Vec2 = {
      x: this.boxOrigin.x + this.hitBox.x + this.hitBox.width / 2,
      y: this.boxOrigin.y + this.hitBox.y + this.hitBox.height / 2,
    };
    const boxAtStart = { ...this.boxOrigin };
    const eventsAtStart = this.windowStateEvents;
    log(
      `开始 起点=(${start.x.toFixed(0)},${start.y.toFixed(0)}) ` +
        `包围盒=(${boxAtStart.x.toFixed(0)},${boxAtStart.y.toFixed(0)})`,
    );

    // 起手：喂一帧"在身体上按下"——与真实输入走同一条路径（按键位由自测提供）
    await this.pumpSynthetic(start.x, start.y, true, 40);
    if (!this.drag.isPressed) {
      log('异常: 身体内按下未被识别，自测中止');
      return;
    }

    // 分步移动：**刻意贴近真实鼠标的事件粒度**（~16ms / ≈9px），
    // 而不是"每步几十像素的巨大跳变"——后者会让过阻尼弹簧显出夸张的滞后，
    // 那是测试轨迹造成的假象，不代表真实手感。
    const stepMs = 16;
    /** 每步位移（≈9px/帧 @60Hz，与真实鼠标拖拽的观感等价） */
    const stepPx = 9;
    const steps = 30;
    /** 最近一次喂给弹簧的指针位置（屏幕坐标）：用来算弹簧滞后量 */
    let lastPointerRaw: Vec2 = { ...start };
    for (let i = 1; i <= steps; i += 1) {
      const point: Vec2 = { x: start.x - i * stepPx, y: start.y + i * (stepPx / 3) };
      lastPointerRaw = point;
      // 拖拽期间按键位保持 true（syntheticPrimaryDown 已置位）：这是状态机"还按着"的依据
      await this.pumpSynthetic(point.x, point.y, true, stepMs);
    }
    const boxAfterDrag = { ...this.boxOrigin };
    // 弹簧滞后量 = （指针 − 抓取偏移） − 包围盒。
    // 按下点就是命中区中心，所以抓取偏移 = 命中区中心在包围盒内的偏移；
    // 这个差值就是"宠物没完全跟住指针"的距离，是手感的核心指标
    // （实现成"位置直接等于指针"的话它会是 0）。
    const grabX = this.hitBox.x + this.hitBox.width / 2;
    const grabY = this.hitBox.y + this.hitBox.height / 2;
    const lagX = lastPointerRaw.x - grabX - boxAfterDrag.x;
    const lagY = lastPointerRaw.y - grabY - boxAfterDrag.y;

    log(
      `拖拽中 包围盒=(${boxAfterDrag.x.toFixed(0)},${boxAfterDrag.y.toFixed(0)}) ` +
        `位移=(${(boxAfterDrag.x - boxAtStart.x).toFixed(0)},${(boxAfterDrag.y - boxAtStart.y).toFixed(0)}) ` +
        `弹簧滞后=(${lagX.toFixed(0)},${lagY.toFixed(0)}) ` +
        `窗口回传累计=${this.windowStateEvents - eventsAtStart} 次`,
    );

    // 甩手：最后 3 帧保持方向但把步长放大到 ~40px/帧（≈2500px/s），
    // 即"用力甩出去"的真实量级——初速估算窗口（150ms）内要有足够位移才会触发抛掷。
    // 全部帧的按键位仍为 true；**松手由"按键位变 false 的那一帧"表达**，
    // 这正是修复后状态机的权威收尾路径（不再依赖 DOM 的 pointerup）。
    for (let i = 1; i <= 3; i += 1) {
      const point: Vec2 = { x: lastPointerRaw.x - i * 40, y: lastPointerRaw.y + i * 20 };
      await this.pumpSynthetic(point.x, point.y, true, 16);
    }
    // 松手：按键位翻成 false（**这正是修复后状态机的权威收尾路径**，不再依赖 DOM pointerup）
    await this.pumpSynthetic(lastPointerRaw.x - 120, lastPointerRaw.y + 60, false, 40);
    log(`松手: 已喂入"按键松开"帧，状态机按下中=${this.drag.isPressed}（期望 false）`);

    // 等飞行结束（ThrowAnimator 会在 atRest 时回调，最多等 12 秒）
    await new Promise((resolve) => window.setTimeout(resolve, 12_000));
    const elapsed = (performance.now() - this.firstWindowStateAt) / 1000;
    log(
      `结束 最终包围盒=(${this.boxOrigin.x.toFixed(0)},${this.boxOrigin.y.toFixed(0)}) ` +
        `飞行中=${this.flying}`,
    );
    // 逐帧窗口跟随的客观证据：窗口被移动的次数与平均回传频率
    log(
      `窗口回传 总计=${this.windowStateEvents} 次，平均 ${(this.windowStateEvents / Math.max(elapsed, 0.001)).toFixed(1)} 次/秒` +
        `（本次自测期间新增 ${this.windowStateEvents - eventsAtStart} 次）`,
    );
  }

  /**
   * 专项自测（`?autotest=10`）：**连续多轮"甩出去 → 在空中抓住 → 再甩一次"**，
   * 每一次松手都**必须**观察到新的飞行开始。
   *
   * ## 为什么需要它（真实 bug 的回归网）
   *
   * 用户实测："只有第一次能拖出来甩，后面怎么拖都不飞"。根因是"是否在飞"这件事被记在
   * 两个地方：`ThrowAnimator` 自己的 rAF 状态，和运行时的 `flying` 标志。
   * 而"在空中被抓住"走的是 `ThrowAnimator.stop()` —— 它按设计**不回调 `onRest`**，
   * 于是标志漏了复位，`startThrow` 里的"已在飞行就跳过"守卫从此永远为真。
   *
   * 修复后"是否在飞"只有一个真相（`flying` 是只读属性，直接读 `ThrowAnimator.running`），
   * 并且去掉了那个守卫。本自测把这个语义钉死：**每轮两次起飞都必须发生**。
   *
   * 顺带记录两个手感量（不需要真鼠标，全部走运行时自己的状态机）：
   *   - 拖动期间的**弹簧滞后**（指针目标 − 窗口实际位置）的均值 / 峰值；
   *   - 飞行期间的**每帧步长**均值 / 峰值（用来判断有没有"一顿一顿"）。
   */
  async runRepeatThrowTest(rounds = 3): Promise<void> {
    this.syntheticControl = true;
    try {
      await this.runRepeatThrowSequence(rounds, (message) => petLog(`重复甩出自测: ${message}`));
    } finally {
      this.syntheticControl = false;
      this.syntheticPrimaryDown = false;
    }
  }

  /** 重复甩出自测的主序列（见 `runRepeatThrowTest` 的说明） */
  private async runRepeatThrowSequence(rounds: number, log: (message: string) => void): Promise<void> {
    /** 命中区中心在包围盒内的偏移（抓取点就是它） */
    const grabX = this.hitBox.x + this.hitBox.width / 2;
    const grabY = this.hitBox.y + this.hitBox.height / 2;
    let takes = 0;
    let expected = 0;

    /** 一轮"按下 → 拖 → 甩 → 松手"，返回是否观察到起飞 */
    const dragAndThrow = async (label: string): Promise<boolean> => {
      // 按下：**先喂一帧"光标在很远的地方"**，再把光标点到宠物身上按下。
      //
      // 为什么必须先走这一步：采样兜底起手的前提是"这一帧开始时窗口还是穿透态"
      //（可交互时按下应该走 DOM，见 evaluateInput 的兜底起手说明）。而合成自测没有 DOM，
      // 所以必须自己制造"穿透态 + 光标驶入身体"这个前提；空中抓取时宠物高速移动，
      // 每轮都重新瞄准一次才能稳定抓到（一次最多重试 8 帧）。
      let pressed = false;
      let start: Vec2 = { x: 0, y: 0 };
      for (let attempt = 1; attempt <= 8 && !pressed; attempt += 1) {
        await this.pumpSynthetic(this.boxOrigin.x - 600, this.boxOrigin.y - 400, false, 16);
        start = { x: this.boxOrigin.x + grabX, y: this.boxOrigin.y + grabY };
        await this.pumpSynthetic(start.x, start.y, true, 16);
        pressed = this.drag.isPressed;
      }
      if (!pressed) {
        log(`${label}: 异常——连续 8 帧都没能在身体上按下`);
        return false;
      }
      // 分步拖动（≈9px/16ms，贴近真实鼠标事件粒度），同时采样弹簧滞后
      let lagSum = 0;
      let lagMax = 0;
      let lagCount = 0;
      let last: Vec2 = { ...start };
      for (let i = 1; i <= 20; i += 1) {
        const point: Vec2 = { x: start.x - i * 9, y: start.y + i * 3 };
        last = point;
        await this.pumpSynthetic(point.x, point.y, true, 16);
        const lag = Math.hypot(point.x - grabX - this.boxOrigin.x, point.y - grabY - this.boxOrigin.y);
        lagSum += lag;
        lagMax = Math.max(lagMax, lag);
        lagCount += 1;
      }
      // 甩手：最后 3 帧放大到 ~40px/帧（≈2500px/s，落在初速估算窗口内）
      for (let i = 1; i <= 3; i += 1) {
        await this.pumpSynthetic(last.x - i * 40, last.y + i * 20, true, 16);
      }
      const beforeThrow = this.flying;
      // 松手：**按真实路径补一条 DOM pointerup**（pointerId=-1 与采样起手的绑定一致）。
      //
      // 为什么不能只喂"按键位翻 false"的采样帧：那条路要等按键看门狗的静默阈值
      //（`DOG_SILENCE_MS = 250ms`）才收尾，而初速估算的样本有效期只有 150ms——
      // 于是自测里的甩出会随机退化成"温柔放下"（假失败）。真实世界里窗口是可交互的，
      // 松手就是一条立刻到达的 DOM pointerup，这里补上它才与真实时序一致。
      window.dispatchEvent(
        new PointerEvent('pointerup', { bubbles: true, button: 0, buttons: 0, pointerId: -1 }),
      );
      await this.pumpSynthetic(last.x - 120, last.y + 60, false, 60);
      const tookOff = !beforeThrow && this.flying;
      log(
        `${label}: 松手前飞行中=${beforeThrow} → 松手后飞行中=${this.flying} ` +
          `（起飞=${tookOff}）弹簧滞后 平均=${(lagSum / Math.max(lagCount, 1)).toFixed(0)}px ` +
          `峰值=${lagMax.toFixed(0)}px`,
      );
      return tookOff;
    };

    /** 等这次飞行结束（最多 15 秒），并统计每帧步长 */
    const waitToLand = async (label: string): Promise<void> => {
      const deadline = performance.now() + 15_000;
      let prev = { ...this.boxOrigin };
      let steps = 0;
      let sum = 0;
      let max = 0;
      while (this.flying && performance.now() < deadline) {
        await new Promise((resolve) => window.setTimeout(resolve, 16));
        const d = Math.hypot(this.boxOrigin.x - prev.x, this.boxOrigin.y - prev.y);
        if (d > 0.5) {
          steps += 1;
          sum += d;
          max = Math.max(max, d);
        }
        prev = { ...this.boxOrigin };
      }
      log(
        `${label}: 落地=${!this.flying} 位置=(${this.boxOrigin.x.toFixed(0)},${this.boxOrigin.y.toFixed(0)}) ` +
          `帧步长 平均=${(sum / Math.max(steps, 1)).toFixed(1)}px 峰值=${max.toFixed(1)}px（${steps} 帧有位移）`,
      );
    };

    log(`开始 轮数=${rounds}（每轮期望两次起飞：先甩出，再在空中抓住重甩）`);
    for (let round = 1; round <= rounds; round += 1) {
      expected += 2;
      if (await dragAndThrow(`第${round}轮·甩出`)) takes += 1;
      // **不等落地**就在空中抓住重甩——这正是当年"标志漏复位"的触发路径
      await new Promise((resolve) => window.setTimeout(resolve, 150));
      if (await dragAndThrow(`第${round}轮·空中重甩`)) takes += 1;
      await waitToLand(`第${round}轮·落地`);
    }
    const passed = takes === expected;
    log(
      `结果: 起飞 ${takes}/${expected} 次 —— ${passed ? '✓ 每轮都成功起飞（重复甩出可用）' : '✗ 有甩出没有起飞（回归！）'}`,
    );
  }

  /**
   * 专项自测：**"拖到命中区外才松手"**（`?autotest=2`）——复现并验证失控 bug 的修复。
   *
   * 失败场景（用户实际遇到的）：
   *   拖拽中宠物滞后于光标（实测约 121px），光标滑出身体命中区 → 前端把窗口翻回点击穿透
   *   → 用户此刻松手，`pointerup` 永远到不了页面 → 状态机停在"按下中"：宠物继续跟指针、
   *   窗口也不再恢复穿透，手感彻底失控。
   *
   * 本自测用合成帧精确复现这条路径，并核对两条行为：
   *   阶段 2：按键仍按下、光标已在命中区外 → 宠物**不再乱跑**（采样兜底的按下不得脱离命中区）；
   *   阶段 3/4：喂入"按键松开"帧 → 状态机**必须收尾**，之后不再移动。
   */
  async runDetachTest(): Promise<void> {
    this.syntheticControl = true;
    try {
      await this.runDetachSequence((message) => petLog(`失控自测: ${message}`));
    } finally {
      this.syntheticControl = false;
      this.syntheticPrimaryDown = false;
    }
  }

  /** 失控场景自测的主序列 */
  private async runDetachSequence(log: (message: string) => void): Promise<void> {
    const start: Vec2 = {
      x: this.boxOrigin.x + this.hitBox.x + this.hitBox.width / 2,
      y: this.boxOrigin.y + this.hitBox.y + this.hitBox.height / 2,
    };
    log(
      `开始 起点=(${start.x.toFixed(0)},${start.y.toFixed(0)}) ` +
        `命中框=${this.hitBox.width.toFixed(0)}x${this.hitBox.height.toFixed(0)}`,
    );

    // 阶段 1：在身体上按下，并**强制来源为 sampler**——模拟"真实按下时窗口还是穿透态、
    // DOM 事件没到"的最坏情况（修复必须覆盖它：这种按下的运动被限制在命中区内）
    this.forcePressOrigin('sampler');
    await this.pumpSynthetic(start.x, start.y, true, 40);
    if (!this.drag.isPressed) {
      log('异常: 身体内按下未被识别，自测中止');
      return;
    }
    const pressOrigin = this.drag.origin;
    log(`阶段1: 已按下，来源=${pressOrigin}（期望 sampler）`);

    // 阶段 2：按住并拖出命中区（远超过命中框半宽 ~79px）
    const outside: Vec2 = { x: start.x - 400, y: start.y + 120 };
    for (let i = 1; i <= 12; i += 1) {
      const p: Vec2 = {
        x: start.x + ((outside.x - start.x) * i) / 12,
        y: start.y + ((outside.y - start.y) * i) / 12,
      };
      await this.pumpSynthetic(p.x, p.y, true, 16);
    }
    const boxBeforeIdle = { ...this.boxOrigin };
    for (let i = 0; i < 10; i += 1) {
      await this.pumpSynthetic(outside.x, outside.y, true, 16);
    }
    const idleDrift = Math.hypot(this.boxOrigin.x - boxBeforeIdle.x, this.boxOrigin.y - boxBeforeIdle.y);
    log(
      `阶段2: 区外持续按住 10 帧，漂移=${idleDrift.toFixed(1)}px ` +
        `（期望 < 60：采样兜底的按下不允许脱离命中区继续跟随）`,
    );

    // 阶段 3：关键一步——喂入"按键已松开"帧（真实场景里 pointerup 丢失的那一刻）
    const eventsBeforeRelease = this.windowStateEvents;
    await this.pumpSynthetic(outside.x, outside.y, false, 40);
    const released = !this.drag.isPressed;
    log(`阶段3: 喂入"按键松开"帧 → 状态机已收尾=${released}（期望 true）`);

    // 阶段 4：收尾后继续喂 10 帧（光标仍在区外、按键保持松开）：宠物不得再移动
    const boxAfterRelease = { ...this.boxOrigin };
    for (let i = 0; i < 10; i += 1) {
      await this.pumpSynthetic(outside.x, outside.y, false, 16);
    }
    const afterDrift = Math.hypot(this.boxOrigin.x - boxAfterRelease.x, this.boxOrigin.y - boxAfterRelease.y);
    log(
      `阶段4: 收尾后 10 帧漂移=${afterDrift.toFixed(1)}px（期望 ≈0）；` +
        `期间窗口移动 ${this.windowStateEvents - eventsBeforeRelease} 次`,
    );
    log(`结论: 收尾=${released ? '成功' : '失败'}，区外漂移=${idleDrift.toFixed(1)}px，收尾后漂移=${afterDrift.toFixed(1)}px`);
    await new Promise((resolve) => window.setTimeout(resolve, 1500));
  }

  /**
   * 诊断：**立刻强制走一段**（`WHALE_PET_FORCE_MOVE=1` 时由 main.ts 调用一次）。
   *
   * 为什么需要：漫游权重只有 5%（`animationWeights.move`），自动化验证时很难等到它，
   * 而"走路是否真的带动窗口位移"必须被确定性验证。本方法走的是与随机链**完全相同**的
   * 路径（`planMove` → `startWalk` → 逐帧推进），只是跳过了掷骰。
   */
  async forceWalk(): Promise<void> {
    const specs = this.petConfig.animations.moves.actions;
    if (specs.length === 0) {
      petLog('漫游: 移动池为空，无法强制行走');
      return;
    }
    const spec = specs[0];
    const merged = { ...this.petConfig.animations.moves.default, ...(spec.params ?? {}) };
    const plan = this.planWalk(-1, {
      minDist: Number(merged.minDist) || 60,
      maxDist: Number(merged.maxDist) || 240,
      margin: Number(merged.margin) || 20,
    });
    if (!plan) {
      petLog('漫游: 强制行走失败（空间不足）');
      return;
    }
    const full = { ...plan, animation: spec.name, leadSec: Number(merged.leadSec) || 2, tailSec: Number(merged.tailSec) || 2 };
    this.startWalk(full);
    this.chain.setPaused(true);
    await this.playAnimation(spec.name, 'once');
    this.chain.setPaused(false);
  }

  /**
   * 专项自测（`?autotest=8`）：把气泡**故意压在宠物身体上**，用来验证"气泡不吃点击"。
   *
   * 为什么必须重叠才测得出来：气泡正常摆在宠物**头顶**（两者在屏幕上不重叠），
   * 那种摆位下"气泡吃点击"与"宠物吃点击"是同一个症状，无法区分。
   * 这里把锚点下移到身体中部，让气泡窗与宠物窗重叠，然后由**外部脚本在重叠点上
   * 打一次真鼠标点击**（`scripts/probe-window-styles.ps1` 的姊妹工具，见文档）：
   *   - 点击穿透成立 → 事件穿过气泡落到下面的宠物窗 → 宠物播放"点击回应"动画；
   *   - 气泡吃点击 → 宠物毫无反应。
   * 判据是宠物日志里那一行动画切换，不依赖任何样式探针。
   */
  runBubbleOverlapTest(): void {
    const anchorX = this.boxOrigin.x + this.petConfig.size / 2;
    // 0.5 倍包围盒高度：气泡底边落在身体中线上，与命中区（约 33~220）稳定重叠
    const anchorY = this.boxOrigin.y + this.petConfig.size * 0.5;
    petLog(`气泡穿透自测: 气泡压在身体上 锚点=(${anchorX.toFixed(0)},${anchorY.toFixed(0)})，保持 60s 等外部真点击`);
    this.say('气泡穿透真点击自测：这条气泡压在宠物身上，验证点击能否落到宠物', 60_000, { x: anchorX, y: anchorY });
  }

  /** 诊断：启动一段有限长度的实时探针（`?autotest=3`），用于核对坐标链与命中判定 */
  startProbe(frames = 180): void {    this.probeFramesLeft = frames;
    petLog(`探针: 启动，将持续 ${frames} 帧（约 ${(frames * 16) / 1000}s）`);
  }

  /**
   * 诊断：让状态机认为"物理按键一直按着"（`?autotest=4`），持续指定时长。
   *
   * 为什么需要它：合成输入（宿主注入的 pointerdown/move/up）不会改变宿主的物理按键状态，
   * 而"按键看门狗"（见 onCursorFrame 规则 1）会在下一帧就把状态机收尾——
   * 合成拖拽因此只能活 1 帧，测不出跟手行为。
   * 本方法把按键位临时交给自测控制，让合成拖拽能完整跑完。
   */
  holdSyntheticButton(ms = 3000): void {
    this.syntheticControl = true;
    this.syntheticPrimaryDown = true;
    petLog(`合成输入: 按键位交给自测控制 ${ms}ms（供注入式拖拽验证使用）`);
    window.setTimeout(() => {
      this.syntheticPrimaryDown = false;
      petLog('合成输入: 释放按键位控制');
    }, ms);
  }

  /**
   * 诊断用：把**当前这次按下**的来源改写为指定值（仅自测使用）。
   *
   * 存在的理由：正常顺序下，自测的第一帧落在身体上时 `updateHitState` 已经把窗口置为可交互，
   * 于是行走的是 DOM 快路径（`origin='dom'`）；而修复要覆盖的最坏情况恰恰是
   * **DOM 事件根本没到**（按下发生在窗口还处于穿透态的那一瞬间）——那条路径的按下，
   * 其运动被限制在命中区内，行为与 DOM 路径**不同**，必须被自测覆盖。
   * 这里用"以同一位置、指定来源重新按下"来实现改写（幂等：不产生位移）。
   */
  private forcePressOrigin(origin: PressOrigin): void {
    if (!this.drag.isPressed) return;
    this.drag.onPointerDown(this.drag.pressedPoint, { ...this.boxOrigin }, origin);
  }
}
