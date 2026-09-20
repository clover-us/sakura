/**
 * 对话输入窗的前端（M3）。
 *
 * ## 分工
 *
 * - **宿主**（`chat_window.rs`）负责窗口：摆在宠物右上角、夹进工作区、显示并**抢焦点**（要打字）、
 *   以及"带提示行时要加高"（`resize_chat`）；
 * - **本页**负责一条输入条：回车发送 → `llm_chat` → **失败原因留在框内**；
 *   成功则把输入清掉并请宿主收起窗口。
 *
 * ## 回复为什么不在这里显示
 *
 * 与上游一致：回复交给**宠物页的气泡**（`whisper::emit` 那条链路）。
 * 这里只显示"正在思考…"与错误——于是对话窗永远只有一行高，
 * 不会在桌面上摊开一块聊天记录挡住别的东西。
 *
 * ## 两条用户实测踩出来的规矩
 *
 * 1. **AI 没开启时必须当场说清楚**（见 `refreshAvailability`）。原来只在发送失败后
 *    把红字写进 `.status`，而那个元素**整个落在 54px 窗口之外**（被 `overflow:hidden` 裁掉），
 *    用户看到的就是"点发送毫无反应"。现在：开局自查 + 真的把窗口加高到能看见那一行。
 * 2. **摆弄输入条时要让宠物窗口让开**（见 `bindDrag` 的"输入闸门"）——否则用户把输入条
 *    拖到宠物身上时，宠物窗会**同时**收到这次拖动并跟着一起走。
 *
 * ## 一条踩过的坑
 *
 * 窗口是 `focusable(true)` 的**唯一**辅助窗，但页面在隐藏状态下不会被聚焦脚本叫醒；
 * 宿主每次 `open()` 都会调 `focusInput()`，这里只负责"清空上一次的输入 + 聚焦"，
 * **不要**在页面里自己决定要不要抢焦点（否则宠物窗那边的"绝不抢焦点"纪律会被悄悄破坏）。
 */
import { petLog, petLogError, setLogLabel } from './bridge/log.ts';
import { invoke, startDragging } from './bridge/tauri.ts';

/** 结构化失败（与 Rust `LlmErrorDto` 同构） */
interface LlmError {
  reason?: string;
  message?: string;
}

/** 提示行的最大字符数（与宿主 `chat_window::STATUS_MAX_CHARS` 对齐，超了由宿主裁剪） */
const STATUS_MAX_CHARS = 18;

/**
 * 输入闸门的续期间隔（毫秒）。
 *
 * 匀速续期：拖动期间定时器与鼠标事件都可能被系统拖动循环节流，余量留够，
 * "某一次续期没发出去"也不会中途掉闸。
 */
const GATE_KEEPALIVE_MS = 300;

/**
 * 上报给宿主的 `ttlMs`（毫秒）。
 *
 * 现在**闸门的生命周期不靠它**：收闸由宿主每 16ms 的全局按键看门狗判定
 * （`lib.rs::release_chat_input_if_button_up`），页面只负责"还在摆弄输入条"的心跳。
 * 这个值作为契约的一部分保留（宿主侧仍会夹上限），将来要加"心跳断了就自愈"的超时时会用到。
 */
const GATE_TTL_MS = 5000;

let sending = false;
/** 是否正在拖动输入条（拖动期间要持续占住"输入闸门"） */
let dragging = false;
/** 闸门续期定时器句柄（0 = 没在续） */
let gateTimer = 0;
/**
 * AI 是否可用（`chat_status` 自查的结果，null = 还没查出来）。
 *
 * 用途：**点发送之前就能给出确定答复**。没开 AI 时若还去发一次请求，
 * 用户要先愣几十毫秒才看到红字（体验上就是"没反应"）；这里直接拦下并说清去哪开。
 */
let aiReady: boolean | null = null;
/** AI 不可用时的提示语（由宿主给，前端不自己编文案） */
let unavailableHint = '';

function byId<T extends HTMLElement>(id: string): T {
  const node = document.getElementById(id);
  if (!node) throw new Error(`对话窗缺少元素 #${id}`);
  return node as T;
}

