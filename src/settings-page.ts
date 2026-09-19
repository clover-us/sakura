/**
 * 设置窗口的前端（M2 外观升级版：左侧导航 + 每只宠物独立页面 + 跟随系统深浅色）。
 *
 * ## 这一版解决了什么
 *
 * 旧版是"一长条表单往下滚"，用户的原话是"不好看、很杂乱、没有导航栏"。具体问题有三类：
 *   1. **没有信息层级**：宠物、物理、动画池挤在同一个滚动流里；
 *   2. **布局 bug**：勾选项被塞进窄网格单元，中文被挤成一列一个字（截图里一眼能看到）；
 *   3. **行为归属不清**：动画池是全局的，但用户在直觉上认为"每个宠物该有自己的行为"。
 *
 * 现在：左侧竖导航（宠物逐个列出来 + 通用分组），右侧一次只渲染**一个页面**，
 * 底部常驻状态栏与保存按钮；宠物页里有"行为：跟随全局 / 单独设置"的显式选择。
 *
 * ## 数据流（仍然是"整份配置进出"）
 *
 * ```text
 * get_settings ─► AppConfig（原样持有，只改用得上的字段，其余原封带回）
 *                     ▼
 * save_settings(config) ─► 校验 → 备份 + 原子写 → 立即重建宠物窗
 * ```
 *
 * 不给每个字段建映射表、也不逐字段回传：Rust 侧加字段这一页不用跟着改，
 * 用户手改过的冷门字段（events、moves 的 params）也不会被抹掉。
 *
 * 校验只在 Rust 侧（`AppConfig::validate`）：前端不重复实现一套规则，
 * 否则两边迟早不一致。
 */
import { petLog, petLogError, setLogLabel } from './bridge/log.ts';
import { invoke } from './bridge/tauri.ts';

// ============================================================================
//  与 Rust 侧同构的类型（只声明本页读写得到的字段）
// ============================================================================

type Corner = 'top-left' | 'top-right' | 'bottom-left' | 'bottom-right';

interface PositionConfig {
  corner: Corner;
  marginX: number;
  marginY: number;
}

interface AnimationsConfig {
  idle: string[];
  turn: string[];
  drag: string[];
  clicks: string[];
  moves: MovesConfig;
  categories: CategoryConfig[];
  /** 事件动画：本页不改（原样带回） */
  events?: Record<string, unknown>;
}

interface AnimationWeights {
  idle: number;
  turn: number;
  move: number;
}

interface PetEntry {
  id: string;
  name: string;
  size: number;
  idle: string;
  click: string;
  position: PositionConfig;
  /** 单独覆盖动画池（不设 = 跟随全局） */
  animations?: AnimationsConfig | null;
  /** 单独覆盖动画链权重（不设 = 跟随全局） */
  animationWeights?: AnimationWeights | null;
}

interface PhysicsParams {
  gravity: number;
  restitution: number;
  groundFriction: number;
  ceilingBounce: boolean;
  throwPower: number;
  petCollision: boolean;
}

interface MoveSpec {
  name: string;
  params?: Record<string, unknown> | null;
}

interface MovesConfig {
  default: Record<string, unknown>;
  actions: MoveSpec[];
}

interface CategoryConfig {
  id: string;
  weight: number;
  actions: string[];
  noMirror?: boolean;
}

interface AppConfig {
  schemaVersion: number;
  physics: PhysicsParams;
  pets: PetEntry[];
  animations: AnimationsConfig;
  animationWeights: AnimationWeights;
}

interface SettingsDto {
  config: AppConfig;
  configPath: string;
  appDataDir: string;
  availableAnimations: string[];
  autostart: boolean;
}

interface SaveSettingsDto {
  path: string;
  backup: string | null;
  warnings: string[];
  petCount: number;
}

// ============================================================================
//  状态
// ============================================================================

let config: AppConfig | null = null;
let available: string[] = [];
let loaded = false;
let autostart = false;
let appDataDir = '';
let configPath = '';
/** 当前视图：`pet:<下标>` / `physics` / `animations` / `system` / `about` */
let view = 'pet:0';
let dirty = false;

const CORNERS: Array<{ value: Corner; label: string }> = [
  { value: 'top-left', label: '左上' },
  { value: 'top-right', label: '右上' },
  { value: 'bottom-left', label: '左下' },
  { value: 'bottom-right', label: '右下' },
];

/**
 * 图标形状取自 **Lucide**（https://lucide.dev，ISC 许可）：统一的 24×24 线性风格、2px 圆头笔画。
 * （之前那套 16×16 手画路径粗细不匀、齿轮画成了"太阳"，被用户点名"图标丑"。）
 * 只把用得到的形状内联进来：不引依赖、不联网、CSP 友好，颜色跟随 currentColor。
 */
