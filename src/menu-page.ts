/**
 * 右键菜单窗口的前端入口。
 *
 * ## 为什么菜单是一个**独立窗口**
 *
 * 早期菜单画在宠物窗里，靠"临时扩大宠物窗"腾地方，代价是一整套坐标换算与随之而来的坑
 * （外扩方向、偏移换算、菜单开着再右键会偏移、换几何时人物虚影闪一下……）。
 * 现在菜单自成一个小窗：宠物窗**从头到尾不改几何**，菜单窗按右键点摆位、关掉就隐藏。
 *
 * ## 这个窗口与宿主的分工
 *
 * - 宿主（Rust）负责**摆窗口**：把右键点夹进工作区，算出窗口原点与"菜单在窗口内的内缩进"，
 *   然后用 `eval` 调本页的 `window.__whalePetMenu.show(insetX, insetY)`；
 * - 本页负责**画菜单**：取配置 → 建菜单树（上游 `shared/menu.ts`）→ 在内缩进处挂载；
 * - 菜单项被点击后：本页把动作回传给宿主（`menu_action`），宿主再转给宠物页执行
 *   （动画播放、回初始角落、显示气泡都发生在宠物页，因为那里才有动画链与包围盒）。
 *
 * 之所以用 `eval` 而不是事件：菜单页因此**不需要事件权限**，宿主也不必等回执。
 */
import { buildMenuTree, MENU_CSS, mountContextMenu, type ContextMenuMount, type MenuLeaf, type MenuNode } from '@shared/menu.ts';
import type { PetConfig } from './bridge/contract.ts';
import { petLog, petLogError, setLogLabel } from './bridge/log.ts';
import { invoke } from './bridge/tauri.ts';

/**
 * 在**上游菜单树之外**追加桌面端本地工具项（保持共享层 `menu.ts` 零改动）。
 *
 * 派发逻辑与上游一致：叶子项的 `anim` 由宠物页播放，`action` 由宠物页执行。
 * 这里有两个本地扩展 action（上游类型里没有，运行时按字符串分发）：
 *   - `say-demo`：显示一句测试气泡，用来确认独立气泡窗生效；
 *   - `home`：回到配置里的初始角落。
 *     **它以前是够不到的死代码**：上游 `buildMenuTree` 只产出"动画叶子"，不会产出 action，
 *     而 `handleAction` 里却写着 `action === 'home'` 的分支。现在把它真正挂进「工具」里，
 *     功能与文档（"右键菜单：分类点播动作、回到初始位置"）才对得上。
 */
function extendTree(base: MenuNode[]): MenuNode[] {
  return [
    ...base,
    {
      label: '工具',
      children: [
        { label: '显示一句气泡', action: 'say-demo' as MenuLeaf['action'] },
        { label: '回到初始位置', action: 'home' as MenuLeaf['action'] },
        { label: '说两句…', action: 'chat' as MenuLeaf['action'] },
        { label: '查余额', action: 'balance' as MenuLeaf['action'] },
      ],
    },
  ];
}

/** 菜单页的对外接口（宿主用 `eval` 调用） */
interface MenuWindowApi {
  /** 在当前窗口内 `(insetX, insetY)` 处弹出菜单（宿主已把窗口摆好） */
  show: (insetX: number, insetY: number) => void;
  /** 收掉菜单 DOM */
  close: () => void;
}

declare global {
  interface Window {
    __whalePetMenu?: MenuWindowApi;
  }
}

/**
 * 桌面端对共享样式的**唯一一处覆盖**：给级联面板再勾一圈 5% 黑的贴边描边。
 *
 * 为什么需要：共享的 `MENU_CSS` 只给了面板 `0 8px 28px` 的投影，压在浅色壁纸/白底上时
 * 边界不够利落（用户实测反馈"面板像一片没有边界的白"）。这层描边极淡，正常观感下几乎看不见。
 *
 * 为什么不直接改共享文件：`docs/COPY-MANIFEST.md` 的纪律是
 * `reference/shared/**` 与上游**逐字节一致**，需要适配就写在本仓库这一层
 * （与"拖拽弹簧 K/C 只在 `src/renderer/drag.ts` 里偏离"同一个做法）。
 * 追加在 `MENU_CSS` 之后 → 同优先级下后者生效，效果与直接改共享层等价。
 */
const MENU_CSS_OVERRIDE =
  '.dsh-pet-menu-column{box-shadow:0 0 0 1px rgba(0,0,0,.05),0 8px 28px rgba(0,0,0,.2)}';

