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
 * ## 动作点播为什么是"分类折叠 + 点击展开"
 *
 * 100 多个动画一次全铺出来会把弹层撑得很长，找一条要滚很久。现在默认**只列分类**，
 * 点哪个展开哪个（右侧显示条数），另有一个「全部展开/收起」的开关。
 * 因为窗口不可聚焦（不抢用户正在打字的窗口），输入框收不到键盘，所以**不做搜索框**——
 * 想搜索就打开设置窗口。
 *
 * ## 为什么要自己 resize 窗口
 *
 * 主菜单是短列表，动作点播展开后高度差别很大。窗口尺寸由宿主控制，
 * 页面量完内容高度后调 `resize_tray_menu` 报上去，宿主用**记住的锚点**重算位置。
 */
import { petLog, petLogError, setLogLabel } from './bridge/log.ts';
import { invoke } from './bridge/tauri.ts';
// 应用图标（用户给的原子图标）：与设置窗侧栏、exe、任务栏是**同一张图**，
// 由 `cargo run --example make-icon` 产出（见 src-tauri/icons/design/）
import appLogoUrl from './assets/app-logo.png';

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
//  图标
// ============================================================================

/**
 * 图标形状取自 **Lucide**（https://lucide.dev，ISC 许可）：统一的 24×24 线性风格、
 * 2px 圆头笔画。之前那套是手画的 16×16 路径，粗细不匀、齿轮还画成了"太阳"，
 * 用户直接点名"图标丑"——这类基础图形没必要自己造。
 *
 * 只把**用得上的形状**内联进代码（不引依赖、不联网、CSP 友好），颜色跟随 currentColor。
 */
const ICONS: Record<string, string> = {
  eye: '<path d="M2.062 12.348a1 1 0 0 1 0-.696 10.75 10.75 0 0 1 19.876 0 1 1 0 0 1 0 .696 10.75 10.75 0 0 1-19.876 0"/><circle cx="12" cy="12" r="3"/>',
  'eye-off':
    '<path d="M10.733 5.076a10.744 10.744 0 0 1 11.205 6.575 1 1 0 0 1 0 .696 10.747 10.747 0 0 1-1.444 2.49"/><path d="M14.084 14.158a3 3 0 0 1-4.242-4.242"/><path d="M17.479 17.499a10.75 10.75 0 0 1-15.417-5.151 1 1 0 0 1 0-.696 10.75 10.75 0 0 1 4.446-5.143"/><path d="m2 2 20 20"/>',
  house:
    '<path d="M15 21v-8a1 1 0 0 0-1-1h-4a1 1 0 0 0-1 1v8"/><path d="M3 10a2 2 0 0 1 .709-1.528l7-6a2 2 0 0 1 2.582 0l7 6A2 2 0 0 1 21 10v9a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"/>',
  sparkles:
    '<path d="M11.017 2.814a1 1 0 0 1 1.966 0l1.051 5.558a2 2 0 0 0 1.594 1.594l5.558 1.051a1 1 0 0 1 0 1.966l-5.558 1.051a2 2 0 0 0-1.594 1.594l-1.051 5.558a1 1 0 0 1-1.966 0l-1.051-5.558a2 2 0 0 0-1.594-1.594l-5.558-1.051a1 1 0 0 1 0-1.966l5.558-1.051a2 2 0 0 0 1.594-1.594z"/><path d="M20 2v4"/><path d="M22 4h-4"/><circle cx="4" cy="20" r="2"/>',
  settings:
    '<path d="M9.671 4.136a2.34 2.34 0 0 1 4.659 0 2.34 2.34 0 0 0 3.319 1.915 2.34 2.34 0 0 1 2.33 4.033 2.34 2.34 0 0 0 0 3.831 2.34 2.34 0 0 1-2.33 4.033 2.34 2.34 0 0 0-3.319 1.915 2.34 2.34 0 0 1-4.659 0 2.34 2.34 0 0 0-3.32-1.915 2.34 2.34 0 0 1-2.33-4.033 2.34 2.34 0 0 0 0-3.831A2.34 2.34 0 0 1 6.35 6.051a2.34 2.34 0 0 0 3.319-1.915"/><circle cx="12" cy="12" r="3"/>',
  power: '<path d="M12 2v10"/><path d="M18.4 6.6a9 9 0 1 1-12.77.04"/>',
  'message-circle':
    '<path d="M2.992 16.342a2 2 0 0 1 .094 1.167l-1.065 3.29a1 1 0 0 0 1.236 1.168l3.413-.998a2 2 0 0 1 1.099.092 10 10 0 1 0-4.777-4.719"/>',
  wallet:
    '<path d="M19 7V4a1 1 0 0 0-1-1H5a2 2 0 0 0 0 4h15a1 1 0 0 1 1 1v4h-3a2 2 0 0 0 0 4h3a1 1 0 0 0 1-1v-2a1 1 0 0 0-1-1"/><path d="M3 5v14a2 2 0 0 0 2 2h15a1 1 0 0 0 1-1v-4"/>',
  chevron: '<path d="m9 18 6-6-6-6"/>',
};