/** 当前宠物的窗口标签（宿主创建窗口时写进 URL） */
function petLabel(): string {
  return new URLSearchParams(window.location.search).get('label') ?? '';
}

/**
 * 写状态行。空字符串 = 收起那一行（并请宿主把窗口缩回一条胶囊）。
 *
 * `kind` 只有两种：`''` 是普通提示（灰），`'error'` 是失败原因（红）。
 *
 * 窗口高度只在**状态行的有无发生变化**时才请宿主调整（每次写都调一遍会让
 * "正在思考…"→失败原因这种连续两次写白跑一次 `set_size`+`set_position`）。
 */
let lastLayout = '';
function setStatus(kind: '' | 'error', text: string): void {
  const node = byId('status');
  const trimmed = clipStatus(text);
  node.className = kind === 'error' ? 'status error' : 'status';
  node.textContent = trimmed;
  node.hidden = trimmed.length === 0;
  // 布局上"有没有这一行"决定窗口高度（见 chat_window.rs 的 HEIGHT / HEIGHT_WITH_STATUS）
  const layout = trimmed.length === 0 ? 'bar' : 'status';
  if (layout === lastLayout) return;
  lastLayout = layout;
  void invoke<void>('resize_chat', { label: petLabel(), mode: layout }).catch((err: unknown) =>
    petLogError('对话: 调整窗口高度失败', err),
  );
}

/** 按"语义完整优先"截断提示行（不是让 CSS 把尾部的「设置 → AI」吃掉） */
function clipStatus(text: string): string {
  return text.length > STATUS_MAX_CHARS ? `${text.slice(0, STATUS_MAX_CHARS - 1)}…` : text;
}

/**
 * 开场自查：AI 现在能不能用。
 *
 * 为什么放在"打开发送框"这一刻而不是等用户点发送：
 *   用户点开输入框就是想说话，此时最该先知道的是"我现在到底能不能发"。
 *   不能发就把**为什么 + 去哪开**写在输入条下面（窗口会同时加高，保证看得见）。
 */
async function refreshAvailability(): Promise<void> {
  try {
    const hint = await invoke<string | null>('chat_status');
    unavailableHint = hint ?? '';
    aiReady = !hint;
    if (hint) {
      petLog(`对话: AI 暂不可用（${hint}）`);
      setStatus('error', hint);
    }
  } catch (err) {
    // 自查失败**不拦发送**：真发不出去的话，请求那一步会给出更准确的失败原因
    aiReady = null;
    petLogError('对话: AI 状态自查失败', err);
  }
}

/** 发送当前输入 */
async function send(): Promise<void> {
  if (sending) return;
  const input = byId<HTMLInputElement>('input');
  const text = input.value.trim();
  if (text.length === 0) {
    setStatus('error', '先写点什么再发吧');
    return;
  }
  if (text.length > 2000) {
    setStatus('error', '消息过长（限 2000 字）');
    return;
  }
  // 已知不可用就不发请求：直接给出"为什么 + 去哪开"（体验上从"没反应"变成"说清楚"）
  if (aiReady === false) {
    setStatus('error', unavailableHint || 'AI 还没开启（设置 → AI）');
    return;
  }

  const label = petLabel();
  sending = true;
  byId<HTMLButtonElement>('send').disabled = true;
  setStatus('', '正在思考…');
  const startedAt = performance.now();
  try {
    const reply = await invoke<string>('llm_chat', { label, text });
    petLog(`对话: 已回复（${Math.round(performance.now() - startedAt)}ms）：${reply}`);
    input.value = '';
    setStatus('', '');
    // 回复由宿主交给宠物页的气泡显示；这里把窗口收起来
    await invoke<void>('close_chat', { label }).catch(() => undefined);
  } catch (err) {
    const failure = err as LlmError;
    const reason = failure?.reason ?? 'unknown';
    const message = failure?.message ?? String(err);
    setStatus('error', `对话失败（${reason}）：${message}`);
    petLogError(`对话: 失败（${reason}）`, message);
    // 失败原因里已经写明"该去设置里开什么"，顺手把自查状态改掉：
    // 下一轮点发送就不必再白跑一次请求
    if (reason === 'disabled' || reason === 'no-key' || reason === 'no-model') {
      aiReady = false;
      unavailableHint = message;
    }
  } finally {
    sending = false;
    byId<HTMLButtonElement>('send').disabled = false;
    input.focus();
  }
}

