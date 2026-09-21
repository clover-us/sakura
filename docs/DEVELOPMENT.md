# 开发文档

面向**改这个仓库的人**：环境、跑起来、出包、冒烟与探针、配置、目录结构、设计要点、当前状态与已知限制。

- 只想装来用：看 [项目介绍（README）](../README.md)
- 每条"已完成"的证据、踩过的坑与复盘：看 [验证记录](VERIFICATION.md)
- 与上游的关系（拷了什么、为什么、怎么同步）：看 [拷贝清单](COPY-MANIFEST.md)
- `tauri.conf.json` 逐字段说明与 CSP 的每条理由：看 [Tauri 配置说明](TAURI-CONFIG.md)

---

## 一、环境

本机是一套**自包含的便携工具链**（全部在 `D:\tools\Tauri`，不改系统 PATH）：

| 组件 | 版本 | 位置 |
| --- | --- | --- |
| Rust / cargo | 1.98.1（目标 `x86_64-pc-windows-gnu`） | `D:\tools\Tauri\cargo\bin` |
| Node.js | v22.23.2 | `D:\tools\Tauri\nodejs` |
| GCC / binutils | 16.2.0 / 2.47 | `D:\tools\Tauri\msys64\mingw64\bin` |
| Tauri CLI | 2.11.4 | `D:\tools\Tauri\nodejs\tauri.cmd` |
| WebView2 运行时 | 153.0.4234.32 | 系统自带 ✅ |

**先激活环境**（两种方式任选）：

```powershell
# 方式一（推荐）：双击打开一个已激活的 PowerShell
D:\tools\Tauri\TauriShell.cmd

# 方式二：在当前会话里激活本项目的封装脚本
powershell -ExecutionPolicy Bypass -NoExit -File scripts\activate-env.ps1
```

> 工具链不在 `node_modules` 里，所以 Tauri CLI 一律走 `D:\tools\Tauri\nodejs\tauri.cmd`；
> `pnpm tauri ...` 在本机不可用。

## 二、跑起来

```powershell
cd D:\programs\deepseek\sakura
pnpm install
pnpm tauri dev          # 开发模式（热更新；自动拉起 Vite，端口 1420）
```

宠物默认出现在**屏幕右上角**。首次运行自动在 `%APPDATA%\com.whalepet.desktop\` 生成配置与两条示例动画。

## 三、出包（生成完整安装包）

安装包**自带全部素材**（106 条动画 51.9 MB + 27 张表情包 4.6 MB + 气泡字体 3.9 MB）：

```powershell
# ① 把上游素材暂存进仓库 assets/（约 57 MB；素材不入库，见 .gitignore，克隆后要重新跑）
pwsh -File scripts\import-animations.ps1 -Stage

# ② 出包。**必须带 --target**（原因见下）：exe + NSIS 安装器 + MSI
& 'D:\tools\Tauri\nodejs\tauri.cmd' build --target x86_64-pc-windows-gnu
#   只出某一种：--bundles nsis / --bundles msi；只出 exe：--no-bundle