function icon(name: string, size = 16): string {
  return `<svg class="ic" viewBox="0 0 24 24" width="${size}" height="${size}" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">${ICONS[name] ?? ''}</svg>`;
}

/**
 * 应用图标的小标记（页头）。
 *
 * 图形源是 `src-tauri/icons/design/app-icon.png`（用户给的原子图标），
 * `cargo run --example make-icon` 把同一张图导出成 `src/assets/app-logo.png`，页面引用它。
 * 以前这里手写内联 SVG（旧版小鲸鱼），必须靠"改图标时记得两处同步"的纪律对齐——
 * 新图标是位图素材（用户只给了 PNG），手抄必然走样，索性直接引用同一份产物。
 */
function appLogo(size: number): string {
  return `<img class="app-logo" src="${appLogoUrl}" width="${size}" height="${size}" alt="" draggable="false" />`;
}

// ============================================================================
//  页面状态
// ============================================================================

let state: TrayMenuState | null = null;
/** 动作点播当前选中的宠物下标 */
let pickerPet = 0;
/** 已展开的分类（**默认空集合 = 全部收起**，用户点开哪个才展开哪个） */
const expandedGroups = new Set<string>();
let view: 'menu' | 'picker' = 'menu';
let toastTimer: number | null = null;

/** toast 停留时长；到点要连窗口一起缩回去，所以单独提出来（见 `setToast`） */
const TOAST_MS = 1600;

function panel(): HTMLElement {
  const node = document.getElementById('panel');
  if (!node) throw new Error('托盘菜单缺少 #panel');
  return node;
}

/**
 * 尺寸调整**串行**执行：`resize_tray_menu` 会按锚点重排整个窗口，
 * 两次调用交错时会互相覆盖（后量到的高度可能被先发出的那次调用盖回去）。
 * toast 出现时最容易撞上——它紧跟在"渲染完量一次"之后。
 */
let fitQueue: Promise<void> = Promise.resolve();

/** 把窗口高度调成"内容刚好放得下"（宿主会按记住的锚点重算位置） */
function fitWindow(): Promise<void> {
  const measured = fitQueue.then(async () => {
    const height = Math.ceil(panel().getBoundingClientRect().height);
    await invoke<void>('resize_tray_menu', { width: PANEL_W, height });
  });
  fitQueue = measured.catch((err) => {
    petLogError('托盘菜单: 调整窗口尺寸失败', err);
  });
  return fitQueue;
}

/**
 * 弹一条提示（动作失败的原因、或"还没有宠物"）。
 *
 * **必须重新量一次窗口高度**：窗口是弹出那一刻按内容量好的，而 toast 是**事后**
 * 才长出来的一行（失败提示还会折成两行）——不重量它就会顶出窗口下边缘，
 * 被窗口裁掉一半（用户截图原话："下面的提示文字显示不全"）。
 * 收起时再量一次缩回去：否则墙上留一片看不见的空白，把桌面的点击吃掉。
 */
function setToast(message: string): void {
  const node = document.getElementById('toast');
  if (!node) return;
  node.textContent = message;
  node.hidden = false;
  void fitWindow();
  if (toastTimer !== null) window.clearTimeout(toastTimer);
  toastTimer = window.setTimeout(() => {
    node.hidden = true;
    void fitWindow();
  }, TOAST_MS);
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
        <div class="logo">${appLogo(26)}</div>
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
          ${icon('house')}
          <span class="label">回到初始位置</span>
        </button>
        <button class="item" data-act="picker">
          ${icon('sparkles')}
          <span class="label">动作点播</span>
          <span class="chev">${icon('chevron', 14)}</span>
        </button>
        <button class="item" data-act="chat">
          ${icon('message-circle')}
          <span class="label">说两句…</span>
        </button>
        <button class="item" data-act="balance">
          ${icon('wallet')}
          <span class="label">查余额</span>
        </button>
        <div class="sep"></div>
        <button class="item" data-act="settings">
          ${icon('settings')}
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
        else if (action === 'chat') void act('chat');
        else if (action === 'balance') void act('balance');
        else if (action === 'settings') void act('settings');
        else if (action === 'quit') void act('quit');
        else if (action === 'picker') showPicker(pickerPet);
      });
    });
  view = 'menu';
  void fitWindow();
}

/** 动作分组：待机 / 点击回应 / 各随机分类（与设置窗口、右键菜单同一套分法） */
function groupsFor(pet: TrayPet): Array<{ title: string; actions: string[] }> {
  const animations = pet.animations;
  return [
    { title: '待机', actions: animations.idle },
    { title: '点击回应', actions: animations.clicks },
    ...animations.categories.map((category) => ({ title: category.id, actions: category.actions })),
  ].filter((group) => group.actions.length > 0);
}

