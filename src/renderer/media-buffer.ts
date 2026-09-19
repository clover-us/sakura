/**
 * 双缓冲视频播放器：透明动画的"零空白帧"切换。
 *
 * 上游做法（见 dsh-pet 的 sprite.js）：两个 `<video>` 交替承担显示，
 * 新动画在隐藏的那一张上加载并 seek 到首帧，`seeked` 后再交叉淡入，
 * 于是切换过程中屏幕上永远有一帧有效画面——单 video 换 src 必然闪一帧空白。
 *
 * M0 阶段只实现"循环播放一个动画 + 可切换到另一个动画"，
 * 但接口按将来要接的**完整动画链**设计（switchTo / 播放完成回调 / dispose），
 * 避免 M1 移植上游动画链时推翻重写。
 *
 * 透明播放要点：
 *   - 素材必须是 VP9-alpha 的 webm（上游发布格式），WebView2 = Chromium 内核可直接透明播放；
 *   - video 必须 muted，否则会被自动播放策略拦截（play() 返回 rejected promise）；
 *   - 不要给 video 设置不透明背景（CSS 里已确保 background: transparent）。
 */

/** 单次播放的结束策略 */
export type LoopMode = 'loop' | 'once';

/** 待机回调：`once` 模式播放结束的通知（用于接动画链） */
export type EndedHandler = (url: string) => void;

/** 排障回调：记录"哪条动画播起来了 / 哪条播不动" */
export type MediaEventHandler = (message: string) => void;

export class MediaBuffer {
  /** 两层缓冲（只存元素本身；"是否当前可见"由 class 表达，避免两份状态不一致） */
  private readonly layers: [HTMLVideoElement, HTMLVideoElement];
  /** 当前可见层下标（0 或 1） */
  private frontIndex = 0;
  /** 当前正在播放的动画 URL（同一 URL 重复播放时用于判断是否需要重新 load） */
  private currentUrl = '';
  /** 当前动画的循环模式 */
  private currentLoop: LoopMode = 'loop';
  /** 本次播放是否已经派发过 ended 回调（once 模式防重复） */
  private endedFired = false;

  /**
   * @param videoA 第一层 video 元素
   * @param videoB 第二层 video 元素
   * @param onEnded once 模式播放结束时的回调（用于接动画链的"播完选下一个"）
   * @param onEvent 排障日志回调（可缺省）；播不动/加载失败这类问题在透明窗口上完全不可见，
   *                必须有一条日志通道，否则只能靠猜
   */
  constructor(
    videoA: HTMLVideoElement,
    videoB: HTMLVideoElement,
    private readonly onEnded: EndedHandler,
    private readonly onEvent: MediaEventHandler = () => {},
  ) {
    this.layers = [videoA, videoB];
    // 监听两层各自的 ended 事件：只有"当前可见层"的结束才对外派发
    this.layers.forEach((el, index) => {
      el.addEventListener('ended', () => {
        if (index !== this.frontIndex) return;
        if (this.currentLoop !== 'once' || this.endedFired) return;
        this.endedFired = true;
        this.onEnded(this.currentUrl);
      });
    });
  }

  /** 当前可见层 */
  private get front(): HTMLVideoElement {
    return this.layers[this.frontIndex];
  }

  /** 当前隐藏层（下一次播放的目标层） */
  private get back(): HTMLVideoElement {
    return this.layers[this.frontIndex === 0 ? 1 : 0];
  }