const ICONS: Record<string, string> = {
  'paw-print':
    '<circle cx="11" cy="4" r="2"/><circle cx="18" cy="8" r="2"/><circle cx="20" cy="16" r="2"/><path d="M9 10a5 5 0 0 1 5 5v3.5a3.5 3.5 0 0 1-6.84 1.045Q6.52 17.48 4.46 16.84A3.5 3.5 0 0 1 5.5 10Z"/>',
  'sliders-horizontal':
    '<path d="M10 5H3"/><path d="M12 19H3"/><path d="M14 3v4"/><path d="M16 17v4"/><path d="M21 12h-9"/><path d="M21 19h-5"/><path d="M21 5h-7"/><path d="M8 10v4"/><path d="M8 12H3"/>',
  clapperboard:
    '<path d="M20.2 6 3 11l-.9-2.4c-.3-1.1.3-2.2 1.3-2.5l13.5-4c1.1-.3 2.2.3 2.5 1.3Z"/><path d="m6.2 5.3 3.1 3.9"/><path d="m12.4 3.4 3.1 4"/><path d="M3 11h18v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2Z"/>',
  rocket:
    '<path d="M4.5 16.5c-1.5 1.26-2 5-2 5s3.74-.5 5-2c.71-.84.7-2.13-.09-2.91a2.18 2.18 0 0 0-2.91-.09z"/><path d="m12 15-3-3a22 22 0 0 1 2-3.95A12.88 12.88 0 0 1 22 2c0 2.72-.78 7.5-6 11a22.35 22.35 0 0 1-4 2z"/><path d="M9 12H4s.55-3.03 2-4c1.62-1.08 5 0 5 0"/><path d="M12 15v5s3.03-.55 4-2c1.08-1.62 0-5 0-5"/>',
  info: '<circle cx="12" cy="12" r="10"/><path d="M12 16v-4"/><path d="M12 8h.01"/>',
  plus: '<path d="M5 12h14"/><path d="M12 5v14"/>',
  x: '<path d="M18 6 6 18"/><path d="m6 6 12 12"/>',
  'folder-open':
    '<path d="m6 14 1.5-2.9A2 2 0 0 1 9.24 10H20a2 2 0 0 1 1.94 2.5l-1.54 6a2 2 0 0 1-1.95 1.5H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h3.9a2 2 0 0 1 1.69.9l.81 1.2a2 2 0 0 0 1.67.9H18a2 2 0 0 1 2 2v2"/>',
  'rotate-ccw': '<path d="M3 12a9 9 0 1 0 9-9 9.75 9.75 0 0 0-6.74 2.74L3 8"/><path d="M3 3v5h5"/>',
  save: '<path d="M15.2 3a2 2 0 0 1 1.4.6l3.8 3.8a2 2 0 0 1 .6 1.4V19a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2z"/><path d="M17 21v-7a1 1 0 0 0-1-1H8a1 1 0 0 0-1 1v7"/><path d="M7 3v4a1 1 0 0 0 1 1h7"/>',
};

/** 内联 SVG 图标（`currentColor` 描边，尺寸由 CSS 控制） */
function icon(name: string, size = 15): string {
  return `<svg class="ic" viewBox="0 0 24 24" width="${size}" height="${size}" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">${ICONS[name] ?? ''}</svg>`;
}

/**
 * 应用图标的小尺寸内联版（与 `icons/design/app-icon.svg` 同一造型：青绿圆角底 +
 * 趴在横条上的小生物）。
 *
 * 为什么不直接引用那个 SVG 文件：应用图标是"一份矢量源 + 预生成产物"，
 * 而页面只需要一个 22px 的小标记；内联一份简化版最省事，也不必让 webview 去请求图标文件。
 * **但它必须跟着图标一起改**——早先这里留的是旧版粉色小鲸鱼，用户截图圈出来说"这两处都没改"。
 */
const PET_LOGO = `
<svg viewBox="0 0 32 32" width="22" height="22" aria-hidden="true">
  <rect x="4.4" y="21" width="23.2" height="6" rx="3" fill="#fff" opacity="0.96"/>
  <circle cx="8.6" cy="24" r="1.1" fill="#9BE0D8"/>
  <circle cx="12.2" cy="24" r="1.1" fill="#FFD9A8"/>
  <circle cx="15.8" cy="24" r="1.1" fill="#BBD7FF"/>
  <path d="M11.4 11.6c.8-3 2.6-3.8 3.6-1.8l1 2.1Z" fill="#fff"/>
  <path d="M20.6 11.6c-.8-3-2.6-3.8-3.6-1.8l-1 2.1Z" fill="#fff"/>
  <ellipse cx="10.6" cy="19.4" rx="2.7" ry="1.6" fill="#fff" transform="rotate(-14 10.6 19.4)"/>
  <ellipse cx="21.4" cy="19.4" rx="2.7" ry="1.6" fill="#fff" transform="rotate(14 21.4 19.4)"/>
  <path d="M16 8.4c5 0 7.8 3.4 7.8 6.5 0 3.3-3.4 5-7.8 5s-7.8-1.7-7.8-5c0-3.1 2.8-6.5 7.8-6.5Z" fill="#fff"/>
  <ellipse cx="13.7" cy="14.3" rx="1.15" ry="1.3" fill="#22384A"/>
  <ellipse cx="18.3" cy="14.3" rx="1.15" ry="1.3" fill="#22384A"/>
  <ellipse cx="11.4" cy="17.2" rx="1.4" ry="0.85" fill="#FF9EC0"/>
  <ellipse cx="20.6" cy="17.2" rx="1.4" ry="0.85" fill="#FF9EC0"/>
  <path d="M14.8 17.3c.5.9 2 .9 2.5 0" fill="none" stroke="#22384A" stroke-width="0.9" stroke-linecap="round"/>
</svg>`;

// ============================================================================
//  DOM 工具
// ============================================================================

