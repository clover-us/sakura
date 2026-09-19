# Tauri 应用配置说明（对应 `src-tauri/tauri.conf.json`）

> 为什么说明写在这里而不是写在 `tauri.conf.json` 里：
> Tauri 的配置解析器使用**严格模式**，任何未知字段都会导致构建失败
> （实测报错：`unknown field ... expected one of ...`）。
> 因此该 JSON 里不能放 `//说明` 这类注释键，字段含义统一记录在本文件里。

| 字段 | 值 | 说明 |
| --- | --- | --- |
| `productName` | `whale-pet` | 产品名，打包后的可执行文件名与安装目录名 |
| `version` | `0.1.0` | 应用版本（与 `package.json`、`Cargo.toml` 建议保持一致） |
| `identifier` | `com.whalepet.desktop` | 应用标识。**同时决定 WebView2 用户数据目录**：`%LOCALAPPDATA%\com.whalepet.desktop` |
| `build.devUrl` | `http://127.0.0.1:1420` | 开发模式前端地址，必须与 `vite.config.ts` 的 `server.port` 一致（`strictPort: true` 保证不会偷偷换端口） |
| `build.frontendDist` | `../dist` | 生产构建的前端产物目录（由 `pnpm run build` 产出） |
| `build.beforeDevCommand` | `pnpm run dev` | `tauri dev` 自动拉起 Vite |
| `build.beforeBuildCommand` | `pnpm run build` | `tauri build` 自动先构建前端（含类型检查） |
| `app.withGlobalTauri` | `true` | 把 `window.__TAURI__`（含 `core.invoke` / `event.listen`）注入页面。**本项目刻意不用 `@tauri-apps/api` npm 包**：少一层依赖，也避免 npm 包版本与 Tauri 核心版本不一致 |
| `app.windows` | `[]` | 刻意留空：宠物窗口**全部由 Rust 动态创建**（每只宠物一个局部小窗，尺寸/位置来自用户配置，静态配置无法表达） |
| `app.security.csp` | 见下方「CSP」 | 生产构建的 Content-Security-Policy（已收紧，不再是 `null`） |
| `app.security.devCsp` | 见下方「CSP」 | 开发模式的 CSP：与生产同一套，另加 Vite 的 `ws://127.0.0.1:1420` 与脚本 `'unsafe-inline'`（HMR 注入） |
| `bundle.targets` | `["nsis","msi"]` | Windows 上产出 NSIS 安装包与 MSI。本机沙箱无法从 GitHub 下载打包器时用 `tauri build --no-bundle` |
| `bundle.icon` | `icons/icon.ico` | M0 用脚本生成的占位图标；发布前用 `pnpm tauri icon <1024px.png>` 生成全套 |
| `bundle.windows.nsis.languages` | `["SimpChinese","English"]` | 中文安装界面优先 |

## CSP（M2 已收紧）

`app.security.csp`（生产）：

```text
default-src 'self';
script-src 'self';
style-src 'self' 'unsafe-inline';
img-src 'self' data: http://pet.localhost pet:;
media-src 'self' blob: http://pet.localhost pet:;
font-src 'self' data: http://pet.localhost pet:;
connect-src 'self' ipc: http://ipc.localhost http://pet.localhost pet:;
object-src 'none'; base-uri 'self'; form-action 'none'
```

每一项都是被实际需求逼出来的，改动前请先读：

| 指令 | 为什么必须有这一项 |
| --- | --- |
| `script-src 'self'` | 页面脚本全部是 Vite 产物（外链 module）。Tauri 自己注入的初始化脚本由 Tauri 自动补 hash，不需要放宽 |
| `style-src 'unsafe-inline'` | 三个页面都用 `<style>` 内联样式（菜单面板的 `MENU_CSS` 由页面注入 `<style>`） |
| `media-src http://pet.localhost pet:` | **动画走 `<video src>` 直连自定义协议**。少了它表现为"宠物不见了但没有任何报错"（透明窗里最难查的一类故障） |
| `connect-src ipc: http://ipc.localhost` | **Tauri v2 的 `invoke` 走 IPC 自定义协议**。少了它所有命令都会失败（页面只会表现为"点了没反应"） |
| `img-src` / `font-src` | 目前只用到 `data:`，`pet:` 是给后续的素材（表情包/字体）留的正规入口 |
| `object-src 'none'` / `base-uri 'self'` / `form-action 'none'` | 桌面端不需要插件、不需要改 base、不提交表单，直接关掉 |

平台差异：Windows / Android 的自定义协议表现为 `http://pet.localhost`，其它平台是 `pet://localhost`，
因此两种写法都放行（前端不做平台判断，统一用注入的 `assetBaseUrl`）。

`app.security.devCsp` 与生产同源，只多两处**开发专用**放行：`ws://127.0.0.1:1420`
（Vite HMR 的 WebSocket）与脚本的 `'unsafe-inline'`（HMR 注入的内联脚本）。
注意：Tauri 的规则是「设了 `devCsp` 就用它」，所以**发布前确认 `csp` 本身也是完整的**，
别只在 `devCsp` 里试通了就以为生产没问题。

> 验证方法见 `VERIFICATION.md` 的 M2 一节：**生产 CSP 必须在 release 二进制上验证**
> （debug 构建走 `devCsp` + `devUrl`，验证的不是同一套策略）。

## 自定义协议说明

动画素材通过自定义协议 `pet://` 提供给 webview，注册点在 `src-tauri/src/lib.rs`
（`register_asynchronous_uri_scheme_protocol`），必须登记为 privileged，
否则 webview 会把它当未知协议直接拒绝（表现为视频黑屏/不播放）。

平台差异：Windows / Android 是 `http://pet.localhost/<path>`，其它平台是
`pet://localhost/<path>`。前端不做平台判断，统一使用注入的 `assetBaseUrl`。
