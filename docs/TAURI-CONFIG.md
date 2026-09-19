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
| `app.security.csp` | `null` | 见下方「CSP 待办」 |
| `bundle.targets` | `["nsis","msi"]` | Windows 上产出 NSIS 安装包与 MSI。本机沙箱无法从 GitHub 下载打包器时用 `tauri build --no-bundle` |
| `bundle.icon` | `icons/icon.ico` | M0 用脚本生成的占位图标；发布前用 `pnpm tauri icon <1024px.png>` 生成全套 |
| `bundle.windows.nsis.languages` | `["SimpChinese","English"]` | 中文安装界面优先 |

## CSP 待办（发布前必须处理）

M0 把 `app.security.csp` 置为 `null`（不限制），原因是：

- 开发模式下页面来自 `http://127.0.0.1:1420`，而动画素材来自自定义协议 `pet://`
  （Windows 上表现为 `http://pet.localhost`）；
- 这种"混合来源"下的 CSP 组合需要逐项实测（`media-src` / `img-src` / `connect-src`
  都要放行素材来源），否则会出现"视频不播但也不报错"的难查问题。

发布前应收紧为（示例，必须回归验证透明动画与拖拽仍可用）：

```
default-src 'self';
script-src 'self';
style-src 'self' 'unsafe-inline';
img-src 'self' http://pet.localhost data:;
media-src 'self' http://pet.localhost blob:;
connect-src 'self' http://pet.localhost
```

## 自定义协议说明

动画素材通过自定义协议 `pet://` 提供给 webview，注册点在 `src-tauri/src/lib.rs`
（`register_asynchronous_uri_scheme_protocol`），必须登记为 privileged，
否则 webview 会把它当未知协议直接拒绝（表现为视频黑屏/不播放）。

平台差异：Windows / Android 是 `http://pet.localhost/<path>`，其它平台是
`pet://localhost/<path>`。前端不做平台判断，统一使用注入的 `assetBaseUrl`。
