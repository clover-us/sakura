/**
 * 设置窗口的前端入口（M2）。
 *
 * ## 这一页与桌宠那三个页面最大的不同
 *
 * 它是**普通界面**：不透明、可聚焦、整窗吃鼠标。桌宠页面要考虑的穿透/坐标/首帧延迟
 * 在这里都不存在——设置页只管"把配置读出来、改、存回去"。
 *
 * ## 数据流（刻意设计成"整份配置进出"）
 *
 * ```text
 * get_settings ──► AppConfig（原样持有）
 *                     │  用户改哪几个字段就动哪几个，其余字段（如 animations.events）原封不动
 *                     ▼
 * save_settings(config) ──► 校验 → 备份 + 原子写盘 → 立即重建宠物窗
 * ```
 *
 * **不给每个字段建 TS 类型映射表、也不逐字段回传**：Rust 侧以后加字段，
 * 这一页不需要跟着改；用户手改过的冷门字段（events、moves 的 params）也不会被抹掉。
 * 页面只声明它**会读写**的那些字段（见下面的接口），其余按 `unknown` 原样带着走。
 *
 * ## 校验在哪
 *
 * 只在 Rust 侧（`AppConfig::validate`）。前端不重复实现一套规则——否则两边迟早不一致，
 * 而"前端放过、后端拒绝"至少还能给出确切原因，"前端拒绝、后端其实允许"则会白白挡住用户。
 */
import { petLog, petLogError, setLogLabel } from './bridge/log.ts';
import { invoke } from './bridge/tauri.ts';

// ============================================================================
//  与 Rust `AppConfig` 同构的类型（只声明本页读写得到的字段）
// ============================================================================

/** 角落（Rust 侧是 kebab-case 枚举） */
type Corner = 'top-left' | 'top-right' | 'bottom-left' | 'bottom-right';

interface PositionConfig {
  corner: Corner;
  marginX: number;
  marginY: number;
}

interface PetEntry {
  id: string;
  name: string;
  size: number;
  /** 留空 = 从动画池派生（本页不编辑，原样带着走） */
  idle: string;
  click: string;
  position: PositionConfig;
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
//  页面状态
// ============================================================================

/** 从宿主取回的配置（保存时原样回传；未加载成功前为 null） */
let config: AppConfig | null = null;
/** 素材目录里可用的动画名（下拉候选） */
let available: string[] = [];
/** 防止未加载完就允许保存 */
let loaded = false;

const CORNERS: Array<{ value: Corner; label: string }> = [
  { value: 'top-left', label: '左上' },
  { value: 'top-right', label: '右上' },
  { value: 'bottom-left', label: '左下' },
  { value: 'bottom-right', label: '右下' },
];

// ============================================================================
//  DOM 小工具（没有引入任何框架：这一页只有几百个元素，手写足够且零依赖）
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

function field(labelText: string, control: HTMLElement): HTMLLabelElement {
  const label = el('label', 'field');
  label.appendChild(el('span', undefined, labelText));
  label.appendChild(control);
  return label;
}

/** 文本输入（带素材候选下拉） */
function textInput(value: string, onChange: (next: string) => void, withSuggestions = false): HTMLInputElement {
  const input = el('input');
  input.type = 'text';
  input.value = value;
  if (withSuggestions) input.setAttribute('list', 'anim-options');
  input.addEventListener('input', () => {
    onChange(input.value);
    markDirty();
  });
  return input;
}

/** 数字输入（整数开关决定是否取整） */
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
    // 空串 / 非数字时**不写回**：否则用户删到一半就被塞进一个 0，光标还会跳
    if (!Number.isFinite(parsed)) return;
    onChange(options.integer ? Math.round(parsed) : parsed);
    markDirty();
  });
  return input;
}