function byId<T extends HTMLElement>(id: string): T {
  const node = document.getElementById(id);
  if (!node) throw new Error(`设置页缺少元素 #${id}（HTML 与脚本版本不一致？）`);
  return node as T;
}

function el<K extends keyof HTMLElementTagNameMap>(
  tag: K,
  className?: string,
  text?: string,
): HTMLElementTagNameMap[K] {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text !== undefined) node.textContent = text;
  return node;
}

function field(labelText: string, control: HTMLElement, hint?: string): HTMLElement {
  const wrap = el('label', 'field');
  wrap.appendChild(el('span', undefined, labelText));
  wrap.appendChild(control);
  // 说明位**恒定存在**（没有就留空）：这样同一行里每个字段的结构一致，
  // 输入框的纵向位置不会被"某个字段多一行说明"顶歪
  wrap.appendChild(el('span', 'inline-hint', hint ?? ''));
  return wrap;
}

function card(title: string, desc?: string): HTMLElement {
  const node = el('section', 'card');
  node.appendChild(el('h3', undefined, title));
  if (desc) node.appendChild(el('p', 'desc', desc));
  return node;
}

function button(text: string, className?: string, onClick?: () => void, iconName?: string): HTMLButtonElement {
  const node = el('button', className);
  node.type = 'button';
  if (iconName) {
    const glyph = el('span', 'ic');
    glyph.innerHTML = icon(iconName, 14);
    node.appendChild(glyph);
  }
  if (text) node.appendChild(el('span', undefined, text));
  if (onClick) node.addEventListener('click', onClick);
  return node;
}

function textInput(value: string, onChange: (next: string) => void, withSuggestions = false, placeholder?: string): HTMLInputElement {
  const input = el('input');
  input.type = 'text';
  input.value = value;
  if (placeholder) input.placeholder = placeholder;
  if (withSuggestions) input.setAttribute('list', 'anim-options');
  input.addEventListener('input', () => {
    onChange(input.value);
    markDirty();
  });
  return input;
}

function numberInput(
  value: number,
  onChange: (next: number) => void,
  options: { min?: number; max?: number; step?: number; integer?: boolean } = {},
): HTMLInputElement {
  const input = el('input');
  input.type = 'number';
  input.value = String(value);
  if (options.min !== undefined) input.min = String(options.min);
  if (options.max !== undefined) input.max = String(options.max);
  input.step = String(options.step ?? (options.integer ? 1 : 'any'));
  input.addEventListener('input', () => {
    const parsed = Number(input.value);
    // 空串/半截输入不写回：否则用户删到一半就被塞进 0，光标还会跳
    if (!Number.isFinite(parsed)) return;
    onChange(options.integer ? Math.round(parsed) : parsed);
    markDirty();
  });
  return input;
}

function checkbox(checked: boolean, onChange: (next: boolean) => void): HTMLInputElement {
  const input = el('input');
  input.type = 'checkbox';
  input.checked = checked;
  input.addEventListener('change', () => {
    onChange(input.checked);
    markDirty();
  });
  return input;
}

function selectInput<T extends string>(
  value: T,
  options: Array<{ value: T; label: string }>,
  onChange: (next: T) => void,
): HTMLSelectElement {
  const select = el('select');
  for (const option of options) {
    const node = el('option', undefined, option.label);
    node.value = option.value;
    select.appendChild(node);
  }
  select.value = value;
  select.addEventListener('change', () => {
    onChange(select.value as T);
    markDirty();
  });
  return select;
}

/** 一行"勾选 + 说明"（横排！这一块的旧实现被挤成竖排文字，是用户说的"杂乱"之一） */
function checkRow(checked: boolean, label: string, hint: string | undefined, onChange: (next: boolean) => void): HTMLElement {
  const row = el('div', 'check-row');
  const wrap = el('label');
  wrap.appendChild(checkbox(checked, onChange));
  const text = el('span');
  text.appendChild(el('strong', undefined, label));
  if (hint) {
    text.appendChild(el('span', 'hint', `　${hint}`));
  }
  wrap.appendChild(text);
  row.appendChild(wrap);
  return row;
}

function markDirty(): void {
  if (!loaded) return;
  dirty = true;
  document.body.dataset.dirty = '1';
  byId('status-text').textContent = '有未保存的改动 —— 点右下角「保存并立即生效」';
  byId('status-text').className = 'warn';
}

function showToast(message: string): void {
  const toast = byId('toast');
  toast.textContent = message;
  toast.classList.add('show');
  window.setTimeout(() => toast.classList.remove('show'), 1800);
}

function setStatus(kind: '' | 'ok' | 'warn' | 'error', text: string): void {
  const box = byId('status-text');
  box.className = kind;
  box.textContent = text;
}

// ============================================================================
//  通用组件
// ============================================================================

/** 字符串列表编辑器（各动画池、分类动作、移动动作都用它） */
function listEditor(
  items: string[],
  onChange: (next: string[]) => void,
  options: { suggestions?: boolean; placeholder?: string; addLabel?: string } = {},
): HTMLElement {
  const wrap = el('div', 'list');
  items.forEach((value, index) => {
    const row = el('div', 'row');
    const input = textInput(value, (next) => {
      items[index] = next;
      onChange(items);
    }, options.suggestions !== false, options.placeholder);
    row.appendChild(input);
    row.appendChild(
      (() => {
        const remove = button('', 'ghost', undefined, 'x');
        remove.title = '删除这一项';
        remove.addEventListener('click', () => {
          const next = items.slice();
          next.splice(index, 1);
          onChange(next);
          render();
        });
        return remove;
      })(),
    );
    wrap.appendChild(row);
  });
  wrap.appendChild(
    button(options.addLabel ?? '添加一项', undefined, () => {
      const next = items.slice();
      next.push('');
      onChange(next);
      render();
    }, 'plus'),
  );
  return wrap;
}

