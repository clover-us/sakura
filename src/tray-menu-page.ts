/**
 * 自绘托盘菜单的前端入口。
 *
 * ## 它与宿主的分工
 *
 * - **宿主**（`tray_menu.rs`）负责窗口：摆到托盘图标附近、夹进工作区、外点关闭、
 *   按页面报上来的尺寸重排；点击托盘图标时用 `eval` 调本页的 `window.__whalePetTrayMenu.show()`；
 * - **本页**负责内容：取状态（`get_tray_menu_state`）→ 画菜单 / 动作点播列表 →
 *   点一下就把动作交给宿主（`tray_menu_action`）→ 请宿主收起窗口（`close`）。
 *
 * ## 为什么不做"悬停级联子菜单"
 *
 * 100 多个动画摊成三级级联，在托盘这种小弹层里很难点准；这里改成
 * **展开成一个可滚动列表**（分类做小标题），动作名一目了然。
 * 因为窗口不可聚焦（不抢用户正在打字的窗口），输入框收不到键盘，
 * 所以也**不做搜索框**——想搜索就打开设置窗口。
 *
 * ## 为什么要自己 resize 窗口
 *
 * 主菜单是短列表（约 280px 高），动作点播展开后要高得多。窗口尺寸由宿主控制，
 * 页面量完内容高度后调 `resize_tray_menu` 报上去，宿主用**记住的锚点**重算位置。
 */
import { petLog, petLogError, setLogLabel } from './bridge/log.ts';
import { invoke } from './bridge/tauri.ts';

// ============================================================================
//  与宿主对接的类型
// ============================================================================

interface CategoryConfig {
  id: string;
  weight: number;
  actions: string[];
  noMirror?: boolean;
}
interface AnimationsConfig {
  idle: string[];
  turn: string[];
  drag: string[];
  clicks: string[];
  moves: { default: Record<string, unknown>; actions: Array<{ name: string }> };
  categories: CategoryConfig[];
  events?: Record<string, unknown>;
}
interface AnimationWeights {
  idle: number;
  turn: number;
  move: number;
}
interface TrayPet {
  label: string;
  name: string;
  visible: boolean;
  animations: AnimationsConfig;
  animationWeights: AnimationWeights;
  customBehaviour: boolean;
}
interface TrayMenuState {
  pets: TrayPet[];
  anyVisible: boolean;
  availableAnimations: string[];
}

/** 面板宽度（与 `tray_menu.rs` 的 `PANEL_W` 一致）；高度由内容决定后报给宿主 */
const PANEL_W = 272;

// ============================================================================
//  图标（内联 SVG：不引外部资源，CSP 友好，颜色跟随 currentColor）
// ============================================================================

const ICONS: Record<string, string> = {
  eye: '<path d="M1.6 8s2.4-4.2 6.4-4.2S14.4 8 14.4 8s-2.4 4.2-6.4 4.2S1.6 8 1.6 8z"/><circle cx="8" cy="8" r="1.9"/>',
  'eye-off':
    '<path d="M1.6 8s2.4-4.2 6.4-4.2c1.2 0 2.2.3 3.1.8M14.4 8s-2.4 4.2-6.4 4.2c-1.2 0-2.2-.3-3.1-.8"/><path d="M2.6 2.6l10.8 10.8"/>',
  home: '<path d="M3 8.4 8 4.2l5 4.2V13a.9.9 0 0 1-.9.9H3.9A.9.9 0 0 1 3 13z"/>',
  sparkles:
    '<path d="M8 2.2l1.3 3.4 3.4 1.3-3.4 1.3L8 11.6 6.7 8.2 3.3 6.9l3.4-1.3z"/><path d="M12.4 10.6l.6 1.5 1.5.6-1.5.6-.6 1.5-.6-1.5-1.5-.6 1.5-.6z"/>',
  gear:
    '<circle cx="8" cy="8" r="2.3"/><path d="M8 1.9v1.7M8 12.4v1.7M2.4 8h1.7M11.9 8h1.7M4 4l1.2 1.2M10.8 10.8L12 12M12 4l-1.2 1.2M5.2 10.8L4 12"/>',
  power: '<path d="M8 2.2v5"/><path d="M4.6 4.6a4.8 4.8 0 1 0 6.8 0"/>',
  chevron: '<path d="M6 3.5 10.5 8 6 12.5"/>',
};

function icon(name: string, size = 16): string {
  return `<svg class="ic" viewBox="0 0 16 16" width="${size}" height="${size}" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">${ICONS[name] ?? ''}</svg>`;
}

