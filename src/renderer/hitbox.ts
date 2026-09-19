/**
 * 身体命中区几何。
 *
 * 语义（与上游 dsh-pet 完全一致）：宠物**只有身体**可以交互，四角的透明像素与
 * 窗口外扩余量一律不抢鼠标事件。命中框定义在 640×360 的动画画布坐标系里
 * （上游 `src/shared/constants.ts` 的 HIT_BOX = { x0:200, y0:50, x1:440, y1:335 }），
 * 这里按实际宠物尺寸等比换算到窗口内像素坐标。
 *
 * 为什么把换算收敛成一个纯函数：
 *   该结果同时被三处使用——Rust 侧兜底通道（盲判）、渲染端精确判定、
 *   以及 CSS 命中区元素尺寸。三处若各自算一遍，必然出现"看着能点、其实差几像素"的问题。
 */
import type { Rect } from '../bridge/contract.ts';

/** 上游动画画布尺寸常量（与 640×360 素材强耦合，不作为配置项） */
export const CANVAS_WIDTH = 640;
/** 上游动画画布高度 */
export const CANVAS_HEIGHT = 360;

/**
 * 上游身体命中框（640×360 画布内）。
 *
 * 直接复制自 `reference/shared/constants.ts` 的 HIT_BOX，注释标明来源：
 * 该常量属于「素材几何契约」，一旦上游调整必须同步（见 docs/SYNC.md）。
 */
export const CANVAS_HIT_BOX = { x0: 200, y0: 50, x1: 440, y1: 335 } as const;

/**
 * 把画布坐标系的命中框换算为包围盒内坐标（像素）。
 *
 * @param size 包围盒宽度（像素）；高度按 9/16 推出，`aspectRatio` 可覆盖
 * @param aspectRatio 高度/宽度比，默认 9/16
 * @returns 以**包围盒左上角为原点**的像素矩形
 */
export function scaleHitBox(size: number, aspectRatio = CANVAS_HEIGHT / CANVAS_WIDTH): Rect {
  const height = size * aspectRatio;
  return {
    x: (CANVAS_HIT_BOX.x0 / CANVAS_WIDTH) * size,
    y: (CANVAS_HIT_BOX.y0 / CANVAS_HEIGHT) * height,
    width: ((CANVAS_HIT_BOX.x1 - CANVAS_HIT_BOX.x0) / CANVAS_WIDTH) * size,
    height: ((CANVAS_HIT_BOX.y1 - CANVAS_HIT_BOX.y0) / CANVAS_HEIGHT) * height,
  };
}

/**
 * 判断窗口**本地坐标系**下的点是否落在身体命中区内。
 *
 * 本地坐标系 = 包围盒左上角为原点（`size × size*9/16`）。
 * 调用方负责先把屏幕坐标换算进来（见 runtime.ts 的 `toLocalPoint`）。
 */
export function isInsideHitBox(box: Rect, localX: number, localY: number): boolean {
  return localX >= box.x && localX < box.x + box.width && localY >= box.y && localY < box.y + box.height;
}

/**
 * 把命中框写进 DOM 元素样式（百分比定位，天然分辨率无关）。
 *
 * @param el 命中区元素
 * @param box 包围盒内坐标的命中框
 * @param size 包围盒宽度
 * @param aspectRatio 高度/宽度比，默认 9/16
 */
export function applyHitBoxStyle(el: HTMLElement, box: Rect, size: number, aspectRatio = CANVAS_HEIGHT / CANVAS_WIDTH): void {
  const height = size * aspectRatio;
  el.style.left = `${(box.x / size) * 100}%`;
  el.style.top = `${(box.y / height) * 100}%`;
  el.style.width = `${(box.width / size) * 100}%`;
  el.style.height = `${(box.height / height) * 100}%`;
}