  /**
   * 播放一个动画。
   *
   * @param url 动画资源 URL（含 `pet://` 协议前缀）
   * @param mode 循环模式；`once` 结束时触发构造时的 onEnded 回调
   * @returns 新动画首帧就绪后 resolve（调用方据此安排后续动作，例如连续切换）
   */
  async switchTo(url: string, mode: LoopMode = 'loop'): Promise<void> {
    // 同一 URL 且仍在播放：不重新加载，避免无谓的重解码与闪帧（只同步循环模式）
    if (url === this.currentUrl && !this.front.paused) {
      if (mode !== this.currentLoop) {
        this.currentLoop = mode;
        this.endedFired = false;
        this.front.loop = mode === 'loop';
      }
      return;
    }

    const target = this.back;
    target.loop = mode === 'loop';
    target.src = url;
    // load() 显式触发资源加载：换 src 后不调用也能工作，但显式调用能确保
    // 旧请求被取消（快速连续切换时避免旧动画的帧串进来）
    target.load();

    await this.waitForFrame(target);

    // 交叉淡入：先点亮新层，再淡出旧层（顺序不能反，否则中间会有一帧两边都透明）
    target.currentTime = 0;
    void target.play().catch((err: unknown) => {
      // 自动播放被拦截/解码未就绪：降级为"保持当前帧"，不打断宠物其它行为
      this.onEvent(`ERROR 播放被拒绝 url=${url} err=${err instanceof Error ? err.message : String(err)}`);
    });
    target.classList.add('is-front');
    this.front.classList.remove('is-front');
    this.onEvent(
      `动画: 切换完成 mode=${mode} readyState=${target.readyState} ` +
        `尺寸=${target.videoWidth}x${target.videoHeight} url=${url}`,
    );

    this.frontIndex = this.frontIndex === 0 ? 1 : 0;
    this.currentUrl = url;
    this.currentLoop = mode;
    this.endedFired = false;
  }

  /**
   * 等待视频具备可显示的首帧（readyState >= HAVE_CURRENT_DATA）。
   *
   * 为什么要等首帧而不是等 `canplay`：
   *   `canplay` 只保证"能开始播"，此刻把层点亮可能仍是空白帧；
   *   HAVE_CURRENT_DATA（2）才表示当前播放位置已有数据可渲染。
   * 超时保护：网络/解码异常时不让调用方永久挂起（超时后照样点亮，由浏览器抖动补齐）。
   */
  private waitForFrame(el: HTMLVideoElement, timeoutMs = 2000): Promise<void> {
    if (el.readyState >= HTMLMediaElement.HAVE_CURRENT_DATA) return Promise.resolve();
    return new Promise<void>((resolve) => {
      let settled = false;
      const finish = (reason: string): void => {
        if (settled) return;
        settled = true;
        el.removeEventListener('seeked', onSeeked);
        el.removeEventListener('loadeddata', onLoadedData);
        el.removeEventListener('error', onError);
        window.clearTimeout(timer);
        if (reason !== 'ok') {
          // 超时/错误都要留痕：透明窗口里"宠物不出现"最常见的原因就是这里
          this.onEvent(`ERROR 等待首帧失败 reason=${reason} src=${el.src}`);
        } else {
          this.onEvent(`动画: 首帧就绪 readyState=${el.readyState} src=${el.src}`);
        }
        resolve();
      };
      const onSeeked = (): void => finish('ok');
      const onLoadedData = (): void => finish('ok');
      const onError = (): void => finish('error');
      el.addEventListener('seeked', onSeeked);
      el.addEventListener('loadeddata', onLoadedData);
      el.addEventListener('error', onError);
      const timer = window.setTimeout(() => finish('timeout'), timeoutMs);
      // 部分情况下换 src 后不会自动触发 seeked，这里主动 seek 到 0 促使解码首帧
      try {
        el.currentTime = 0;
      } catch {
        /* 某些状态（如尚未有元数据）下设置 currentTime 会抛错，忽略即可 */
      }
    });
  }

  /** 当前是否正在播放 */
  get playing(): boolean {
    return !this.front.paused;
  }

  /**
   * 当前动画时长（秒）；元数据未就绪或时长非法时返回 0（调用方自行兜底）。
   *
   * 用途：走路动画的位移节奏要与动画时长对齐（lead/tail 期间原地不动），
   * 因此需要拿到真实时长而不是拍一个常数。
   */
  get currentDurationSec(): number {
    const duration = this.front.duration;
    return Number.isFinite(duration) && duration > 0 ? duration : 0;
  }

  /** 释放资源（窗口销毁前调用，避免 WebView2 的媒体资源悬挂） */
  dispose(): void {
    for (const el of this.layers) {
      el.pause();
      el.removeAttribute('src');
      el.load();
    }
  }
}