/** 动画池编辑器（全局默认值与"某只宠物的单独设置"共用同一套控件） */
function poolEditor(animations: AnimationsConfig, options: { showWeights: boolean; weights?: AnimationWeights }): HTMLElement {
  const host = el('div');

  const pools: Array<{ key: 'idle' | 'turn' | 'drag' | 'clicks'; title: string; hint: string }> = [
    { key: 'idle', title: '待机池', hint: '等概率抽；第一个同时作为首帧' },
    { key: 'turn', title: '转向池', hint: '必须都是"播完会翻转朝向"的动画' },
    { key: 'drag', title: '拖拽池', hint: '"被无形抓起悬空"的姿势' },
    { key: 'clicks', title: '点击回应池', hint: '点按时随机抽 1 个' },
  ];
  for (const pool of pools) {
    const head = el('div', 'subhead');
    head.appendChild(el('strong', undefined, pool.title));
    head.appendChild(el('span', 'hint', pool.hint));
    host.appendChild(head);
    host.appendChild(listEditor(animations[pool.key], (next) => {
      animations[pool.key] = next;
    }));
  }

  // ---- 随机动作分类 ----
  const catHead = el('div', 'subhead');
  catHead.appendChild(el('strong', undefined, '随机动作分类'));
  catHead.appendChild(el('span', 'hint', 'weight 是出现权重；带文字、镜像会颠倒的勾上 noMirror'));
  host.appendChild(catHead);

  animations.categories.forEach((category, index) => {
    const box = el('div', 'cat');
    const head = el('div', 'cat-head');
    head.appendChild(textInput(category.id, (next) => {
      category.id = next;
    }, false, '分类名'));
    head.appendChild(numberInput(category.weight, (next) => {
      category.weight = next;
    }, { min: 0 }));
    const mirror = el('label', 'check-row');
    mirror.style.padding = '4px 8px';
    mirror.appendChild(checkbox(category.noMirror === true, (next) => {
      category.noMirror = next;
    }));
    mirror.appendChild(el('span', undefined, '镜像会颠倒'));
    head.appendChild(mirror);
    head.appendChild(
      button('', 'ghost', () => {
        animations.categories.splice(index, 1);
        render();
      }, 'x'),
    );
    box.appendChild(head);
    box.appendChild(el('div', 'subhead'));
    box.appendChild(listEditor(category.actions, (next) => {
      category.actions = next;
    }, { placeholder: '动画名' }));
    host.appendChild(box);
  });
  host.appendChild(
    button('添加分类', undefined, () => {
      animations.categories.push({
        id: `新分类 ${animations.categories.length + 1}`,
        weight: 10,
        // actions 不能为空（Rust 侧校验会拒绝）：给一个真实存在的动画作为起点
        actions: [available[0] ?? animations.idle[0] ?? ''],
        noMirror: false,
      });
      markDirty();
      render();
    }, 'plus'),
  );

  // ---- 移动池 ----
  const moveHead = el('div', 'subhead');
  moveHead.appendChild(el('strong', undefined, '移动池'));
  moveHead.appendChild(el('span', 'hint', '每项的参数覆盖（params）保持配置文件里的原值'));
  host.appendChild(moveHead);
  const moves = animations.moves;
  const moveList = el('div', 'list');
  moves.actions.forEach((spec, index) => {
    const row = el('div', 'row');
    row.appendChild(textInput(spec.name, (next) => {
      spec.name = next;
    }, true, '动画名'));
    if (spec.params) {
      const badge = el('span', 'pill muted', 'params');
      badge.title = `该动作有参数覆盖：${JSON.stringify(spec.params)}`;
      row.appendChild(badge);
    }
    row.appendChild(
      button('', 'ghost', () => {
        moves.actions.splice(index, 1);
        render();
      }, 'x'),
    );
    moveList.appendChild(row);
  });
  moveList.appendChild(
    button('添加移动动作', undefined, () => {
      moves.actions.push({ name: available[0] ?? '' });
      markDirty();
      render();
    }, 'plus'),
  );
  host.appendChild(moveList);

  // ---- 权重 ----
  if (options.showWeights && options.weights) {
    const weights = options.weights;
    const head = el('div', 'subhead');
    head.appendChild(el('strong', undefined, '动画链权重'));
    head.appendChild(el('span', 'hint', 'idle + turn + move 之和，加上各分类 weight，应当等于 100'));
    host.appendChild(head);
    const grid = el('div', 'grid');
    grid.appendChild(field('待机（idle）', numberInput(weights.idle, (next) => {
      weights.idle = next;
    }, { min: 0, max: 100 })));
    grid.appendChild(field('转向（turn）', numberInput(weights.turn, (next) => {
      weights.turn = next;
    }, { min: 0, max: 100 })));
    grid.appendChild(field('移动（move）', numberInput(weights.move, (next) => {
      weights.move = next;
    }, { min: 0, max: 100 })));
    host.appendChild(grid);
  }

  const eventsCount = animations.events ? Object.keys(animations.events).length : 0;
  const note = el('p', 'desc', `事件动画（events）：${eventsCount} 组，原样保留（本页不编辑）。`);
  note.style.marginTop = '14px';
  note.style.marginBottom = '0';
  host.appendChild(note);
  return host;
}