/** 面板左上角的小鲸鱼（与 exe 图标同一造型的简化版，用 SVG 手绘，省一张图片） */
const WHALE_LOGO = `
<svg viewBox="0 0 32 32" width="20" height="20" aria-hidden="true">
  <ellipse cx="13.5" cy="17" rx="8.4" ry="6.6" fill="#fff"/>
  <ellipse cx="24.5" cy="14.2" rx="4.6" ry="1.9" fill="#fff" transform="rotate(-19 24.5 14.2)"/>
  <ellipse cx="24.5" cy="19.6" rx="4.6" ry="1.9" fill="#fff" transform="rotate(19 24.5 19.6)"/>
  <ellipse cx="18.6" cy="17" rx="2.6" ry="1.8" fill="#fff"/>
  <circle cx="9.6" cy="15.6" r="1.5" fill="#23304a"/>
  <circle cx="9.6" cy="15.6" r="0.55" fill="#fff"/>
  <ellipse cx="7.4" cy="19.2" rx="1.9" ry="1.2" fill="#ff9ec0"/>
</svg>`;

// ============================================================================
//  页面状态
// ============================================================================

let state: TrayMenuState | null = null;
/** 动作点播当前选中的宠物下标 */
let pickerPet = 0;
let view: 'menu' | 'picker' = 'menu';
let toastTimer: number | null = null;

function panel(): HTMLElement {
  const node = document.getElementById('panel');
  if (!node) throw new Error('托盘菜单缺少 #panel');
  return node;
}

/** 把窗口高度调成"内容刚好放得下"（含四周阴影留边） */
async function fitWindow(): Promise<void> {
  const element = panel();
  const height = Math.ceil(element.getBoundingClientRect().height);
  try {
    await invoke<void>('resize_tray_menu', { width: PANEL_W, height });
  } catch (err) {
    petLogError('托盘菜单: 调整窗口尺寸失败', err);
  }
}

function setToast(message: string): void {
  const node = document.getElementById('toast');
  if (!node) return;
  node.textContent = message;
  node.hidden = false;
  if (toastTimer !== null) window.clearTimeout(toastTimer);
  toastTimer = window.setTimeout(() => {
    node.hidden = true;
  }, 1600);
}

async function act(action: string, options: { label?: string; anim?: string; close?: boolean } = {}): Promise<void> {
  try {
    await invoke<void>('tray_menu_action', {
      action,
      label: options.label ?? null,
      anim: options.anim ?? null,
    });
    petLog(`托盘菜单: 动作 ${action}${options.anim ? ` anim=${options.anim}` : ''}`);
  } catch (err) {
    setToast(`动作失败：${err instanceof Error ? err.message : String(err)}`);
    petLogError(`托盘菜单: 动作 ${action} 失败`, err);
    return;
  }
  if (options.close !== false) {
    await invoke<void>('tray_menu_action', { action: 'close', label: null, anim: null }).catch(() => undefined);
  }
}

// ============================================================================
//  渲染
// ============================================================================

function renderMenu(): void {
  const pets = state?.pets ?? [];
  const anyVisible = state?.anyVisible ?? false;
  const subtitle = pets.length === 1 ? `${pets[0]?.name || '宠物'} · 1 只` : `${pets.length} 只宠物`;
  panel().innerHTML = `
    <div class="view" id="view-menu">
      <div class="head">
        <div class="logo">${WHALE_LOGO}</div>
        <div class="head-text">
          <strong>whale-pet</strong>
          <span>${subtitle}</span>
        </div>
      </div>
      <div class="items">
        <button class="item" data-act="toggle">
          ${icon(anyVisible ? 'eye-off' : 'eye')}
          <span class="label">${anyVisible ? '隐藏宠物' : '显示宠物'}</span>
        </button>
        <button class="item" data-act="home">
          ${icon('home')}
          <span class="label">回到初始位置</span>
        </button>
        <button class="item" data-act="picker">
          ${icon('sparkles')}
          <span class="label">动作点播</span>
          <span class="chev">›</span>
        </button>
        <div class="sep"></div>
        <button class="item" data-act="settings">
          ${icon('gear')}
          <span class="label">设置…</span>
        </button>
        <button class="item danger" data-act="quit">
          ${icon('power')}
          <span class="label">退出</span>
        </button>
      </div>
      <div class="toast" id="toast" hidden></div>
    </div>`;

  panel()
    .querySelectorAll<HTMLButtonElement>('.item')
    .forEach((button) => {
      button.addEventListener('click', () => {
        const action = button.dataset.act ?? '';
        if (action === 'toggle') void act('toggle');
        else if (action === 'home') void act('home');
        else if (action === 'settings') void act('settings');
        else if (action === 'quit') void act('quit');
        else if (action === 'picker') showPicker(pickerPet);
      });
    });
  view = 'menu';
  void fitWindow();
}

