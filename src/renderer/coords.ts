/**
 * 窗口本地坐标 ↔ 屏幕坐标 的换算原语。
 *
 * 桌面端坐标系有三套，混用必然错位，故把换算**只**放在这里：
 *   1. 屏幕坐标系：桌面左上角为原点（多显示器时可为负）。Rust 的窗口位置、光标位置、
 *      物理层（拖拽/抛掷）都在这一套里。
 *   2. 窗口内容区坐标系：窗口内容区左上角为原点。Rust 的 `set_pet_bounds` 用它。
 *   3. 宠物包围盒坐标系：包围盒（= 舞台 = 视频盒）左上角为原点，`0..size × 0..size*9/16`。
 *      命中判定与 Q 弹挤压锚点用它。
 *
 * 窗口 = 包围盒 + 四周外扩余量（`margin`，为气泡/弹窗预留），所以：
 *   包围盒屏幕坐标 = 窗口内容区屏幕坐标 + (margin, margin)
 */

/**
 * 窗口四周外扩余量（相对宠物宽度的比例）——与 Rust 侧 `WINDOW_MARGIN_RATIO` **必须一致**。
 *
 * **M0 取 0**：窗口尺寸 = 宠物包围盒尺寸，宠物周围不留空白。
 * 原因见 Rust 侧该常量的注释：窗口只要处于"可交互"态，整块矩形都在吃鼠标，
 * 留 0.5 的外扩会让用户觉得"宠物周围一大片点不到下层页面"。
 */
export const WINDOW_MARGIN_RATIO = 0.0;

/** 由宠物宽度算出四周外扩余量（像素，四舍五入） */
export function windowMargin(size: number): number {
  return Math.round(size * WINDOW_MARGIN_RATIO);
}

/** 窗口内容区尺寸（像素）：包围盒宽高 + 四周余量 */
export function windowSizeFor(size: number, aspectRatio: number): { width: number; height: number } {
  const margin = windowMargin(size);
  const boxHeight = Math.round(size * aspectRatio);
  return { width: size + margin * 2, height: boxHeight + margin * 2 };
}

/** 包围盒屏幕坐标 → 窗口内容区屏幕坐标（加上外扩余量） */
export function boxToWindow(box: { x: number; y: number }, size: number): { x: number; y: number } {
  const margin = windowMargin(size);
  return { x: Math.round(box.x) - margin, y: Math.round(box.y) - margin };
}

/** 窗口内容区屏幕坐标 → 包围盒屏幕坐标（减去外扩余量） */
export function windowToBox(origin: { x: number; y: number }, size: number): { x: number; y: number } {
  const margin = windowMargin(size);
  return { x: origin.x + margin, y: origin.y + margin };
}

/** 屏幕坐标 → 包围盒本地坐标（原点 = 包围盒左上角） */
export function screenToLocal(point: { x: number; y: number }, box: { x: number; y: number }): { x: number; y: number } {
  return { x: point.x - box.x, y: point.y - box.y };
}