// ============================================================================
//  各页面
// ============================================================================

function renderPetPage(pet: PetEntry, index: number): HTMLElement {
  if (!config) throw new Error('配置尚未载入');
  // 闭包（按钮回调）里 TS 不会保留 `config` 的非空收窄（它是个可变变量），先固定成局部常量
  const cfg = config;
  const host = el('div', 'page-inner');

  // ---- 基本信息 ----
  const basic = card('基本信息', '尺寸是包围盒宽度（高度按 9:16 推出）；角落与边距决定启动落点，「回到初始位置」用的是同一套语义。');
  const grid = el('div', 'grid');
  grid.appendChild(field('显示名', textInput(pet.name, (next) => {
    pet.name = next;
    renderNav();
  })));
  grid.appendChild(field('ID', textInput(pet.id, (next) => {
    pet.id = next;
    renderNav();
  }), '窗口标签用；不能含 / \\ : * ? " < > |'));
  grid.appendChild(field('尺寸（64~2048px）', numberInput(pet.size, (next) => {
    pet.size = next;
  }, { min: 64, max: 2048 })));
  grid.appendChild(field('初始角落', selectInput(pet.position.corner, CORNERS, (next) => {
    pet.position.corner = next;
  })));
  grid.appendChild(field('边距 X（px）', numberInput(pet.position.marginX, (next) => {
    pet.position.marginX = next;
  }, { integer: true })));
  grid.appendChild(field('边距 Y（px）', numberInput(pet.position.marginY, (next) => {
    pet.position.marginY = next;
  }, { integer: true })));
  basic.appendChild(grid);
  host.appendChild(basic);

  // ---- 行为 ----
  const custom = pet.animations != null || pet.animationWeights != null;
  const behaviour = card('行为', '决定这只宠物"会做哪些动作"。跟随全局时，改「动画池默认值」会同时影响所有跟随的宠物。');
  const radios = el('div', 'radio-row');

  const followCard = el('label', 'radio-card');
  followCard.dataset.active = String(!custom);
  // 稳定 id：排障探针要能"切到单独设置再保存"（见 src-tauri/src/diagnostics.rs）
  followCard.id = `behaviour-global-${index}`;
  const followRadio = el('input');
  followRadio.type = 'radio';
  followRadio.name = `behaviour-${index}`;
  followRadio.checked = !custom;
  followRadio.addEventListener('change', () => {
    pet.animations = null;
    pet.animationWeights = null;
    markDirty();
    render();
    showToast('已改为跟随全局默认');
  });
  followCard.appendChild(followRadio);
  const followText = el('div');
  followText.appendChild(el('strong', undefined, '跟随全局默认'));
  followText.appendChild(el('span', undefined, '与其它宠物共用一套动画池'));
  followCard.appendChild(followText);
  radios.appendChild(followCard);

  const ownCard = el('label', 'radio-card');
  ownCard.dataset.active = String(custom);
  ownCard.id = `behaviour-own-${index}`;
  const ownRadio = el('input');
  ownRadio.type = 'radio';
  ownRadio.name = `behaviour-${index}`;
  ownRadio.checked = custom;
  ownRadio.addEventListener('change', () => {
    // 从全局复制一份作为起点：直接给空池会被 Rust 侧校验拒绝（idle/clicks 不能为空）
    pet.animations = structuredClone(cfg.animations);
    pet.animationWeights = structuredClone(cfg.animationWeights);
    markDirty();
    render();
    showToast('已复制全局默认作为起点，接着改就行');
  });
  ownCard.appendChild(ownRadio);
  const ownText = el('div');
  ownText.appendChild(el('strong', undefined, '单独设置'));
  ownText.appendChild(el('span', undefined, '这只宠物用自己的动画池与权重'));
  ownCard.appendChild(ownText);
  radios.appendChild(ownCard);
  behaviour.appendChild(radios);

  if (custom) {
    const head = el('div', 'subhead');
    head.appendChild(el('strong', undefined, '这只宠物的动画池'));
    head.appendChild(el('span', 'pill', '单独设置'));
    behaviour.appendChild(head);
    const refresh = button('从全局重新复制一份', undefined, () => {
      pet.animations = structuredClone(cfg.animations);
      pet.animationWeights = structuredClone(cfg.animationWeights);
      markDirty();
      render();
      showToast('已从全局覆盖');
    }, 'rotate-ccw');
    behaviour.appendChild(refresh);
    const editor = el('div');
    editor.style.marginTop = '10px';
    editor.appendChild(
      poolEditor(pet.animations as AnimationsConfig, {
        showWeights: true,
        weights: pet.animationWeights as AnimationWeights,
      }),
    );
    behaviour.appendChild(editor);
  }
  host.appendChild(behaviour);

  // ---- 危险区 ----
  const danger = card('移除', '从配置里删掉这只宠物；保存后它的窗口会被关闭。');
  const removeButton = button('删除这只宠物', 'danger', () => {
    if (!config) return;
    config.pets.splice(index, 1);
    const nextIndex = Math.max(0, Math.min(index, config.pets.length - 1));
    view = config.pets.length > 0 ? `pet:${nextIndex}` : 'animations';
    markDirty();
    render();
  }, 'x');
  // 稳定 id：排障探针要能"删掉第 N 只再保存"（见 src-tauri/src/diagnostics.rs）
  removeButton.id = `btn-del-pet-${index}`;
  danger.appendChild(removeButton);
  host.appendChild(danger);

  return host;
}