/** 宿主每次打开时调用：清掉上一次的输入与错误、聚焦输入框、顺带自查 AI 是否可用 */
function focusInput(): void {
  const input = byId<HTMLInputElement>('input');
  input.value = '';
  setStatus('', '');
  input.focus();
  petLog('对话: 输入框已就绪');
  void refreshAvailability();
}

// ---------------------------------------------------------------------------
// 输入闸门：拖动输入条时，让宠物窗口整段让开鼠标
// ---------------------------------------------------------------------------

/**
 * 放行/续期闸门（`blocked = true`）或收闸（`blocked = false`）。
 *
 * **收闸的权威判据在宿主**（`lib.rs::release_chat_input_if_button_up`：每 16ms 查全局按键位，
 * 看到主键真的松开才收）——因为页面对"松手"的判断在系统拖动期间不可信（见 `bindDrag` 的复盘）。
 * 这里传 `blocked = false` 只是"页面自己确认松手"时的双保险（正常拖动路径）。
 *
 * 为什么这件事必须由对话窗发起：宠物页只看得到"光标在我身上"，它分不清
 * "用户在抓宠物"和"用户拖着输入条经过我"——而这个区别只有输入条这一侧知道。
 */
function setGate(blocked: boolean): void {
  void invoke<void>('mark_chat_input', {
    label: petLabel(),
    blocked,
    dragging,
    ttlMs: GATE_TTL_MS,
  }).catch((err: unknown) => petLogError('对话: 上报输入闸门失败', err));
}

/** 续期定时器：拖动期间每 300ms 续一次（系统拖动循环吞掉鼠标事件，只能靠定时器） */
function startGateKeepalive(): void {
  stopGateKeepalive();
  gateTimer = window.setInterval(() => setGate(true), GATE_KEEPALIVE_MS);
}

function stopGateKeepalive(): void {
  if (gateTimer !== 0) {
    window.clearInterval(gateTimer);
    gateTimer = 0;
  }
}

/**
 * 拖动输入条（用户要求：把输入框做成可拖动）。
 *
 * 只绑在**握柄**上：输入框里的按下必须留给文本选择、按钮必须能点，
 * 整条都能拖会导致"想选字却把窗口拖走了"。
 * 拖动本身交给系统的 `startDragging`（比自己算偏移更跟手，窗口移动时也不抖）。
 *
 * **同时要占住"输入闸门"**：拖动期间宠物窗口必须整段让开，否则用户把输入条拖到
 * 宠物身上时，下面的宠物窗会收到同一次拖动并跟着一起走（用户实测反馈的 bug）。
 * 闸门由宿主侧记账（带到期时间），这里只负责"按下时放行 + 期间续期 + 确认松手时收闸"。
 *
 * ## 实测教训：**不能用"第一个 buttons=0 的 mousemove"当松手证据**
 *
 * 第一版就是那么写的，结果闸门在按下后 **13ms** 就被自己关掉了（`pet-debug.log` 现场）：
 *
 * ```text
 * [chat] 对话: 握柄按下，交给系统拖动（已让宠物窗口让开鼠标）
 * [chat] 对话: 拖动结束，宠物窗口恢复命中判定      ← 13ms 后
 * [pet] 按下: 来源=sampler 指针=(367,79)          ← 闸门已关，宠物把这次拖动当成了"抓住我"
 * ```
 *
 * 原因是系统拖动那段模态循环里，页面会收到**按键位不可信的** mousemove（`buttons === 0`，
 * 尽管手指还按着）。现在的判据是"**先等 `RELEASE_DELAY_MS` 再看有没有新的按下证据**"：
 * 拖动进行中会不停有 `buttons > 0` 的移动（或干脆一个事件都没有）来取消这次收闸计划。
 */
