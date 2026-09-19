/**
 * 全局光标采样通道（Tauri 下的**必需**基建，不是优化项）。
 *
 * 为什么必须有它：
 *   Electron 的 `setIgnoreMouseEvents(true, { forward: true })` 在穿透的同时仍会把
 *   鼠标移动事件转发给页面，所以上游能用 `mousemove` 自行判断"光标进了身体"再翻转穿透。
 *   Tauri 的 `set_ignore_cursor_events(bool)` **没有 forward 语义**：一旦穿透，WebView
 *   收不到任何鼠标事件，页面无法自愈，宠物会永久失去交互能力。
 *
 *   因此这里采用"全局采样 + 页面命中判定"的方案：
 *     Rust 侧固定 ~50ms 轮询一次系统光标位置 → emit(`pet://cursor`) → 本模块派发帧 →
 *     业务层换算到本地坐标做命中判定 → 调 Rust 翻转"可交互 / 穿透"。
 *
 * 采样频率权衡：
 *   50ms ≈ 20Hz 只用于**翻转决策**，人眼/手感完全够用；
 *   拖拽跟手的实时性不依赖它——见 drag.ts 的说明，拖拽期间 DOM 的 pointermove
 *   仍在持续到达（窗口处于可交互状态），采样只作为兜底与穿透态下的唯一来源。
 */

/** 一帧光标采样（屏幕坐标系，物理像素） */
export interface CursorFrame {
  /** 光标屏幕坐标 */
  x: number;
  y: number;
  /** 该采样的时间戳（毫秒，取自 performance.now 同基准） */
  at: number;
  /** 主键是否按下（全局按键状态，不依赖窗口消息——穿透期间也能拿到） */
  primaryDown: boolean;
}

/** 帧回调类型 */
export type CursorFrameHandler = (frame: CursorFrame) => void;

export class CursorChannel {
  /** Rust 侧 listen 返回的取消函数（尚未完成注册时为 null） */
  private unlisten: (() => void) | null = null;
  /** 是否已 dispose（防止 dispose 与异步注册竞态导致回调泄漏） */
  private disposed = false;
  /** 最近一帧（业务层可在任意时刻读取，避免为一次命中查询而等待下一帧） */
  private latest: CursorFrame | null = null;

  /**
   * @param listen 事件注册函数（从 bridge/tauri.ts 注入，便于测试时替换）
   * @param onFrame 每帧回调（业务层在此做命中判定、拖拽跟随与按键收尾）
   */
  constructor(
    private readonly listen: (
      event: string,
      handler: (payload: { position: { x: number; y: number }; at: number; primaryDown: boolean }) => void,
    ) => Promise<() => void>,
    private readonly onFrame: CursorFrameHandler,
  ) {}

  /** 启动监听（幂等；页面卸载时会自动取消） */
  async start(): Promise<void> {
    if (this.unlisten || this.disposed) return;
    const unlisten = await this.listen('pet://cursor', (payload) => {
      if (this.disposed) return;
      const frame: CursorFrame = {
        x: payload.position.x,
        y: payload.position.y,
        at: payload.at,
        primaryDown: payload.primaryDown,
      };
      this.latest = frame;
      this.onFrame(frame);
    });
    // 注册过程中可能已经被 dispose：此时立刻退订，避免悬挂监听
    if (this.disposed) {
      unlisten();
      return;
    }
    this.unlisten = unlisten;
  }

  /** 最近一帧采样；从未收到过则为 null */
  get last(): CursorFrame | null {
    return this.latest;
  }

  /** 退订并停止派发 */
  dispose(): void {
    this.disposed = true;
    this.unlisten?.();
    this.unlisten = null;
    this.latest = null;
  }
}