function showPicker(petIndex: number): void {
  const pets = state?.pets ?? [];
  if (pets.length === 0) {
    setToast('还没有宠物');
    return;
  }
  pickerPet = Math.min(Math.max(petIndex, 0), pets.length - 1);
  const pet = pets[pickerPet];
  const animations = pet.animations;
  const available = new Set(state?.availableAnimations ?? []);

  // 分组顺序与设置窗口/右键菜单一致：待机、点击回应，然后各随机分类
  const groups: Array<{ title: string; actions: string[] }> = [
    { title: '待机', actions: animations.idle },
    { title: '点击回应', actions: animations.clicks },
    ...animations.categories.map((category) => ({ title: category.id, actions: category.actions })),
  ].filter((group) => group.actions.length > 0);

  const petChips =
    pets.length > 1
      ? `<div class="pets">${pets
          .map(
            (item, index) =>
              `<button class="chip" data-pet="${index}" data-active="${index === pickerPet}">${escapeHtml(item.name || item.label)}${
                item.customBehaviour ? ' ·自定义' : ''
              }</button>`,
          )
          .join('')}</div>`
      : '';

  const listHtml =
    groups.length === 0
      ? '<div class="empty">这只宠物的动画池是空的（去设置窗口添加动作）</div>'
      : groups
          .map(
            (group) => `
        <div class="group">${escapeHtml(group.title)}</div>
        ${group.actions
          .map(
            (name) => `<div class="anim" data-anim="${escapeAttr(name)}">
              <span class="label">${escapeHtml(name)}</span>
              ${available.has(name) ? '' : '<span class="missing">无素材</span>'}
            </div>`,
          )
          .join('')}`,
          )
          .join('');

  panel().innerHTML = `
    <div class="view" id="view-picker">
      <div class="picker-head">
        <button class="back" id="btn-back" title="返回">‹</button>
        <span class="picker-title">动作点播</span>
        ${pet.customBehaviour ? '<span class="badge" title="这只宠物有自己的一套动画池">自定义</span>' : ''}
      </div>
      ${petChips}
      <div class="list" id="anim-list">${listHtml}</div>
      <div class="toast" id="toast" hidden></div>
    </div>`;

  document.getElementById('btn-back')?.addEventListener('click', () => renderMenu());
  panel()
    .querySelectorAll<HTMLButtonElement>('.chip')
    .forEach((chip) => {
      chip.addEventListener('click', () => showPicker(Number(chip.dataset.pet ?? '0')));
    });
  panel()
    .querySelectorAll<HTMLDivElement>('.anim')
    .forEach((row) => {
      row.addEventListener('click', () => {
        const anim = row.dataset.anim ?? '';
        void act('anim', { anim, label: pet.label });
      });
    });
  view = 'picker';
  void fitWindow();
}

function escapeHtml(text: string): string {
  return text.replace(/[&<>"']/g, (ch) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[ch] as string);
}
function escapeAttr(text: string): string {
  return escapeHtml(text);
}

// ============================================================================
//  对外接口（宿主用 eval 调用）
// ============================================================================

interface TrayMenuApi {
  /** 每次弹出时调用：回到主菜单并刷新状态 */
  show: () => void;
}

declare global {
  interface Window {
    __whalePetTrayMenu?: TrayMenuApi;
  }
}

async function bootstrap(): Promise<void> {
  setLogLabel('tray-menu');
  window.__whalePetTrayMenu = {
    show: () => {
      // 弹出瞬间先用旧状态画一版（避免空白闪烁），随后拉新状态重画
      if (!state) renderMenu();
      void refresh();
    },
  };
  await refresh();
}

async function refresh(): Promise<void> {
  try {
    state = await invoke<TrayMenuState>('get_tray_menu_state');
    petLog(`托盘菜单: 状态已刷新（${state.pets.length} 只宠物，可见=${state.anyVisible}）`);
    renderMenu();
  } catch (err) {
    panel().innerHTML = `<div class="empty">读取状态失败：${escapeHtml(err instanceof Error ? err.message : String(err))}</div>`;
    petLogError('托盘菜单: 读取状态失败', err);
  }
}

void bootstrap();

// view 变量保留给调试（宿主日志与页面日志对照时能看出当前在哪一屏）
export { view };