# ③ 产物
#   src-tauri\target\x86_64-pc-windows-gnu\release\whale-pet-desktop.exe          免安装可执行
#   src-tauri\target\x86_64-pc-windows-gnu\release\bundle\nsis\whale-pet_0.1.0_x64-setup.exe   62.3 MB（推荐）
#   src-tauri\target\x86_64-pc-windows-gnu\release\bundle\msi\whale-pet_0.1.0_x64_zh-CN.msi    63.6 MB
```

> 第 ② 步必须在**已激活环境**的会话里跑（见 §一）。系统里那个 `cargo`（`C:\Users\admin\.cargo`）
> 只装了 msvc 目标，直接跑会报 `Target x86_64-pc-windows-gnu is not installed`
> （实测：2026-09-21 换图标后重新出包时撞过一次，`rustup target list --installed` 只有
> `x86_64-pc-windows-msvc`；gnu 那套在便携工具链 `D:\tools\Tauri\rustup` 里）。
>
> 出包时 Tauri 会**在原地给那个 release exe 打上"bundle 类型"标记**（nsis / msi 各打一次），
> 所以 `--no-bundle` 出的 exe 与打完包的 exe 不是逐字节相同；直接拷出去跑都一样能用，
> 只是它记得自己"是某个安装包的一部分"。

**改版本号要四处一起改**：`src-tauri/Cargo.toml`、`src-tauri/tauri.conf.json`（出包名取它）、
`package.json`、以及设置页「关于」里那一行（`src/settings-page.ts` 的 `whale-pet <版本>`）。

> 这四份文件都带中文，**别用 PowerShell 的 `Get-Content`/`Set-Content` 改**：Windows PowerShell 5.1
> 按 ANSI(GBK) 读无 BOM 的 UTF-8，再按 UTF-8 写回去，中文会变成乱码（而 `-Encoding UTF8` 还会
> 顺手加个 BOM，serde_json 见到 BOM 直接解析失败）。2026-09-21 出 1.0.0 时真踩了一次，
> 只好 `git checkout` 还原后逐行改。用编辑器改，或者用 `.NET` 的
> `[System.IO.File]::ReadAllText($p, [Text.Encoding]::UTF8)` + `WriteAllText(..., UTF8Encoding($false))`。
>
> 用 .NET 那套时还有两个坑，同一天连着踩：
>   - **相对路径是按"进程工作目录"解析的**，不是 PowerShell 的当前位置（`cd` 不影响它）。
>     写 `'..\package.json'` 会落到仓库外面去；
>   - **`WriteAllText($p, $null)` 会创建一个 0 字节文件**。造出来的空 `package.json` 会一路向上
>     被 vite/esbuild 找到，构建报 `Unexpected end of file in JSON`（排查花了十几分钟，
>     最后是 `Get-ChildItem D:\programs\deepseek` 看到那个 0 字节文件）。
>
> 另外 `.ps1` 里出现中文必须存成 **UTF-8 with BOM**（见 §五 末尾）；临时脚本干脆写成纯 ASCII 最省事。

**素材怎么进安装包**：`tauri.conf.json` 的 `bundle.resources` 把 `assets/{webm,memes,pic,fonts}`
映射进资源目录；首次启动由 `src-tauri/src/assets_seed.rs` **只补缺失地**释放到数据目录
（同名文件已存在就跳过——用户改过/删掉的素材不会被覆盖或复生）。

**出包踩过的四个坑**（都已修，写在这里省下一轮）：

1. **NSIS 会漏掉 `WebView2Loader.dll`，必须带 `--target x86_64-pc-windows-gnu`**。
   GNU 工具链下 `webview2-com-sys` 链接的是 `WebView2Loader.dll`（MSVC 才是静态链接），
   而打包器只在"它认识的目标三元组以 `-gnu` 结尾"时才把这个 DLL 加进 NSIS；
   不带 `--target` 时 Tauri CLI 会用**自己的编译期 cfg** 拼出 `x86_64-pc-windows-msvc`，
   于是 NSIS 安装包少了它 → 装完启动报「由于找不到 WebView2Loader.dll，无法继续执行代码」。
   MSI 那条路径是"扫 `target\...\*.dll`"，不看三元组，所以不受影响。
2. **MSI 报 `LGHT0311`：代码页 1252 装不下中文素材文件名**（`待机呼吸休闲.webm`、`可爱.png`…）。
   已设 `bundle.windows.wix.language = "zh-CN"`（代码页 936）；NSIS 用 Unicode，本来没这个问题。
   副作用是产物名从 `…_en-US.msi` 变成 `…_zh-CN.msi`。
3. **右键菜单在生产构建里会丢样式**：Tauri 的资源管线会给 HTML 里的内联 `<style>` 补 nonce，
   而 CSP 规定"指令里出现 nonce/hash 时 `'unsafe-inline'` 被忽略"，于是页面**运行期注入**的
   菜单样式（`MENU_CSS`）被拦掉，菜单只剩裸文字。已用
   `app.security.dangerousDisableAssetCspModification = ["style-src"]` 修掉——
   该指令本来就写着 `'unsafe-inline'`，关掉 style-src 的改写没有安全回退。
4. **`import-animations.ps1` 必须保持 UTF-8 BOM**：脚本里有中文，PowerShell 5.1 对无 BOM 的
   UTF-8 会按 GBK 读，整份脚本直接解析失败。用 PowerShell 改写时记得
   `New-Object System.Text.UTF8Encoding($true)`。

> 坑 1 / 2 的实测记录见 [VERIFICATION.md](VERIFICATION.md) 第 17 节，坑 3 的根因链与证据见第 18 节。
> 另外：`pnpm tauri:build` 走的是 `--no-bundle`（只出 exe，**不产安装包**），别拿它当出包命令。
> 安装包**未做代码签名**，装的时候 Windows 会提示「未知发布者」。
> 为什么没签、四条路各要多少钱、拿到证书后 `bundle.windows` 具体加哪几个字段（含自签演练
> 与 signtool 从哪来），都记在 [`SIGNING.md`](SIGNING.md)。

## 四、冒烟检查与图标

```powershell
cd src-tauri
cargo run --example logic-smoke  # 115 项断言：配置解析/校验/写回、每宠独立动画池、路径防穿越、
                                 # JSONC 注释、几何换算、余额档位、表情包标记、记忆文件容错、版本号一致性…
                                 # （放在 examples/ 而不是 bin/：bin 目标会被 Tauri 打进安装包）
