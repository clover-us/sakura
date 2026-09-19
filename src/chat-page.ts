/**
 * 对话输入窗的前端（M3）。
 *
 * ## 分工
 *
 * - **宿主**（`chat_window.rs`）负责窗口：摆在宠物右上角、夹进工作区、显示并**抢焦点**（要打字）；
 * - **本页**负责一条输入条：回车发送 → `llm_chat` → **失败红字留在框内**；
 *   成功则把输入清掉并请宿主收起窗口。
 *
 * ## 回复为什么不在这里显示
 *
 * 与上游一致：回复交给**宠物页的气泡**（`whisper::emit` 那条链路）。
 * 这里只显示"正在思考…"与错误——于是对话窗永远只有一行高，
 * 不会在桌面上摊开一块聊天记录挡住别的东西。
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

let sending = false;

function byId<T extends HTMLElement>(id: string): T {
  const node = document.getElementById(id);
  if (!node) throw new Error(`对话窗缺少元素 #${id}`);
  return node as T;
}

function setStatus(kind: '' | 'error', text: string): void {
  const node = byId('status');
  node.className = kind === 'error' ? 'status error' : 'status';
  node.textContent = text;
  node.hidden = text.length === 0;
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

  const params = new URLSearchParams(window.location.search);
  const label = params.get('label') ?? '';

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
  } finally {
    sending = false;
    byId<HTMLButtonElement>('send').disabled = false;
    input.focus();
  }
}

/** 宿主每次打开时调用：清掉上一次的输入与错误、聚焦输入框 */
function focusInput(): void {
  const input = byId<HTMLInputElement>('input');
  input.value = '';
  setStatus('', '');
  input.focus();
  petLog('对话: 输入框已就绪');
}

/**
 * 拖动输入条（用户要求：把输入框做成可拖动）。
 *
 * 只绑在**握柄**上：输入框里的按下必须留给文本选择、按钮必须能点，
 * 整条都能拖会导致"想选字却把窗口拖走了"。
 * 拖动本身交给系统的 `startDragging`（比自己算偏移更跟手，窗口移动时也不抖）。
 */
function bindDrag(): void {
  const grip = byId('grip');
  grip.addEventListener('mousedown', (event) => {
    if (event.button !== 0) return;
    // 阻止默认行为：否则按住握柄会被当成开始选文字/拖拽 DOM
    event.preventDefault();
    // 留一行日志：拖动"没反应"时，先要能分清是"没点到握柄"还是"点了但系统没接管"
    petLog('对话: 握柄按下，交给系统拖动');
    void startDragging().catch((err: unknown) => petLogError('对话: 拖动失败', err));
  });
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
      const label = new URLSearchParams(window.location.search).get('label') ?? '';
      void invoke<void>('close_chat', { label }).catch(() => undefined);
    }
  });
  byId('send').addEventListener('click', () => void send());
  bindDrag();
  window.__whalePetChat = { focusInput };
  petLog('对话: 页面就绪');
}

bootstrap();
