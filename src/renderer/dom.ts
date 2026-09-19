/**
 * 最小 DOM 工具：把"取元素"和"报错"收敛到一处。
 *
 * 为什么不直接 querySelector：
 *   - 页面里任何必需元素缺失都属于**致命配置错误**（HTML 与 TS 不同步），
 *     此处统一抛出带选择器的错误，避免后续出现 "Cannot read properties of null" 这类
 *     看不出根因的报错；
 *   - 类型上把 `T | null` 收窄成 `T`，业务代码不必到处写非空断言（noUncheckedIndexedAccess 友好）。
 */

/** 按选择器取必需元素；找不到即抛错（不静默兜底） */
export function requireElement<T extends Element>(selector: string): T {
  const el = document.querySelector<T>(selector);
  if (!el) throw new Error(`页面缺少必需元素：${selector}`);
  return el;
}

/** 右下角/左上角的红色错误条（DOM 已在 index.html 中声明） */
export function showFatalError(message: string): void {
  // 这里刻意用 console.error + 页面提示双通道：开发时看控制台，运行时用户看窗口
  console.error('[whale-pet] ' + message);
  const el = document.getElementById('pet-error');
  if (!el) return;
  el.textContent = '桌宠启动失败：' + message;
  el.classList.add('is-visible');
}

/** 隐藏错误条（配置重载成功后调用） */
export function hideFatalError(): void {
  const el = document.getElementById('pet-error');
  if (!el) return;
  el.classList.remove('is-visible');
  el.textContent = '';
}

/**
 * 用户是否要求减少动效（系统级无障碍设置）。
 * 与上游一致：为 true 时跳过 Q 弹挤压等装饰性动效，但保留拖拽等必要交互。
 */
export function prefersReducedMotion(): boolean {
  return window.matchMedia('(prefers-reduced-motion: reduce)').matches;
}