function checkboxInput(checked: boolean, onChange: (next: boolean) => void): HTMLInputElement {
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

function markDirty(): void {
  if (loaded) document.body.classList.add('dirty');
}

/** 状态条：kind 为空表示普通提示 */
function setStatus(kind: '' | 'ok' | 'error' | 'warn', text: string): void {
  const box = byId('status');
  box.className = kind;
  box.textContent = text;
}

// ============================================================================
//  渲染
// ============================================================================

/** 素材候选下拉：全局一个 datalist，所有动画名输入框共用 */
function renderAnimOptions(): void {
  const existing = document.getElementById('anim-options');
  if (existing) existing.remove();
  const datalist = el('datalist');
  datalist.id = 'anim-options';
  for (const name of available) {
    const option = el('option');
    option.value = name;
    datalist.appendChild(option);
  }
  document.body.appendChild(datalist);
}

/**
 * 通用"字符串列表"编辑器（idle / turn / drag / clicks / 某个分类的动作都用它）。
 *
 * 增删后整页重渲染（结构变了，重渲染最省事也更不容易错）；
 * 逐字修改只改数组不重渲染，避免输入框失焦。
 */
function listEditor(
  items: string[],
  onChange: (next: string[]) => void,
  options: { placeholder?: string } = {},
): HTMLElement {
  const wrap = el('div', 'list');
  items.forEach((value, index) => {
    const row = el('div', 'row');
    const input = textInput(value, (next) => {
      items[index] = next;
      onChange(items);
    }, true);
    if (options.placeholder) input.placeholder = options.placeholder;
    row.appendChild(input);
    const remove = el('button', 'icon', '✕');
    remove.type = 'button';
    remove.title = '删除这一项';
    remove.addEventListener('click', () => {
      const next = items.slice();
      next.splice(index, 1);
      onChange(next);
      markDirty();
      render();
    });
    row.appendChild(remove);
    wrap.appendChild(row);
  });

  const add = el('button', undefined, '＋ 添加一项');
  add.type = 'button';
  add.style.alignSelf = 'flex-start';
  add.addEventListener('click', () => {
    const next = items.slice();
    next.push('');
    onChange(next);
    markDirty();
    render();
  });
  wrap.appendChild(add);
  return wrap;
}

function renderPets(container: HTMLElement, pets: PetEntry[]): void {
  container.textContent = '';
  pets.forEach((pet, index) => {
    const card = el('div', 'pet-card');

    const head = el('div', 'pet-head');
    head.appendChild(el('strong', undefined, `宠物 ${index + 1}：${pet.name || '(未命名)'}（窗口标签 pet-${pet.id || '?'}-${index}）`));
    const remove = el('button', 'icon', '删除这只');
    remove.type = 'button';
    // 稳定的 id：排障探针要能"点中某一行的删除"（见 src-tauri/src/diagnostics.rs 的设置窗口探针）
    remove.id = `btn-del-pet-${index}`;
    remove.title = '从配置里移除这只宠物（保存后窗口关闭）';
    remove.addEventListener('click', () => {
      pets.splice(index, 1);
      markDirty();
      render();
    });
    head.appendChild(remove);
    card.appendChild(head);

    const grid = el('div', 'grid');
    grid.appendChild(field('显示名', textInput(pet.name, (next) => { pet.name = next; }, false)));
    grid.appendChild(field('ID（窗口标签用，不能含 / \\ : * ? " < > |）', textInput(pet.id, (next) => { pet.id = next; }, false)));
    grid.appendChild(field('尺寸（64~2048 像素）', numberInput(pet.size, (next) => { pet.size = next; }, { min: 64, max: 2048 })));
    grid.appendChild(field('初始角落', selectInput(pet.position.corner, CORNERS, (next) => { pet.position.corner = next; })));
    grid.appendChild(field('边距 X（像素）', numberInput(pet.position.marginX, (next) => { pet.position.marginX = next; }, { integer: true })));
    grid.appendChild(field('边距 Y（像素）', numberInput(pet.position.marginY, (next) => { pet.position.marginY = next; }, { integer: true })));
    card.appendChild(grid);

    container.appendChild(card);
  });
}

function renderPhysics(container: HTMLElement, physics: PhysicsParams): void {
  container.textContent = '';
  container.appendChild(field('重力（px/s²，通常 2000）', numberInput(physics.gravity, (next) => { physics.gravity = next; }, { min: 0 })));
  container.appendChild(field('恢复系数（0~1，撞击后保留的速度比例）', numberInput(physics.restitution, (next) => { physics.restitution = next; }, { min: 0, max: 1 })));
  container.appendChild(field('地面摩擦（0~1，越大停得越快）', numberInput(physics.groundFriction, (next) => { physics.groundFriction = next; }, { min: 0, max: 1 })));
  container.appendChild(field('抛掷力度（倍率，越大甩得越远）', numberInput(physics.throwPower, (next) => { physics.throwPower = next; }, { min: 0.01 })));

  const checks = el('div');
  checks.style.display = 'flex';
  checks.style.alignItems = 'center';
  const ceiling = el('label', 'check');
  ceiling.appendChild(checkboxInput(physics.ceilingBounce, (next) => { physics.ceilingBounce = next; }));
  ceiling.appendChild(el('span', undefined, '碰到屏幕顶部会反弹'));
  checks.appendChild(ceiling);
  const collision = el('label', 'check');
  collision.appendChild(checkboxInput(physics.petCollision, (next) => { physics.petCollision = next; }));
  collision.appendChild(el('span', undefined, '多只宠物之间碰撞（M1 规划中，当前不生效）'));
  checks.appendChild(collision);
  container.appendChild(field('开关', checks));
}

function renderPools(container: HTMLElement, animations: AnimationsConfig): void {
  container.textContent = '';
  const pools: Array<{ key: 'idle' | 'turn' | 'drag' | 'clicks'; title: string; hint: string }> = [
    { key: 'idle', title: '待机池（等概率抽）', hint: '' },
    { key: 'turn', title: '转向池（必须都是"播完会翻转朝向"的动画）', hint: '' },
    { key: 'drag', title: '拖拽池（"被无形抓起悬空"的姿势）', hint: '' },
    { key: 'clicks', title: '点击回应池', hint: '' },
  ];
  for (const pool of pools) {
    const head = el('div', 'list-head');
    head.appendChild(el('strong', undefined, pool.title));
    if (pool.hint) head.appendChild(el('span', 'hint', pool.hint));
    container.appendChild(head);
    container.appendChild(listEditor(animations[pool.key], (next) => { animations[pool.key] = next; }));
  }

  const eventsCount = animations.events ? Object.keys(animations.events).length : 0;
  const note = el('p', 'hint', `事件动画（animations.events）：${eventsCount} 组，原样保留（本页不编辑）。`);
  container.appendChild(note);
}

function renderWeights(container: HTMLElement, weights: AnimationWeights): void {
  container.textContent = '';
  container.appendChild(field('idle 权重', numberInput(weights.idle, (next) => { weights.idle = next; }, { min: 0, max: 100 })));
  container.appendChild(field('turn 权重', numberInput(weights.turn, (next) => { weights.turn = next; }, { min: 0, max: 100 })));
  container.appendChild(field('move 权重', numberInput(weights.move, (next) => { weights.move = next; }, { min: 0, max: 100 })));
}

function renderCategories(container: HTMLElement, categories: CategoryConfig[], animations: AnimationsConfig): void {
  container.textContent = '';
  categories.forEach((category, index) => {
    const box = el('div', 'cat');

    const head = el('div', 'row');
    const idInput = textInput(category.id, (next) => { category.id = next; });
    idInput.className = 'grow';
    head.appendChild(idInput);
    const weightInput = numberInput(category.weight, (next) => { category.weight = next; }, { min: 0 });
    weightInput.className = 'num';
    weightInput.title = '出现权重';
    head.appendChild(weightInput);
    const mirrorLabel = el('label', 'check');
    mirrorLabel.appendChild(checkboxInput(category.noMirror === true, (next) => { category.noMirror = next; }));
    mirrorLabel.appendChild(el('span', undefined, '镜像会颠倒（带文字）'));
    head.appendChild(mirrorLabel);
    const remove = el('button', 'icon', '✕');
    remove.type = 'button';
    remove.title = '删除这个分类';
    remove.addEventListener('click', () => {
      categories.splice(index, 1);
      markDirty();
      render();
    });
    head.appendChild(remove);
    box.appendChild(head);

    box.appendChild(listEditor(category.actions, (next) => { category.actions = next; }));
    container.appendChild(box);
  });
  void animations;
}

function renderMoves(container: HTMLElement, moves: MovesConfig): void {
  container.textContent = '';
  const specs = moves.actions;
  const wrap = el('div', 'list');
  specs.forEach((spec, index) => {
    const row = el('div', 'row');
    const input = textInput(spec.name, (next) => { spec.name = next; }, true);
    row.appendChild(input);
    const hasParams = spec.params !== undefined && spec.params !== null;
    if (hasParams) {
      const badge = el('span', 'hint', 'params');
      badge.title = `该动作有参数覆盖：${JSON.stringify(spec.params)}`;
      badge.style.flex = 'none';
      row.appendChild(badge);
    }
    const remove = el('button', 'icon', '✕');
    remove.type = 'button';
    remove.addEventListener('click', () => {
      specs.splice(index, 1);
      markDirty();
      render();
    });
    row.appendChild(remove);
    wrap.appendChild(row);
  });
  const add = el('button', undefined, '＋ 添加一个移动动作');
  add.type = 'button';
  add.style.alignSelf = 'flex-start';
  add.addEventListener('click', () => {
    specs.push({ name: available[0] ?? '' });
    markDirty();
    render();
  });
  wrap.appendChild(add);

  const defaultHint = el('p', 'hint', `移动默认参数（moves.default）：${JSON.stringify(moves.default)}（只读，改请编辑配置文件）`);
  container.appendChild(defaultHint);
  container.appendChild(wrap);
}

/** 整页重渲染（配置结构变化后调用） */
function render(): void {
  if (!config) return;
  renderAnimOptions();
  renderPets(byId('pets'), config.pets);
  renderPhysics(byId('physics'), config.physics);
  renderPools(byId('pools'), config.animations);
  renderWeights(byId('weights'), config.animationWeights);
  renderCategories(byId('categories'), config.animations.categories, config.animations);
  renderMoves(byId('moves'), config.animations.moves);
}

// ============================================================================
//  与宿主交互
// ============================================================================

/** 拉取配置并整页渲染 */
async function load(): Promise<void> {
  try {
    const dto = await invoke<SettingsDto>('get_settings');
    config = dto.config;
    available = dto.availableAnimations;
    loaded = true;
    document.body.classList.remove('dirty');
    byId('config-path').textContent = `配置文件：${dto.configPath}`;
    (byId('autostart') as HTMLInputElement).checked = dto.autostart;
    byId('btn-save').toggleAttribute('disabled', false);
    render();
    setStatus('', `已载入：${config.pets.length} 只宠物，素材 ${available.length} 条。（改动后点右下角保存）`);
    petLog(`设置: 已载入配置（${config.pets.length} 只宠物，${available.length} 条素材）`);
  } catch (err) {
    loaded = false;
    byId('btn-save').setAttribute('disabled', 'disabled');
    setStatus('error', `读取配置失败：${err instanceof Error ? err.message : String(err)}`);
    petLogError('设置: 读取配置失败', err);
  }
}

/** 保存：校验 + 落盘 + 立即生效都在宿主侧完成，这里只报结果 */
async function save(): Promise<void> {
  if (!config || !loaded) return;
  setStatus('', '正在保存并应用…');
  try {
    const result = await invoke<SaveSettingsDto>('save_settings', { config });
    document.body.classList.remove('dirty');
    const lines: string[] = [
      `已保存并立即生效：${result.petCount} 只宠物`,
      `配置文件：${result.path}`,
    ];
    if (result.backup) lines.push(`上一版备份：${result.backup}`);
    if (result.warnings.length > 0) {
      lines.push(`素材警告 ${result.warnings.length} 条（这些名字在素材目录里找不到，保存本身已成功）：`);
      for (const line of result.warnings.slice(0, 8)) lines.push(`  - ${line}`);
      if (result.warnings.length > 8) lines.push(`  …（其余 ${result.warnings.length - 8} 条见日志）`);
    }
    setStatus(result.warnings.length > 0 ? 'warn' : 'ok', lines.join('\n'));
    petLog(`设置: 保存成功（${result.petCount} 只宠物，${result.warnings.length} 条素材警告）`);
    // 保存后宠物窗是全新的：素材清单没变，但为了让 UI 与宿主完全对齐，重新拉一次
    await load();
    setStatus(result.warnings.length > 0 ? 'warn' : 'ok', lines.join('\n'));
  } catch (err) {
    setStatus('error', `保存失败：${err instanceof Error ? err.message : String(err)}`);
    petLogError('设置: 保存失败', err);
  }
}

/** 生成一个未被占用的宠物 id */
function nextPetId(): string {
  const used = new Set((config?.pets ?? []).map((pet) => pet.id));
  let index = (config?.pets.length ?? 0) + 1;
  while (used.has(`pet${index}`)) index += 1;
  return `pet${index}`;
}

// ============================================================================
//  启动
// ============================================================================

function bind(): void {
  setLogLabel('settings');

  byId('btn-save').addEventListener('click', () => void save());
  byId('btn-reload').addEventListener('click', () => void load());
  byId('btn-close').addEventListener('click', () => {
    void invoke<void>('close_settings').catch((err: unknown) => petLogError('设置: 关闭窗口失败', err));
  });
  byId('btn-open-file').addEventListener('click', () => {
    void invoke<void>('open_config_location').catch((err: unknown) => {
      setStatus('error', `打开配置文件所在目录失败：${err instanceof Error ? err.message : String(err)}`);
      petLogError('设置: 打开配置文件所在目录失败', err);
    });
  });

  byId('btn-add-pet').addEventListener('click', () => {
    if (!config) return;
    config.pets.push({
      id: nextPetId(),
      name: `宠物 ${config.pets.length + 1}`,
      size: config.pets[0]?.size ?? 462,
      idle: '',
      click: '',
      // 与上一只错开一点，否则新宠物会正好压在旧宠物身上（看着像"没生效"）
      position: { corner: 'bottom-right', marginX: 40 + config.pets.length * 24, marginY: 40 },
    });
    markDirty();
    render();
  });

  byId('btn-add-category').addEventListener('click', () => {
    if (!config) return;
    // actions 不能为空（Rust 侧校验会拒绝）：给一个真实存在的动画作为起点
    config.animations.categories.push({
      id: `新分类 ${config.animations.categories.length + 1}`,
      weight: 10,
      actions: [available[0] ?? config.animations.idle[0] ?? ''],
      noMirror: false,
    });
    markDirty();
    render();
  });

  byId('autostart').addEventListener('change', (event) => {
    const target = event.target as HTMLInputElement;
    const wanted = target.checked;
    void (async () => {
      try {
        const now = await invoke<boolean>('set_autostart', { enabled: wanted });
        target.checked = now;
        setStatus('ok', now ? '开机自启：已启用（写入当前用户启动项）' : '开机自启：已关闭');
        petLog(`设置: 开机自启 → ${now ? '启用' : '关闭'}`);
      } catch (err) {
        target.checked = !wanted;
        setStatus('error', `开机自启设置失败：${err instanceof Error ? err.message : String(err)}`);
        petLogError('设置: 开机自启设置失败', err);
      }
    })();
  });
}

bind();
void load();