cargo run --example make-icon    # 重新生成 icons/ + src/assets/app-logo.png
                                 # 图形源在 src-tauri/icons/design/：默认 app-icon.png（位图），
                                 # 同目录的 app-icon-<尺寸>.png 会被当作该尺寸的原图直接使用；
                                 # 也可以 --svg <路径> / --png <路径> / --sheet 换源或只出对比图
```

## 五、诊断日志与探针

透明无边框窗口出问题时的表现往往只是"看不见宠物"或"点不动"——没有界面可看，发布版也没有控制台。
因此前端把关键节点写进 `%APPDATA%\com.whalepet.desktop\pet-debug.log`：

```
[d20714 18:41:09.030][pet-main-0] 动画: 首帧就绪 readyState=4 尺寸=640x360 src=...
[d20714 18:41:09.034][pet-main-0] 命中: 首次进入身体 光标=(2140,336) 包围盒=(1930,210)
[d20714 18:41:09.541][pet-main-0] 松手: 甩出 速度=(-3214,1607)px/s 位置=(1415,514)
[d20714 18:41:20.250][pet-main-0] 飞行: 结束于 (2086,1156)，落地冲击 2083px/s
```

**受控自测与探针**（走真实代码路径，默认不启用；真实鼠标注入在自动化环境里不可靠）：

```powershell
$env:WHALE_PET_AUTOTEST = '1'; pnpm tauri dev   # 拖拽 + 甩抛全链路（弹簧滞后/初速/落地）
$env:WHALE_PET_AUTOTEST = '2'; pnpm tauri dev   # "拖到命中区外松手"的失控场景（松手事件丢失的自愈）

# 设置窗口：打开同一个 settings_window::open()，再由页面自己点按钮（页面 → 命令 → 写盘 → 重建）
$env:WHALE_PET_DIAG_SETTINGS = 'save'           # 1 | save | autostart | addpet | delpet | ownbehaviour | nav:physics
# 托盘菜单：弹出真实菜单窗（截图用），或按序走一遍动作层
$env:WHALE_PET_DIAG_TRAY_MENU = '5000:picker'   # 5 秒后弹出并展开"动作点播"
$env:WHALE_PET_DIAG_TRAY_MENU = '5000:toast'    # 或点一下「查余额」，弹出失败提示（验证提示有没有被窗口裁掉）
$env:WHALE_PET_DIAG_TRAY = 'toggle,home,anim:待机呼吸休闲,settings'
$env:WHALE_PET_DIAG_CURSOR = '1'                # 逐帧对照"宿主采样坐标"与"页面 DOM 坐标"
pnpm tauri dev
```

界面类改动靠**截图**验证（WebView2 的内容 `PrintWindow` 抓不到，脚本走屏幕合成截图）：

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\capture-window.ps1 -TitleLike '设置'
```

| 脚本 | 用途 |
| --- | --- |
| `scripts/capture-window.ps1` | 按窗口标题抓屏幕合成截图（先提前台，所以**别拿它判断"窗口是否被遮挡"**） |
| `scripts/probe-window-styles.ps1` | 按节奏采样本进程全部顶层窗口的类名/矩形/扩展样式位（含子窗口树） |
| `scripts/diagnose-menu-zorder.ps1` | 一次快照：顶层窗口的 Z 序 / 尺寸 / 位置 / `TOPMOST`、`LAYERED`、`TRANSPARENT`、`NOACTIVATE`，并给"菜单窗是否在宠物窗之上"下结论；顺带打印进程路径、DPI、显卡、系统 |
| `scripts/watch-menu-zorder.ps1` | 连续采样同一批窗口的 Z 序，只在变化时打印——抓"右键那一下谁被抬起来了" |
| `scripts/test-bubble-click-through.ps1` | 气泡窗点击归属的 A/B 对照 |