function renderPhysicsPage(): HTMLElement {
  if (!config) throw new Error('配置尚未载入');
  const physics = config.physics;
  const host = el('div', 'page-inner');
  const node = card('拖拽与抛掷物理', '这些参数对所有宠物生效（与上游 dsh-pet 的物理参数同一套语义）。');
  const grid = el('div', 'grid');
  grid.appendChild(field('重力（px/s²）', numberInput(physics.gravity, (next) => {
    physics.gravity = next;
  }, { min: 0 })));
  grid.appendChild(field('恢复系数（0~1）', numberInput(physics.restitution, (next) => {
    physics.restitution = next;
  }, { min: 0, max: 1 })));
  grid.appendChild(field('地面摩擦（0~1）', numberInput(physics.groundFriction, (next) => {
    physics.groundFriction = next;
  }, { min: 0, max: 1 })));
  grid.appendChild(field('抛掷力度（倍率）', numberInput(physics.throwPower, (next) => {
    physics.throwPower = next;
  }, { min: 0.01 })));
  node.appendChild(grid);

  const switches = el('div');
  switches.style.marginTop = '14px';
  switches.appendChild(
    checkRow(physics.ceilingBounce, '碰到屏幕顶部会反弹', '关掉后宠物可以被甩出屏幕上缘，靠重力落回来', (next) => {
      physics.ceilingBounce = next;
    }),
  );
  switches.appendChild(
    checkRow(physics.petCollision, '多只宠物之间会碰撞', 'M1 规划中：目前是占位开关，暂不生效', (next) => {
      physics.petCollision = next;
    }),
  );
  node.appendChild(switches);
  host.appendChild(node);
  return host;
}

function renderAnimationsPage(): HTMLElement {
  if (!config) throw new Error('配置尚未载入');
  const host = el('div', 'page-inner');
  const followers = config.pets.filter((pet) => pet.animations == null && pet.animationWeights == null).length;
  const node = card(
    '全局动画池（默认值）',
    followers === config.pets.length
      ? '所有宠物当前都跟随这份默认值。'
      : `有 ${config.pets.length - followers} 只宠物改成了"单独设置"，它们不受这里的改动影响。`,
  );
  node.appendChild(poolEditor(config.animations, { showWeights: true, weights: config.animationWeights }));
  host.appendChild(node);
  return host;
}

function renderSystemPage(): HTMLElement {
  const host = el('div', 'page-inner');

  const start = card('启动', '开机自启写入当前用户的启动项，不需要管理员权限；这一项**立即生效**，不经过「保存」。');
  start.appendChild(
    checkRow(autostart, '开机时自动启动 whale-pet', undefined, (next) => {
      void (async () => {
        try {
          const now = await invoke<boolean>('set_autostart', { enabled: next });
          autostart = now;
          render();
          showToast(now ? '开机自启：已启用' : '开机自启：已关闭');
          petLog(`设置: 开机自启 → ${now ? '启用' : '关闭'}`);
        } catch (err) {
          render();
          setStatus('error', `开机自启设置失败：${err instanceof Error ? err.message : String(err)}`);
          petLogError('设置: 开机自启设置失败', err);
        }
      })();
    }),
  );

  const info = card('文件与位置');
  const rows: Array<[string, string]> = [
    ['配置文件', configPath],
    ['应用数据目录', appDataDir],
    ['诊断日志', `${appDataDir}\\pet-debug.log`],
    ['上一版备份', `${configPath}.bak`],
  ];
  for (const [key, value] of rows) {
    const row = el('div', 'info-row');
    row.appendChild(el('div', 'k', key));
    row.appendChild(el('div', 'v', value));
    info.appendChild(row);
  }
  const actions = el('div', 'row-actions');
  actions.appendChild(
    button('打开配置文件所在目录', undefined, () => {
      void invoke<void>('open_config_location').catch((err: unknown) => {
        setStatus('error', `打开失败：${err instanceof Error ? err.message : String(err)}`);
        petLogError('设置: 打开配置文件所在目录失败', err);
      });
    }, 'folder-open'),
  );
  info.appendChild(actions);
  host.appendChild(start);
  host.appendChild(info);

  const single = card('运行方式');
  single.appendChild(
    checkRow(true, '同一个应用只运行一份', '重复启动会显示已有实例而不是再开一份（单实例锁）；关掉设置窗口不会退出应用，靠托盘菜单的「退出」结束。', () => {
      showToast('这一项是固定行为，不需要配置');
    }),
  );
  host.appendChild(single);
  return host;
}