function showPicker(petIndex: number): void {
  const pets = state?.pets ?? [];
  if (pets.length === 0) {
    setToast('还没有宠物');
    return;
  }
  pickerPet = Math.min(Math.max(petIndex, 0), pets.length - 1);
  const pet = pets[pickerPet];
  const available = new Set(state?.availableAnimations ?? []);
  const groups = groupsFor(pet);
  const allExpanded = groups.length > 0 && groups.every((group) => expandedGroups.has(group.title));

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
          .map((group) => {
            // 折叠态：标题行（chevron + 名称 + 条数），点了才展开具体动作
            const open = expandedGroups.has(group.title);
            const missing = group.actions.filter((name) => !available.has(name)).length;
            return `
        <div class="group" data-open="${open}">
          <button class="group-head" data-group="${escapeAttr(group.title)}">
            <span class="chev">${icon('chevron', 14)}</span>
            <span class="label">${escapeHtml(group.title)}</span>
            <span class="count">${group.actions.length}</span>
            ${missing > 0 ? `<span class="missing">${missing} 无素材</span>` : ''}
          </button>
          ${
            open
              ? `<div class="group-body">${group.actions
                  .map(
                    (name) => `<div class="anim" data-anim="${escapeAttr(name)}">
                      <span class="label">${escapeHtml(name)}</span>
                    </div>`,
                  )
                  .join('')}</div>`
              : ''
          }
        </div>`;
          })
          .join('');

  panel().innerHTML = `
    <div class="view" id="view-picker">
      <div class="picker-head">
        <button class="back" id="btn-back" title="返回">‹</button>
        <span class="picker-title">动作点播</span>
        ${
          groups.length > 0
            ? `<button class="expand-all" id="btn-expand-all">${allExpanded ? '全部收起' : '全部展开'}</button>`
            : ''
        }
        ${pet.customBehaviour ? '<span class="badge" title="这只宠物有自己的一套动画池">自定义</span>' : ''}
      </div>
      ${petChips}
      <div class="list" id="anim-list">${listHtml}</div>
      <div class="toast" id="toast" hidden></div>
    </div>`;

  document.getElementById('btn-back')?.addEventListener('click', () => renderMenu());
  document.getElementById('btn-expand-all')?.addEventListener('click', () => {
    if (allExpanded) expandedGroups.clear();
    else groups.forEach((group) => expandedGroups.add(group.title));
    showPicker(pickerPet);
  });
  panel()
    .querySelectorAll<HTMLButtonElement>('.chip')
    .forEach((chip) => {
      chip.addEventListener('click', () => showPicker(Number(chip.dataset.pet ?? '0')));
    });
  panel()
    .querySelectorAll<HTMLButtonElement>('.group-head')
    .forEach((head) => {
      head.addEventListener('click', () => {
        const title = head.dataset.group ?? '';
        if (expandedGroups.has(title)) expandedGroups.delete(title);
        else expandedGroups.add(title);
        showPicker(pickerPet);
      });
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

/** 显式主题覆盖（只给自检用：`?theme=dark`；深色跟随系统，本机桌面是浅色） */
function applyThemeOverride(): void {
  const theme = new URLSearchParams(window.location.search).get('theme');
  if (theme === 'dark' || theme === 'light') {
    document.documentElement.dataset.theme = theme;
  }
}

async function bootstrap(): Promise<void> {
  applyThemeOverride();
  setLogLabel('tray-menu');
  window.__whalePetTrayMenu = {
    show: () => {
      // 每次弹出都回到"主菜单 + 分类全收起"的干净状态
      expandedGroups.clear();
      if (!state) renderMenu();
      void refresh();
    },
  };
  await refresh();
}

async function refresh(): Promise<void> {
  try {
    state = await invoke<TrayMenuState>('get_tray_menu_state');
    petLog(
      // 记下**实际生效**的主题：深色跟随系统，而"强制覆盖"是给自检用的（?theme=dark）
      `托盘菜单: 状态已刷新（${state.pets.length} 只宠物，可见=${state.anyVisible}；主题=${
        document.documentElement.dataset.theme ??
        (window.matchMedia('(prefers-color-scheme: dark)').matches ? 'dark(系统)' : 'light(系统)')
      }）`,
    );
    renderMenu();
  } catch (err) {
    panel().innerHTML = `<div class="empty">读取状态失败：${escapeHtml(err instanceof Error ? err.message : String(err))}</div>`;
    petLogError('托盘菜单: 读取状态失败', err);
  }
}

void bootstrap();

// view 变量保留给调试（宿主日志与页面日志对照时能看出当前在哪一屏）
export { view };
