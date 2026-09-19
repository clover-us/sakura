/**
 * Vite 配置（前端构建 / 开发服务器）。
 *
 * 与 Tauri 的约定（见 src-tauri/tauri.conf.json 的 build 段）：
 *   - devUrl      = http://localhost:1420  → 这里的 server.port 必须一致，且 strictPort
 *   - frontendDist = ../dist               → 这里的 build.outDir
 *
 * 目录约定：
 *   - 构建入口是仓库根的 index.html（Tauri 的 URL 与 file:// 加载都以它为根）
 *   - **bubble.html 也是入口**：气泡是独立小窗，页面由宿主用 `WebviewUrl::App("bubble.html")`
 *     加载。Vite 默认只把 index.html 当入口，不声明的话开发服务器能看到这个文件，
 *     但**生产构建里根本不会产出它**——`tauri build` 出来的应用点开气泡只会得到 404 空窗。
 *   - base 必须是相对路径 './'：生产环境页面经 tauri://localhost 加载，绝对路径会 404
 */
import { defineConfig } from 'vite';
import { fileURLToPath } from 'node:url';

/** 仓库根目录（本配置文件所在目录） */
const rootDir = fileURLToPath(new URL('.', import.meta.url));

export default defineConfig({
  // 生产构建用相对路径引用产物（Tauri 打包后不是从域名根提供的）
  base: './',

  // 开发服务器：端口与 tauri.conf.json 的 devUrl 严格一致，禁止自动换端口
  server: {
    port: 1420,
    strictPort: true,
    // Tauri 的 webview 从 tauri://localhost 访问 dev server，按 IP 直连即可
    host: '127.0.0.1',
  },

  // 构建产物：交给 Tauri 的 frontendDist 打包进应用
  build: {
    outDir: 'dist',
    emptyOutDir: true,
    // 桌宠对首帧延迟敏感，关掉体积优化换取更小的启动解析开销
    minify: false,
    sourcemap: true,
    target: 'chrome110',
    rollupOptions: {
      // 多页面入口：宠物窗（index.html）+ 气泡窗（bubble.html）+ 菜单窗（menu.html）
      input: {
        main: `${rootDir}index.html`,
        bubble: `${rootDir}bubble.html`,
        menu: `${rootDir}menu.html`,
      },
    },
  },

  resolve: {
    alias: {
      // 拷贝自 dsh-pet 的纯逻辑层：统一入口，日后换 npm 包 / submodule 只改这一处
      '@shared': `${rootDir}reference/shared`,
    },
  },
});