function renderAboutPage(): HTMLElement {
  if (!config) throw new Error('配置尚未载入');
  const host = el('div', 'page-inner');
  const node = card('关于', 'whale-pet desktop：把上游鲸鱼桌宠做成独立的 Windows 桌面应用（Tauri v2）。');
  const logoRow = el('div', 'row');
  const logo = el('div');
  logo.style.width = '56px';
  logo.style.height = '56px';
  logo.style.borderRadius = '16px';
  logo.style.background = 'linear-gradient(160deg, #8FE6DC, #39A9C9)';
  logo.style.display = 'grid';
  logo.style.placeItems = 'center';
  logo.innerHTML = PET_LOGO;
  logoRow.appendChild(logo);
  const meta = el('div');
  meta.style.marginLeft = '14px';
  meta.appendChild(el('strong', undefined, 'whale-pet 0.1.0'));
  meta.appendChild(el('div', 'inline-hint', `${config.pets.length} 只宠物 · 素材 ${available.length} 条`));
  logoRow.appendChild(meta);
  node.appendChild(logoRow);

  const info = el('div');
  info.style.marginTop = '14px';
  const lines: Array<[string, string]> = [
    ['代码许可', 'MIT（与上游一致）'],
    ['素材许可', '允许开源使用，禁止商用（上游约定，本应用沿用）'],
    ['文档', 'docs/ROADMAP.md · docs/VERIFICATION.md · docs/TAURI-CONFIG.md'],
  ];
  for (const [key, value] of lines) {
    const row = el('div', 'info-row');
    row.appendChild(el('div', 'k', key));
    row.appendChild(el('div', 'v', value));
    info.appendChild(row);
  }
  node.appendChild(info);
  host.appendChild(node);
  return host;
}

// ============================================================================
//  导航与整页渲染
// ============================================================================

function renderNav(): void {
  if (!config) return;
  const nav = byId('nav');
  nav.textContent = '';

  nav.appendChild(el('div', 'nav-group', '宠物'));
  config.pets.forEach((pet, index) => {
    const item = el('button', 'nav-item');
    item.type = 'button';
    // 稳定的 id：排障探针要能"切到某一页"截图（见 src-tauri/src/diagnostics.rs）
    item.id = `nav-pet-${index}`;
    const glyph = el('span', 'ic');
    glyph.innerHTML = icon('paw-print', 14);
    item.appendChild(glyph);
    const label = el('span', 'label', pet.name.trim() || pet.id || `宠物 ${index + 1}`);
    item.appendChild(label);
    if (pet.animations != null || pet.animationWeights != null) {
      const dot = el('span', 'dot');
      dot.title = '这只宠物有自己的行为设置';
      item.appendChild(dot);
    }
    item.dataset.active = String(view === `pet:${index}`);
    item.addEventListener('click', () => {
      view = `pet:${index}`;
      render();
    });
    nav.appendChild(item);
  });
  const add = el('button', 'nav-item');
  add.type = 'button';
  // 稳定 id：排障探针要能"加一只宠物再保存"（见 src-tauri/src/diagnostics.rs）
  add.id = 'btn-add-pet';
  const addGlyph = el('span', 'ic');
  addGlyph.innerHTML = icon('plus', 14);
  add.appendChild(addGlyph);
  add.appendChild(el('span', 'label', '添加宠物'));
  add.addEventListener('click', () => {
    if (!config) return;
    config.pets.push({
      id: nextPetId(),
      name: `宠物 ${config.pets.length + 1}`,
      size: config.pets[0]?.size ?? 462,
      idle: '',
      click: '',
      // 与上一只错开一点，否则新宠物正好压在旧宠物身上（看着像"没生效"）
      position: { corner: 'bottom-right', marginX: 40 + config.pets.length * 24, marginY: 40 },
      animations: null,
      animationWeights: null,
    });
    view = `pet:${config.pets.length - 1}`;
    markDirty();
    render();
    showToast('已添加一只宠物（记得保存）');
  });
  nav.appendChild(add);

  nav.appendChild(el('div', 'nav-group', '通用'));
  const commons: Array<{ id: string; label: string; glyph: string }> = [
    { id: 'physics', label: '物理参数', glyph: 'sliders-horizontal' },
    { id: 'animations', label: '动画池默认值', glyph: 'clapperboard' },
    { id: 'system', label: '启动与系统', glyph: 'rocket' },
    { id: 'about', label: '关于', glyph: 'info' },
  ];
  for (const entry of commons) {
    const item = el('button', 'nav-item');
    item.type = 'button';
    item.id = `nav-${entry.id}`;
    const glyph = el('span', 'ic');
    glyph.innerHTML = icon(entry.glyph, 14);
    item.appendChild(glyph);
    item.appendChild(el('span', 'label', entry.label));
    item.dataset.active = String(view === entry.id);
    item.addEventListener('click', () => {
      view = entry.id;
      render();
    });
    nav.appendChild(item);
  }
}

function nextPetId(): string {
  const used = new Set((config?.pets ?? []).map((pet) => pet.id));
  let index = (config?.pets.length ?? 0) + 1;
  while (used.has(`pet${index}`)) index += 1;
  return `pet${index}`;
}