> `.ps1` 里有中文的脚本必须存成 **UTF-8 with BOM**（PowerShell 5.1 按 ANSI 读无 BOM 的 UTF-8，
> 中文注释会把引号吞掉并报 `TerminatorExpectedAtEndOfString`）。

**排查残留会攒得很快**：截图、日志、一次性脚本默认都丢在 `src-tauri\target\`（那里被 gitignore，
所以 `git status` 一直是干净的，看着不脏但占地方）。2026-09-21 清了一次：顶层攒了 **180 个**
散落文件（82 个 `.log`、64 个 `.png`、19 个 `.ps1`…），另有 `debug\incremental` **5.7 GB** 与
一个没人用的 msvc `target\release`（3.3 GB，正式出包走 gnu）。清法：

```powershell
# ① 只清排查残留：保留构建缓存与 cargo 的元数据
Get-ChildItem src-tauri\target | Where-Object {
  $_.Name -notin @('debug', 'x86_64-pc-windows-gnu', 'CACHEDIR.TAG', '.rustc_info.json')
} | Remove-Item -Recurse -Force

# ② 增量编译缓存（纯缓存，删了下次构建慢一点）
Remove-Item src-tauri\target\debug\incremental -Recurse -Force

# ③ msvc 的 release 缓存（本项目的出包走 gnu，见 §三）
Remove-Item src-tauri\target\release -Recurse -Force
```

> `target\debug\deps`（10.8 GB）是**依赖编译产物**，删了下次 `pnpm tauri dev` 要全量重编
> （十分钟量级），想省这 10 GB 再删。

## 六、配置

完整模板与逐字段注释见 [`config/default-config.jsonc`](../config/default-config.jsonc)；
运行期的文件位置与目录说明见 [README 的「数据目录」](../README.md#33-数据目录与配置)。

```jsonc
{
  "schemaVersion": 1,
  "physics": { "gravity": 1400, "restitution": 0.78, "groundFriction": 2.5,
               "ceilingBounce": true, "throwPower": 1.0, "petCollision": false },
  "pets": [
    { "id": "main", "name": "小鲸鱼", "size": 420,
      "idle": "待机呼吸休闲.webm", "click": "点击回应-元气挥手.webm",
      "position": { "corner": "top-right", "marginX": 24, "marginY": 100 } }
  ]
}
```

- `pets` 数组**每一项 = 一个独立的透明置顶窗口**（多开就多写几项，`id` 必须唯一；也可在设置窗口增删）
- `idle` / `click` 留空字符串 = 自动挑选目录里排序最靠前的动画
- 每只宠物可写 `animations` / `animationWeights` **整体覆盖**全局默认；不写则跟随全局
- 改完配置 1 秒内热重载；改坏了保留当前配置继续跑，并把原因写进日志
- 从设置窗口保存会整体重写文件（JSON 不存注释），取舍理由见 `VERIFICATION.md` 9.4

## 七、目录结构

```
.
├─ index.html                     桌宠窗口页面（透明布局 + 双缓冲 video + 命中区）
├─ bubble.html / menu.html        气泡窗 / 右键菜单窗页面（都是独立小窗）
├─ settings.html                  设置窗口页面（左侧导航 + 跟随系统深浅色）
├─ tray-menu.html / chat.html     托盘菜单页 / 对话输入窗页
├─ src/
│  ├─ main.ts                     桌宠页入口：取配置 → 建通道 → 装配运行时
│  ├─ menu-page.ts                右键菜单页：上游菜单树 + 桌面端本地工具项 + 样式注入
│  ├─ tray-menu-page.ts           托盘菜单页：主菜单 / 动作点播，动作交给宿主执行
│  ├─ settings-page.ts            设置页：整份配置进出，改哪几个字段就动哪几个
│  ├─ chat-page.ts                对话输入窗页
│  ├─ bridge/                     tauri.ts（`window.__TAURI__` 唯一入口）/ contract.ts（数据契约）/ log.ts
│  ├─ assets/app-logo.png         应用图标（make-icon 的产物，页头 logo 直接引用它）
│  └─ renderer/
│     ├─ runtime.ts               核心装配：位置真相、命中判定、窗口跟随、输入状态机
│     ├─ chain.ts                 动画链（掷骰选下一段、转向、随机动作、走路计划）
│     ├─ drag.ts / anim.ts        拖拽弹簧与松手初速 / Q 弹挤压与甩抛飞行
│     ├─ media-buffer.ts          双缓冲透明视频播放（零空白帧切换）
│     ├─ hitbox.ts / coords.ts    命中区几何 / 三套坐标系的换算收口
│     ├─ cursor.ts / dom.ts       全局光标采样通道 / 最小 DOM 工具 + 大声报错
│  ├─ src-tauri/
│  │  ├─ src/
│  │  │  ├─ lib.rs                启动装配：插件 → 协议 → 配置 → 几何 → 窗口 → 托盘 → 轮询线程
│  │  │  ├─ pet_window.rs         窗口创建/定位/移动/穿透翻转（每宠一个局部小窗）
│  │  │  ├─ bubble.rs             气泡窗：不可聚焦 + 整窗点击穿透 + 跟头顶定位
│  │  │  ├─ menu_window.rs        右键菜单窗：摆位（夹进工作区）、外点关闭
│  │  │  ├─ chat_window.rs        对话输入窗（唯一可聚焦的辅助窗）
│  │  │  ├─ tray.rs / tray_menu.rs 托盘图标与动作分发 / 自绘托盘菜单窗（按内容变高）
│  │  │  ├─ settings_window.rs    设置窗口的建/显/关
│  │  │  ├─ reload.rs             配置热重载与"保存即生效"（拆窗重建）
│  │  │  ├─ config.rs             配置读取/校验/JSONC 注释剥离/写回/每宠池解析
│  │  │  ├─ pet_protocol.rs       自定义协议 pet://（动画/表情包/字体等本地素材）
│  │  │  ├─ display.rs            显示器几何 + 变化轮询（工作区并集，不是外接矩形）
│  │  │  ├─ assets_seed.rs        随包素材"只补缺失"地释放到数据目录
│  │  │  ├─ llm.rs / whisper.rs    LLM 适配层（OpenAI 兼容/DeepSeek/Ollama/自定义）/ 碎碎念
│  │  │  ├─ balance.rs / memes.rs  余额与用量查询 / 表情包池与选图
│  │  │  ├─ memory.rs / secret.rs  对话记忆 / Key（DPAPI 加密存储）
│  │  │  ├─ model.rs / contract.ts 数据契约（Rust ↔ 前端一一对应）
│  │  │  ├─ commands.rs / state.rs 前端可调用的命令 / 共享状态
│  │  │  ├─ watchdog.rs           取锁/窗口操作计时 + 主线程健康看门狗
│  │  │  ├─ diagnostics.rs        诊断日志落盘 + 受控自测/探针入口
│  │  │  └─ src/bin/              （已清空：开发工具都挪到 examples/，免得被打进安装包）
│  │  ├─ examples/logic-smoke.rs  纯逻辑冒烟（115 项断言，`cargo run --example logic-smoke`）
│  │  ├─ examples/mock-llm.rs     本地假 LLM 端点（`cargo run --example mock-llm`）
│  │  ├─ examples/make-icon.rs    图标生成：图形源（SVG/PNG）→ png/ico/托盘 RGBA/前端 logo（resvg 只在 dev-dependencies）
│  │  ├─ icons/                   design/*.svg 图形源 + 生成物
│  │  └─ capabilities/default.json 最小权限集
├─ reference/shared/              ← 从上游逐字节拷贝的纯逻辑（**零改动**）
├─ config/default-config.jsonc    默认配置模板（编译进 exe，首次运行释放）
├─ assets/webm/                   内置示例动画（编译进 exe，保证"装完就有东西看"）
├─ scripts/                       环境激活、素材导入、窗口探针/截图、Z 序诊断、动画池同步
└─ docs/                          拷贝清单、验证记录、路线图、Tauri 配置说明、LLM 设计、截图、本文档
```

## 八、设计要点（为什么这么做）

这套实现里有几个**不能想当然**的地方，都写在对应源码的注释里，这里给索引：

1. **点击穿透必须换方案**（`pet_window.rs::apply_fallback_hit` + `cursor.ts`）
   Electron 的 `setIgnoreMouseEvents(true, { forward: true })` 在穿透时仍转发鼠标移动，页面能自己发现
   "光标进来了"；**Tauri 没有 forward**，穿透后页面收不到任何事件。所以改为：Rust 每 16ms 轮询全局光标 →
   事件下发 → 前端做精确命中判定 → 翻转穿透。Rust 侧只做"恢复可交互"这一个方向，绝不主动翻回穿透。

2. **跟手坐标用 DOM 事件，不用宿主的光标采样**（`runtime.ts::bindDomInput`）
   曾经的结论是"宿主采样不可信"——**后来被复测推翻了**：用 `WHALE_PET_DIAG_CURSOR=1` 逐帧对照，
   两条链路逐像素一致。当年看着不一致，是因为拿 `SetCursorPos` 设定的位置当"真实光标"，
   而本机指针被桌面环境持续驱动、几百毫秒内就被挪走了。现在仍用 DOM 坐标，理由换成更硬的一条：
   DOM 坐标是**按下那一刻的真实位置**且零延迟，而采样通道要走"宿主轮询(16ms) → IPC → 页面"。
   （更正过程见 [VERIFICATION.md](VERIFICATION.md) 第 6.4 节。）

3. **看门狗不能只看按键位，且拖拽必须做指针捕获**（`runtime.ts::onCursorFrame` / `beginPress`）
   本机 `GetAsyncKeyState(VK_LBUTTON)` 会**误报"已松开"**：单独用它当收尾依据，真实按下会在 2ms 内被收尾
   （表现为"刚按住就脱手"）。现在的判据是三重条件（按键位报松开 + 该状态早于本次按下 + DOM 指针事件静默 ≥250ms），
   并用 `setPointerCapture` 把拖拽期间的指针事件锁在宠物窗口。

4. **宠物窗口必须 `focusable(false)`**（`pet_window.rs::create_one`）
   否则鼠标按下会激活窗口并抢前台焦点，拖拽结束后焦点仍留在宠物上，点别的窗口**第一次点击只被用来切前台**
   （宠物在屏幕中央、透明窗覆盖大片区域时尤其明显）。不可聚焦后照常收鼠标消息但永不激活。

5. **窗口尺寸必须等于宠物包围盒**（`WINDOW_MARGIN_RATIO = 0`）
   窗口只要可交互，**整块矩形**都在吃鼠标。早期按 0.5 比例外扩，用户感受就是"形象周围一大片空白点不到下层"。
   代价是气泡（头顶）与右键菜单需要宠物**周围**的空间——两者都已改成**各自单独开一个窗**，
   宠物窗因此从头到尾不改几何，连带消掉了"外扩坐标换算、重复右键偏移、换几何时人物虚影"那一整类问题。

6. **每只宠物一个小窗，不用全屏透明窗**：Windows DWM 在"全屏透明 + 视频层"下会黑屏（上游实测），小窗不黑。

7. **busy 期间绝不翻回穿透**（`runtime.ts::reportBusy`）
   拖拽时宠物由弹簧追赶光标、**滞后**于光标，按几何盲判会在光标滑出包围盒时误判成"用户离开了"，
   正在进行的拖拽当场断掉。这条铁律继承自上游的 `setInputBusy`。

8. **位置的唯一真相在 Rust**（`contract.ts::PetRuntime`）
   前端只上报"我希望窗口去哪"，Rust 移动后回传真实位置，前端再据此换算；两边各自记账必然累积误差。

9. **三套坐标系只在 `coords.ts` 换算**：屏幕坐标系（物理/光标/窗口位置）、窗口内容区坐标系
   （`set_pet_bounds`）、包围盒坐标系（命中判定/Q 弹锚点）。混用一次就是半个身位的错位。

10. **抛掷边界用逐屏工作区并集，不用外接矩形**：显示器摆放不规则时外接矩形含大片空洞，
    宠物会飞进看不见的地方（上游实测一例占 23.7%）。

11. **`reference/shared` 保持零改动**：需要适配就在 `src/` 里包一层（例如 `drag.ts` 把 shared 的
    `(vx, vy)` 命名转换成本项目的契约 `(x, y)`），这样上游修 bug 时能直接 diff 取回。

12. **配置错误大声报错，绝不静默兜底**：读不到/不合法就报错并在页面显示红条；素材缺失时窗口照建
    （否则用户连错误都看不到）。

13. **运行期改配置 = 拆窗重建，不做"就地改参数"**（`reload.rs::apply_config`）
    配置能表达"多一只/少一只"，就地改无法覆盖结构变化；统一走一条路径就不会出现"窗口还是旧的、
    账本已经是新的"这种半新半旧状态（代价是一次几百毫秒的重建）。

14. **拆窗那一刻必须拦住退出**（`lib.rs` 的 `run` 闭包）
    拆完旧窗、新窗未建的一瞬间窗口数为 0，Tauri 会当成"最后一个窗口被关闭"而退出整个应用。
    只在"正在应用新配置"时 `prevent_exit()`；而托盘的"退出"要先立旗——因为 `AppHandle::exit()`
    **也**会走 `ExitRequested`，否则会变成"点退出退不掉"（真踩过）。

15. **设置窗口保存会重写整个配置文件**（`config.rs::save_config`）
    JSON 不存注释，而模板是带注释的；因此采用"备份上一版 + 文件开头写说明段"，并让"手工编辑 → 热重载"
    这条路继续可用。取舍理由见 `VERIFICATION.md` 9.4。

## 九、当前状态与已知限制

**版本 v1.0.0**；里程碑：M0 技术验证 ✅ / M1 宠物本体 ✅ / M2 像个正经应用 ✅ / M2.5 外观与结构 ✅ /
M3 加分能力（LLM）✅ 基本完成 / M4 发布：NSIS + MSI 已产出并可装（本机 2026-09-21 出 1.0.0 包），
代码签名与自动更新未做。
逐项状态与待办见 [ROADMAP.md](ROADMAP.md)；每条"已完成"的证据都在 [VERIFICATION.md](VERIFICATION.md)
（纯逻辑冒烟 **114 项断言全通过**）。

> 这个项目的规矩是：**"完成了"必须有可复现的证据**——要么是日志/探针输出，要么是截图，
> 要么是逐像素比对。做不到的项一律写进下面的表，而不是含糊过去。

| 限制 / 未验证 | 说明 | 计划 |
| --- | --- | --- |
| 右键菜单的生产版观感 | 根因已定位并修复（`style-src` 的 nonce 让运行期注入的菜单样式被拦，见 `VERIFICATION.md` 第 18 节），**但修法尚未在生产构建上回归** | 装一次新出的包确认白底面板 |
| `cargo test` 跑不起来 | GNU 工具链下 libtest 可执行文件被加载器拒绝（`0xC0000139`，导入表与主程序一致，属环境问题）。改用 `cargo run --example logic-smoke` | 换 MSVC 工具链或在正常环境跑 |
| 仅 Windows | 透明窗/穿透/DPI 都按 Windows 验证 | macOS 需 `.mov` 素材 + 签名公证；Linux 合成器差异大 |
| 多显示器跨屏抛掷未做观感回归 | 本机只有一块屏（几何/校验/放行逻辑已就位） | 需双屏机器回归一次 |
| 高 DPI 未做观感回归 | 本机 DPR=1（100%） | 需 125%/150% 的机器各跑一次 |
| 深色主题未做真机观感回归 | 结构是同一份，靠 `prefers-color-scheme` 切换；本机桌面为浅色 | 需深色桌面机器看一眼 |
| 托盘图标的真实点击未被自动化 | 本机鼠标注入不可靠，验证覆盖到"菜单项 → 动作"这一层为止 | 见 `VERIFICATION.md` 9.9 |
| 无跨窗碰撞 | 多宠物可开，但宠物之间不碰撞（`petCollision` 目前是占位开关） | 见 ROADMAP |
| 无点击积分 | 飞行中被按下的粒子与积分卡未做 | 见 ROADMAP |
| AI：多把 Key 不能共存 | 密钥库目前单槽位，"LLM 用 DeepSeek + 余额查 OpenCode"需要改成 `{provider: key}` 映射 | 下一轮 |
| 安装包未签名 | Windows 会提示「未知发布者」 | 要 OV 代码签名证书（约 ¥1000–3000/年）；路子与接法见 [`SIGNING.md`](SIGNING.md) |
| 从 dsh-pet 一键导入未做 | 素材导入已有 `scripts/import-animations.ps1`，设置窗口里的一键入口未做 | 见 ROADMAP |