/** 本窗口承载的宠物标签（由建窗时的 URL 给出） */
let petLabel = '';
/** 已挂载的菜单实例 */
let mount: ContextMenuMount | null = null;
/** 菜单树的数据来源（启动时取一次；配置在运行期不变） */
let petConfig: PetConfig | null = null;
/** 样式只注入一次 */
let cssInjected = false;
/**
 * 正在"为下一次弹出腾地方"而收掉旧菜单。
 *
 * 这种关闭**不能**再请宿主隐藏窗口：窗口马上就要重新显示，
 * 一个 `hide_menu` 会把刚弹出来的菜单立刻藏掉（同一类回声问题的另一面）。
 */
let cleaningForShow = false;

/**
 * 收掉菜单 DOM 并请宿主把窗口藏起来（页面主动关闭时调用：点了菜单项、鼠标移开菜单）。
 *
 * ## 幂等 + 不回声（关键）
 *
 * 只有在"确实挂着菜单"时才收 + 喊宿主。早期版本无条件 `invoke('hide_menu')`，
 * 而宿主的 `close()` 又会来调本页的 `close()`——两边互相调用形成**无限回声**，
 * 结果窗口被反复隐藏，**右键再也弹不出菜单**（用户实测反馈的 bug）。
 * 现在宿主那边已经不再回调本页（见 `menu_window.rs::close` 的说明），
 * 这里再加一道幂等，双重保险。
 */
function closeMenu(): void {
  if (cleaningForShow) {
    mount = null;
    return;
  }
  if (!mount) return;
  const mounted = mount;
  mount = null;
  mounted.close(); // 触发上游的 onClose（onClose 里只是再走一次本函数，已被上面的幂等挡住）
  void invoke('hide_menu', { label: petLabel }).catch((err: unknown) => petLogError('菜单: 隐藏失败', err));
}

/** 处理菜单项点击：把动作回传给宿主，由宿主转给宠物页执行 */
function handleAction(leaf: MenuLeaf): void {
  const anim = leaf.anim ?? null;
  const action = (leaf.action as string | undefined) ?? null;
  petLog(`菜单: 点播 anim=${anim ?? '(无)'} action=${action ?? '(无)'}`);
  void invoke('menu_action', { label: petLabel, anim, action }).catch((err: unknown) =>
    petLogError('菜单: 动作下发失败', err),
  );
}

/** 弹出菜单（宿主已把窗口摆到正确位置） */
function showMenu(insetX: number, insetY: number): void {
  if (!petConfig) {
    petLogError('菜单: 配置尚未就绪，忽略本次弹出', null);
    return;
  }
  // 幂等：上一次没收干净就先收掉，避免叠出第二份菜单。
  // 这次关闭只是"腾地方"——用 cleaningForShow 压住"请宿主隐藏窗口"的那个动作，
  // 否则刚弹出来的窗口会被自己立刻藏掉。
  if (mount) {
    cleaningForShow = true;
    mount.close();
    mount = null;
    cleaningForShow = false;
  }

  if (!cssInjected) {
    const style = document.createElement('style');
    style.textContent = MENU_CSS + MENU_CSS_OVERRIDE;
    document.head.appendChild(style);
    cssInjected = true;
  }

  const tree = extendTree(buildMenuTree(petConfig.animations));
  if (tree.length === 0) {
    petLog('菜单: 动画池为空，无可点播动作');
    closeMenu();
    return;
  }
  // clamp = 整个窗口：级联面板永远落在窗口内（窗口本身就是宿主按工作区夹好的）
  const w = Math.round(window.innerWidth);
  const h = Math.round(window.innerHeight);
  mount = mountContextMenu({
    tree,
    x: insetX,
    y: insetY,
    onAction: (leaf) => handleAction(leaf),
    onClose: () => closeMenu(),
    clamp: { x: 0, y: 0, w, h },
  });
  petLog(`菜单: 已弹出（窗口 ${w}×${h}，内缩进 (${insetX.toFixed(0)},${insetY.toFixed(0)})）`);
}

/** 页面启动 */
async function bootstrap(): Promise<void> {
  try {
    const label = new URLSearchParams(window.location.search).get('label');
    if (!label) throw new Error('菜单窗 URL 缺少 ?label=');
    petLabel = label;
    setLogLabel(label);
    petConfig = await invoke<PetConfig>('get_pet_config', { label });
    petLog(`启动: 菜单页就绪（宠物 ${label} 的动画池已取到）`);
    window.__whalePetMenu = { show: showMenu, close: closeMenu };
  } catch (err) {
    petLogError('菜单页启动失败', err);
  }
}

void bootstrap();