/** 整页渲染（结构变化后调用） */
function render(): void {
  if (!config) return;
  renderNav();

  const body = byId('content-body');
  const scrollTop = body.scrollTop;
  body.textContent = '';

  const crumbs = byId('crumbs');
  crumbs.textContent = '';

  let page: HTMLElement;
  if (view.startsWith('pet:')) {
    const index = Number(view.slice(4));
    const pet = config.pets[index];
    if (!pet) {
      view = config.pets.length > 0 ? 'pet:0' : 'animations';
      render();
      return;
    }
    crumbs.appendChild(el('b', undefined, pet.name.trim() || pet.id));
    crumbs.appendChild(el('span', undefined, `宠物 ${index + 1} / ${config.pets.length}`));
    page = renderPetPage(pet, index);
  } else if (view === 'physics') {
    crumbs.appendChild(el('b', undefined, '物理参数'));
    crumbs.appendChild(el('span', undefined, '全局'));
    page = renderPhysicsPage();
  } else if (view === 'animations') {
    crumbs.appendChild(el('b', undefined, '动画池默认值'));
    crumbs.appendChild(el('span', undefined, '全局'));
    page = renderAnimationsPage();
  } else if (view === 'system') {
    crumbs.appendChild(el('b', undefined, '启动与系统'));
    page = renderSystemPage();
  } else {
    crumbs.appendChild(el('b', undefined, '关于'));
    page = renderAboutPage();
  }
  body.appendChild(page);
  body.scrollTop = scrollTop;

  byId('sidebar-foot').textContent = `${config.pets.length} 只宠物 · ${available.length} 条素材`;
}

/** 素材候选下拉（全局一个 datalist，所有动画名输入框共用） */
function renderAnimOptions(): void {
  const datalist = byId<HTMLDataListElement>('anim-options');
  datalist.textContent = '';
  for (const name of available) {
    const option = el('option');
    option.value = name;
    datalist.appendChild(option);
  }
}

// ============================================================================
//  与宿主交互
// ============================================================================

async function load(): Promise<void> {
  try {
    const dto = await invoke<SettingsDto>('get_settings');
    config = dto.config;
    available = dto.availableAnimations;
    autostart = dto.autostart;
    configPath = dto.configPath;
    appDataDir = dto.appDataDir;
    loaded = true;
    dirty = false;
    document.body.dataset.dirty = '0';
    byId('btn-save').removeAttribute('disabled');
    renderAnimOptions();
    if (view.startsWith('pet:')) {
      const index = Number(view.slice(4));
      if (!config.pets[index]) view = config.pets.length > 0 ? 'pet:0' : 'animations';
    }
    render();
    setStatus('', `已载入：${config.pets.length} 只宠物 · 素材 ${available.length} 条`);
    petLog(`设置: 已载入配置（${config.pets.length} 只宠物，${available.length} 条素材）`);
  } catch (err) {
    loaded = false;
    byId('btn-save').setAttribute('disabled', 'disabled');
    setStatus('error', `读取配置失败：${err instanceof Error ? err.message : String(err)}`);
    petLogError('设置: 读取配置失败', err);
  }
}

async function save(): Promise<void> {
  if (!config || !loaded) return;
  setStatus('', '正在保存并应用…');
  try {
    const result = await invoke<SaveSettingsDto>('save_settings', { config });
    dirty = false;
    document.body.dataset.dirty = '0';
    const lines = [`已保存并立即生效：${result.petCount} 只宠物`, `配置文件：${result.path}`];
    if (result.backup) lines.push(`上一版备份：${result.backup}`);
    if (result.warnings.length > 0) {
      lines.push(`素材警告 ${result.warnings.length} 条（保存本身已成功）：`);
      for (const line of result.warnings.slice(0, 6)) lines.push(`  - ${line}`);
      if (result.warnings.length > 6) lines.push(`  …（其余 ${result.warnings.length - 6} 条见日志）`);
    }
    setStatus(result.warnings.length > 0 ? 'warn' : 'ok', lines.join('\n'));
    petLog(`设置: 保存成功（${result.petCount} 只宠物，${result.warnings.length} 条素材警告）`);
    showToast('已保存并立即生效');
    // 保存后宠物窗是全新的：重新拉一次让界面与宿主完全对齐
    await load();
    setStatus(result.warnings.length > 0 ? 'warn' : 'ok', lines.join('\n'));
  } catch (err) {
    setStatus('error', `保存失败：${err instanceof Error ? err.message : String(err)}`);
    petLogError('设置: 保存失败', err);
  }
}

// ============================================================================
//  启动
// ============================================================================

function bind(): void {
  setLogLabel('settings');
  byId('brand-logo').innerHTML = PET_LOGO;
  // 顶部与底部的按钮是静态 HTML：这里补上图标（用同一套 Lucide 形状）
  byId('btn-open-file').innerHTML = `${icon('folder-open', 14)}<span>打开配置文件</span>`;
  byId('btn-reload').innerHTML = `${icon('rotate-ccw', 14)}<span>放弃改动</span>`;
  byId('btn-save').innerHTML = `${icon('save', 14)}<span>保存并立即生效</span>`;
  byId('btn-save').addEventListener('click', () => void save());
  byId('btn-reload').addEventListener('click', () => {
    void load().then(() => showToast('已放弃未保存的改动'));
  });
  byId('btn-open-file').addEventListener('click', () => {
    void invoke<void>('open_config_location').catch((err: unknown) => {
      setStatus('error', `打开配置文件所在目录失败：${err instanceof Error ? err.message : String(err)}`);
      petLogError('设置: 打开配置文件所在目录失败', err);
    });
  });
  window.addEventListener('beforeunload', (event) => {
    if (!dirty) return;
    event.preventDefault();
    event.returnValue = '';
  });
}

bind();
void load();