function bindDrag(): void {
  const grip = byId('grip');
  grip.addEventListener('mousedown', (event) => {
    if (event.button !== 0) return;
    // 阻止默认行为：否则按住握柄会被当成开始选文字/拖拽 DOM
    event.preventDefault();
    dragging = true;
    setGate(true);
    startGateKeepalive();
    // 日志带上按键位：拖动"没反应"时，先要能分清是"没点到握柄"还是"点了但系统没接管"
    petLog(`对话: 握柄按下（buttons=${event.buttons}），交给系统拖动（已让宠物窗口让开鼠标）`);
    void startDragging().catch((err: unknown) => petLogError('对话: 拖动失败', err));
  });
  // 松手就收闸：宠物恢复正常命中判定（时间上是一次 IPC，肉眼无感）
  window.addEventListener('mouseup', (event) => requestRelease('mouseup', event.buttons));
  window.addEventListener('mousemove', (event) => {
    if (!dragging) return;
    if (event.buttons > 0) {
      // 明确的"还按着"证据：取消待执行的收闸计划并续期
      cancelPendingRelease();
      setGate(true);
      return;
    }
    // 系统拖动期间这条分支会被触发，**不能立刻收闸**（见函数注释的实测复盘）
    requestRelease('mousemove(buttons=0)', event.buttons);
  });
  // 失焦兜底：拖动中被抢焦点/窗口被隐藏时鼠标抬起事件可能永远不来
  window.addEventListener('blur', () => requestRelease('blur', -1));
  // 窗口被隐藏（Esc/发送成功/托盘收起）同样要收闸——这条是"宿主 close 收闸"之外的双保险
  document.addEventListener('visibilitychange', () => {
    if (document.visibilityState === 'hidden') endDrag('visibilitychange');
  });
}

/** 松手确认的等待时长（毫秒）：见 `bindDrag` 的实测复盘 */
const RELEASE_DELAY_MS = 250;
/** 待执行的收闸计划（句柄；0 = 没有） */
let releaseTimer = 0;

/**
 * 请求收闸：不是立刻关，而是等 `RELEASE_DELAY_MS` 再确认——
 * 这段时间里只要出现"还按着"的证据（`buttons > 0` 的移动）就会被取消。
 */
function requestRelease(reason: string, buttons: number): void {
  if (!dragging || releaseTimer !== 0) return;
  petLog(`对话: 疑似松手（${reason} buttons=${buttons}），${RELEASE_DELAY_MS}ms 后确认`);
  releaseTimer = window.setTimeout(() => {
    releaseTimer = 0;
    endDrag(reason);
  }, RELEASE_DELAY_MS);
}

/** 取消待执行的收闸计划（有新的"还按着"证据时调用） */
function cancelPendingRelease(): void {
  if (releaseTimer === 0) return;
  window.clearTimeout(releaseTimer);
  releaseTimer = 0;
  petLog('对话: 取消收闸（又收到按键仍按下的移动）');
}

/** 结束拖动：收闸 + 停掉续期（可重复调用） */
function endDrag(reason: string): void {
  cancelPendingRelease();
  if (!dragging) return;
  dragging = false;
  stopGateKeepalive();
  setGate(false);
  petLog(`对话: 拖动结束（${reason}），宠物窗口恢复命中判定`);
}

declare global {
  interface Window {
    __whalePetChat?: { focusInput: () => void };
  }
}

function bootstrap(): void {
  setLogLabel('chat');
  const input = byId<HTMLInputElement>('input');
  input.addEventListener('keydown', (event) => {
    if (event.key === 'Enter' && !event.shiftKey) {
      event.preventDefault();
      void send();
      return;
    }
    if (event.key === 'Escape') {
      event.preventDefault();
      void invoke<void>('close_chat', { label: petLabel() }).catch(() => undefined);
    }
  });
  byId('send').addEventListener('click', () => void send());
  bindDrag();
  window.__whalePetChat = { focusInput };
  petLog('对话: 页面就绪');
}

bootstrap();
