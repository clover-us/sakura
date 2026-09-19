/**
 * 前端诊断日志：把关键节点写进应用数据目录下的 `pet-debug.log`（Rust 侧落盘）。
 *
 * 为什么必须有：
 *   桌宠是透明无边框窗口，"没反应"可能意味着很多层出问题（配置没读到 / 素材 404 /
 *   视频解码失败 / 命中判定没生效 …）。发布版没有控制台、窗口里也看不到提示，
 *   因此把页面侧的关键节点做成文件日志是**排障基础设施**，不是调试残留。
 *
 * 约定：
 *   - 只记"状态变化"与"失败"，绝不逐帧记录（否则日志会被刷爆）；
 *   - 日志失败**不抛错、不影响业务**（日志不能成为新的故障源）；
 *   - 失败只在控制台提示一次，避免刷屏。
 */
import { invoke } from './tauri.ts';

/** 本窗口的标签（由 main.ts 在启动第一步调用 setLogLabel 设置） */
let label = 'unknown';

/** 是否已经提示过日志写入失败（只提示一次） */
let warnedOnce = false;

/** 设置日志标签（窗口标签，多开时用于区分是哪只宠物） */
export function setLogLabel(next: string): void {
  label = next;
}

/**
 * 追加一条诊断日志（异步、尽力而为）。
 *
 * @param message 日志正文；建议写成"阶段: 细节"的形式，便于 grep
 */
export function petLog(message: string): void {
  void invoke<void>('pet_debug_log', { label, message }).catch((err: unknown) => {
    if (warnedOnce) return;
    warnedOnce = true;
    console.warn('[whale-pet] 诊断日志写入失败（后续不再提示）：', err);
  });
}

/** 记录一次失败（自动补上 `ERROR` 前缀，便于在日志里快速筛出） */
export function petLogError(message: string, detail?: unknown): void {
  const suffix = detail === undefined ? '' : ` | ${detail instanceof Error ? detail.message : String(detail)}`;
  petLog(`ERROR ${message}${suffix}`);
}
