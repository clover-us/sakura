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
| `app.security.dangerousDisableAssetCspModification` | `["style-src"]` | **只关掉 `style-src` 的 CSP 改写**（不塞 nonce）。原因见下方「运行期注入的 `<style>` 与 nonce」——不关它，右键菜单的 `MENU_CSS` 在生产构建里会被拦掉 |
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
| `style-src 'unsafe-inline'` | 页面都用 `<style>` 内联样式。**但光有它不够**：Tauri 会给内联 `<style>` 补 nonce，而 CSP 规定指令里一旦出现 nonce/hash，`'unsafe-inline'` 就被忽略 → 运行期注入的 `MENU_CSS` 会被拦。所以另外设了 `dangerousDisableAssetCspModification: ["style-src"]`，见下一节 |
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

### 运行期注入的 `<style>` 与 nonce（2026-09-20 修复）

生产构建里，`tauri-codegen` 会给 HTML 中**已有的**每个内联 `<style>` 打上
`nonce="__TAURI_STYLE_NONCE__"`，运行时 `tauri::manager::set_csp` 把它换成随机 nonce，
并往 `style-src` 追加 `'nonce-…'`。而 CSP 的规则是：**指令里一旦出现 nonce/hash，
`'unsafe-inline'` 即被忽略** —— 于是 `src/menu-page.ts` 在弹出右键菜单时用
`document.createElement('style')` 注入的 `MENU_CSS`（没有 nonce）在**生产构建里被拦掉**，
菜单退化成一排裸文字（详见 `VERIFICATION.md` 第 18 节）。

- 只有右键菜单中招：它是**唯一**在运行期注入样式的页面（托盘菜单 / 设置窗 / 气泡的样式都
  写在各自 HTML 的内联 `<style>` 里，天然带得上 nonce）；
- 开发模式**看不出来**：走 `devCsp` + Vite dev server，HTML 不经资源管线、没有 nonce；
- 修法就是本文件表格里那一行 `dangerousDisableAssetCspModification: ["style-src"]`：
  Tauri 不再注入 style 的 nonce token、也不再往 `style-src` 追加 nonce，策略回到
  `'self' 'unsafe-inline'` 的字面语义，静态与运行期注入的样式都放行。

**为什么这不是安全回退**：`style-src` 本来就声明了 `'unsafe-inline'`，此前那份 nonce
恰好把这句声明废掉了；关掉 style-src 的改写只是让**实际策略等于声明的策略**。
`script-src` 的 nonce/hash 机制完全不动——XSS 的风险面在脚本，不在样式。

> 另一条候选方案是"给注入的 `<style>` 抄上页面的 nonce"，没有选它，原因有二：
> ① 现代浏览器会把 `getAttribute('nonce')` 读到的值隐藏成空串（只能用 IDL 属性 `el.nonce`），
> 写错了会**静默失效**；② 它只修一处，以后谁再注入样式还会踩同一个坑。
> 如果哪天要恢复 style-src 的 CSP 改写，记得把 `menu-page.ts` 的注入改成用 `el.nonce` 兜底。

## 自定义协议说明

动画素材通过自定义协议 `pet://` 提供给 webview，注册点在 `src-tauri/src/lib.rs`
（`register_asynchronous_uri_scheme_protocol`），必须登记为 privileged，
否则 webview 会把它当未知协议直接拒绝（表现为视频黑屏/不播放）。

平台差异：Windows / Android 是 `http://pet.localhost/<path>`，其它平台是
`pet://localhost/<path>`。前端不做平台判断，统一使用注入的 `assetBaseUrl`。
