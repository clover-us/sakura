/**
 * Tauri 全局对象的最小类型声明与访问封装。
 *
 * 为什么不用 `@tauri-apps/api` 包：
 *   tauri.conf.json 里开启了 `withGlobalTauri`，webview 会在页面加载前注入
 *   `window.__TAURI__`，与 npm 包是同一套实现。直接用它可以让前端零运行时依赖，
 *   也避免 npm 包版本与 Tauri 核心版本不一致导致的隐蔽问题（本项目依赖越少越好）。
 *
 * 本文件是**唯一**接触 window.__TAURI__ 的地方：其它模块只 import 这里的函数，
 * 便于日后换成官方 npm 包（只改这一个文件），也便于在纯浏览器里做界面调试。
 */

/** 后端 emit 的事件载荷（Tauri 事件对象的最小形状） */
interface TauriEvent<T> {
  payload: T;
}

/** Tauri 注入的全局对象（只声明本项目用到的部分） */
interface TauriGlobal {
  core: {
    /** 调用 Rust 侧 #[tauri::command]：命令名 → 参数对象 → 返回值 */
    invoke<T>(command: string, args?: Record<string, unknown>): Promise<T>;
  };
  event: {
    /** 监听后端 emit 的事件；返回取消监听的函数 */
    listen<T>(event: string, handler: (e: TauriEvent<T>) => void): Promise<() => void>;
  };
}

declare global {
  interface Window {
    __TAURI__?: TauriGlobal;
    /** Rust 侧在页面加载前注入的进程级环境（见 src-tauri/src/pet_init.js 与 lib.rs） */
    __PET_ENV__?: PetEnvironment;
  }
}

/** Rust 侧注入的环境信息（页面加载前即可用，故同步可用、无需 await） */
export interface PetEnvironment {
  /** 资源根地址，形如 `pet://localhost`；拼成 `${assetBaseUrl}/webm/<文件>` 使用 */
  assetBaseUrl: string;
}

/**
 * 取注入的环境；缺失即视为致命错误。
 *
 * 刻意不返回默认值：拿不到环境说明注入脚本没生效（窗口参数写错 / 版本不匹配），
 * 此时靠猜一个地址继续跑只会得到难以定位的加载失败，不如立刻大声报错。
 */
export function readPetEnvironment(): PetEnvironment {
  const env = window.__PET_ENV__;
  if (!env || typeof env.assetBaseUrl !== 'string' || env.assetBaseUrl.length === 0) {
    throw new Error('未注入 window.__PET_ENV__（assetBaseUrl 缺失），无法解析动画资源地址');
  }
  return env;
}

/** 调用 Rust 命令；Tauri 未注入时抛出可读错误（而不是 undefined 报错） */
export async function invoke<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  const tauri = window.__TAURI__;
  if (!tauri) {
    throw new Error(`Tauri 全局对象不可用，无法调用命令 ${command}（withGlobalTauri 是否开启？）`);
  }
  return tauri.core.invoke<T>(command, args);
}

/** 监听后端事件；返回取消监听函数 */
export async function listen<T>(event: string, handler: (payload: T) => void): Promise<() => void> {
  const tauri = window.__TAURI__;
  if (!tauri) {
    throw new Error(`Tauri 全局对象不可用，无法监听事件 ${event}`);
  }
  return tauri.event.listen<T>(event, (e) => handler(e.payload));
}
