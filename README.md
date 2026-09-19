# whale-pet desktop

把 [whale-pet](https://github.com/PC2005-cloud/whale-pet) 的桌宠做成一个**独立的 Windows 桌面应用**
（Tauri v2）。不依赖 DSH，装完即用：透明置顶小窗、手绘透明动画、拖拽甩抛、点击 Q 弹、点击穿透。

> 本仓库**只读取**上游 `D:\programs\github\whale-pet`，从不修改它。
> 拷贝了哪些文件、为什么拷、怎么同步，见 [`docs/COPY-MANIFEST.md`](docs/COPY-MANIFEST.md)。

---

## 一、当前进度（M0 / M1 已完成，M2 已完成）

M0 的目标是验证"Tauri 能不能把这套桌宠跑起来"这条链路上所有**有风险的环节**。
下表全部是在本机**实跑验证**的结果（验证方式见 [`docs/VERIFICATION.md`](docs/VERIFICATION.md)）：

| 验证项 | 结果 | 证据 |
| --- | --- | --- |
| 透明置顶小窗创建成功 | ✅ | 窗口 `840×656`，扩展样式含 `WS_EX_LAYERED(0x80000)` + `WS_EX_TOOLWINDOW(0x80)` |
| VP9-alpha webm 透明播放 | ✅ | 自定义协议取到素材，解码出 `640x360`，`readyState=4` |
| 窗口内布局与坐标换算 | ✅ | 包围盒 `(2116,100)` = 配置的右上角 + 边距 24/100（窗口原点 `1906,-110`，越界部分只是透明余量） |
| **DPI 严格 1:1** | ✅ | `devicePixelRatio=1`，窗口 CSS `840x656` = 物理 `840x656`，包围盒 CSS `420.0x236.0` = 配置 |
| 全局光标采样 + 命中判定 | ✅ | 光标进入身体 → 记 `命中: 首次进入身体` → 窗口翻 `可交互` |
| 点击穿透自愈（进/出翻转） | ✅ | 光标移开 → `交互: 窗口切换为 点击穿透` |
| **busy 保护** | ✅ | 拖拽全程无一次翻回穿透；拖拽期间看门狗也不误伤 |
| **松手事件丢失的自愈** | ✅ | 专项自测 `?autotest=2`：拖到命中区外再松手 → 收尾成功，区外漂移 `0.0px` |
| **跟手坐标的正确来源** | ✅ | 注入式拖拽：抓取点 = DOM 推出的屏幕坐标 `(2320,226)`，与命中判定同坐标系（见下） |
| 点击回应 | ✅ | 单击 → 播 `点击回应-元气挥手.webm`（`mode=once`） |
| 拖拽弹簧跟手 | ✅ | 真实输入粒度下滞后 `(121,-68)` px（过阻尼弹簧应有的"跟手但带重量"） |
| 甩抛物理 | ✅ | 初速 `(-3269,1509)px/s` → 飞行 11.5 秒 → 落地冲击 `2173px/s` |
| 落地静止 | ✅ | 落定 `y=1156` = 工作区底 `1392` − 宠物高 `236`，精确踩在工作区底边上 |
| **逐帧窗口跟随** | ✅ | 前端计数 **63.3 次/秒**（≈刷新率）；进程外 160ms 采样到 **77 个不同位置**、尺寸恒为 `840×656` |
| **多显示器几何（工作区/面板）** | ✅ | 工作区 `2560×1392` vs 面板 `2560×1440`（含 48px 任务栏），按序成对下发并启动期校验 |
| 配置读取 / 校验 / 默认配置释放 | ✅ | 首次运行自动释放 `config.jsonc` + 2 条示例动画 |
| 纯逻辑冒烟（51 项断言） | ✅ | `cargo run --bin logic-smoke` → 通过 51、失败 0（含 M2 的配置写回路径） |

### M1「宠物本体补齐」已完成

完整动画链与随机动作分类、素材导入（106 条）、左右转向与镜像、屏幕漫游、右键级联菜单、
气泡独立小窗、角落定位语义统一，以及三轮真机 bug 修复（持锁跨线程动窗口导致死锁、
输入状态机两处真 bug、"跟手太松"的弹簧滞后从 0.15s 压到 0.082s）。
**用户真机验收："可以，当前版本很丝滑。"** 复盘见 `VERIFICATION.md` 6.6 ~ 6.8。

### M2「像个正经应用」已完成

| 能力 | 说明 |
| --- | --- |
| 托盘完整菜单 | 显示/隐藏、回到初始位置、动作点播（按分类级联）、开机自启、设置、退出 |
| 设置窗口 | 独立普通窗口：宠物增删、名称/ID/尺寸/角落/边距、物理参数、动画池与权重；**保存即生效** |
| 单实例锁 | 重复启动不再开第二份，而是显示已有实例 |
| 开机自启 | 托盘勾选项与设置窗口共用；状态直接读系统 |
| 配置热重载 | 外部改 `config.jsonc` 约 1 秒内生效（改坏了保留当前配置并在日志里说明原因） |
| CSP 收紧 | `csp` + `devCsp` 落地，并在 **release 二进制**上回归过透明动画与 IPC |

> 注意：从设置窗口保存会**整体重写**配置文件（JSON 不存注释），文件开头会写明这一点，
> 上一版会备份成 `config.jsonc.bak`。想保留手写注释就别从界面保存。

### M2.5「外观与结构升级」已完成

| 能力 | 说明 |
| --- | --- |
| 应用与托盘图标 | 图形源是**矢量 SVG**（`src-tauri/icons/design/app-icon.svg`）：`cargo run --example make-icon` 用 resvg 生成 `.ico` 多尺寸 + png + 托盘用的 RGBA；三款候选见 `docs/screenshots/icon-candidates.png` |
| 自绘托盘菜单 | 托盘右键弹出浅/深色圆角菜单（跟随系统）：显示/隐藏（一个切换项）、回到初始位置、动作点播、设置、退出；**动作点播的分类默认收起，点击才展开**（另有"全部展开/收起"） |
| 界面图标 | 菜单/导航/按钮的线性图标取自 [Lucide](https://lucide.dev)（ISC 许可），形状内联、无依赖 |
| 设置界面 | 左侧竖向导航 + 每只宠物独立页面 + 通用参数页；跟随系统深浅色 |
| 每只宠物独立行为 | 每只宠物可**单独覆盖**动画池与权重（不设则跟随全局默认），右键菜单与托盘点播都按它自己的池 |

> 托盘菜单里的"显示/隐藏"是**一个**切换项，文案随状态变；开机自启移到了设置窗口的「启动与系统」。

**尚未实现**（见 [`docs/ROADMAP.md`](docs/ROADMAP.md)）：
多宠物跨窗碰撞、点击积分、高 DPI 与多显示器观感回归、从 dsh-pet 一键导入、
LLM 能力（碎碎念/对话/表情包/余额）、安装包产出。

---

## 二、快速开始

### 2.1 环境

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

### 2.2 安装依赖并运行

```powershell
cd D:\programs\deepseek\sakura
pnpm install
pnpm tauri dev          # 开发模式（热更新；自动拉起 Vite）
```

宠物默认出现在**屏幕右上角**。首次运行会自动在
`%APPDATA%\com.whalepet.desktop\` 下生成配置与两条示例动画。

### 2.3 构建可执行文件与安装包

```powershell
pnpm tauri build --no-bundle     # 只产 exe：src-tauri\target\release\whale-pet-desktop.exe
pnpm tauri build                 # 连安装包一起出：bundle\nsis\*.exe 与 bundle\msi\*.msi
```

> 本机（这台开发机）没有把 Tauri CLI 装进 `node_modules`（依赖刻意只留 vite + typescript），
> CLI 走 `D:\tools\Tauri\nodejs\tauri.cmd`；直接用它也是一样的：
>
> ```powershell
> & 'D:\tools\Tauri\nodejs\tauri.cmd' build
> ```
>
> **安装包已于 v0.1.0 首次产出**（NSIS 2.91 MB / MSI 4.13 MB，本机实测），
> 下载见 [Releases](https://github.com/clover-us/sakura/releases)。
> 安装包**未做代码签名**，Windows 可能提示「未知发布者」。
> NSIS 的安装向导图标由 `bundle.windows.nsis.installerIcon` 指定（否则会是 NSIS 默认图标）。

### 2.4 导入完整动画素材（106 条）

仓库里只内置了 2 条示例动画（编译进 exe，保证"装完就有东西看"）。
要导入上游完整素材集：

```powershell
pwsh -File scripts\import-animations.ps1
```

脚本把上游 `dsh-pet/assets/webm` 下的 106 条透明动画拷进
`%APPDATA%\com.whalepet.desktop\webm\`（已存在且大小一致的会跳过），**从不修改上游仓库**。

### 2.5 运行纯逻辑冒烟检查

```powershell
cd src-tauri
cargo run --bin logic-smoke      # 60 项断言：配置解析/校验/写回、每宠独立动画池、路径防穿越、JSONC 注释、几何换算
cargo run --example make-icon    # 重新生成 icons/（图形源是 SVG，见 src-tauri/icons/design/）
```

---

## 三、配置

配置目录：`%APPDATA%\com.whalepet.desktop\`

| 文件 | 说明 |
| --- | --- |
| `config.jsonc` | 主配置（支持 `//` 与 `/* */` 注释）。首次运行自动生成，**绝不覆盖你的改动** |
| `config.jsonc.bak` | 设置窗口每次保存前自动备份的上一版（只留最近一次） |
| `webm/` | 动画素材目录。放入 `.webm`（VP9-alpha）即可在配置里引用 |
| `pet-debug.log` | 诊断日志（见下） |

配置结构（完整模板与逐字段注释见 [`config/default-config.jsonc`](config/default-config.jsonc)）：

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

- `pets` 数组**每一项 = 一个独立的透明置顶窗口**（多开就多写几项，`id` 必须唯一；也可以在设置窗口里增删）
- `idle` / `click` 留空字符串 = 自动挑选目录里排序最靠前的动画
- 任何字段缺失或非法都会**明确报错**（控制台打印文件完整路径），不做静默兜底
- **改完配置不用重启**：应用每 1 秒比对文件指纹，变化就热重载（拆掉旧窗、按新配置重建）；
  改坏了（解析/校验失败）会保留当前配置继续跑，把原因写进日志。改动画池之后托盘的"动作点播"也会跟着重建
- 从**设置窗口**保存会整体重写该文件（JSON 不存注释），详见上文 M2 的提示

### 诊断日志

透明无边框窗口出问题时的表现往往只是"看不见宠物"或"点不动"——没有界面可看，
发布版也没有控制台。因此前端会把关键节点写进 `%APPDATA%\com.whalepet.desktop\pet-debug.log`：

```
[d20714 18:41:09.030][pet-main-0] 动画: 首帧就绪 readyState=4 尺寸=640x360 src=...
[d20714 18:41:09.034][pet-main-0] 命中: 首次进入身体 光标=(2140,336) 包围盒=(1930,210)
[d20714 18:41:09.541][pet-main-0] 松手: 甩出 速度=(-3214,1607)px/s 位置=(1415,514)
[d20714 18:41:20.250][pet-main-0] 飞行: 结束于 (2086,1156)，落地冲击 2083px/s
```

这条通道同时是**自动化验证的手段**，有两个受控自测场景（真实鼠标注入在自动化环境里不可靠，
桌面随时可能被别的窗口遮挡）：

```powershell
$env:WHALE_PET_AUTOTEST = '1'; pnpm tauri dev   # 拖拽 + 甩抛全链路（弹簧滞后/初速/落地）
$env:WHALE_PET_AUTOTEST = '2'; pnpm tauri dev   # "拖到命中区外松手"的失控场景（松手事件丢失的自愈）
```

M2 又加了两组**走真实代码路径**的探针（自动化验证用，默认不启用）：

```powershell
# 设置窗口：打开同一个 settings_window::open()，再由页面自己点按钮（页面 → 命令 → 写盘 → 重建）
$env:WHALE_PET_DIAG_SETTINGS = 'save'          # 1 | save | autostart | addpet | delpet | ownbehaviour | nav:physics
# 托盘菜单：弹出真实菜单窗（截图用），或按序走一遍动作层
$env:WHALE_PET_DIAG_TRAY_MENU = '5000:picker'  # 5 秒后弹出并展开"动作点播"
$env:WHALE_PET_DIAG_TRAY = 'toggle,home,anim:待机呼吸休闲,settings'
pnpm tauri dev
```

界面类改动的检查方式是**截图**（WebView2 的内容 `PrintWindow` 抓不到，脚本走屏幕合成截图）：

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\capture-window.ps1 -TitleLike '设置'
```

两个钩子都只用合成采样驱动**与真实输入同一条代码路径**，不改变业务语义；
`autotest=2` 的场景与结果见 [`docs/VERIFICATION.md`](docs/VERIFICATION.md) 第 6.4 节，
M2 / M2.5 探针的证据见第 9 / 10 节。

---

## 四、目录结构

```
.
├─ index.html                    桌宠窗口页面（透明布局 + 双缓冲 video + 命中区）
├─ bubble.html / menu.html       气泡窗 / 右键菜单窗页面（都是独立小窗）
├─ settings.html                 设置窗口页面（左侧导航 + 跟随系统深浅色）
├─ tray-menu.html                托盘菜单页面（自绘菜单，浅/深色跟随系统）
├─ src/
│  ├─ main.ts                    入口：取配置 → 建通道 → 装配运行时
│  ├─ settings-page.ts           设置页逻辑：整份配置进出，改哪几个字段就动哪几个
│  ├─ tray-menu-page.ts          托盘菜单逻辑：主菜单 / 动作点播，动作交给宿主执行
│  ├─ menu-page.ts               右键菜单页逻辑：上游菜单树 + 桌面端本地工具项
│  ├─ bridge/
│  │  ├─ tauri.ts                window.__TAURI__ 的唯一访问点（含类型声明）
│  │  ├─ contract.ts             Rust ↔ 前端 数据契约（与 model.rs 一一对应）
│  │  └─ log.ts                  诊断日志（写进 pet-debug.log）
│  └─ renderer/
│     ├─ runtime.ts              核心装配：位置真相、命中判定、窗口跟随
│     ├─ drag.ts                 拖拽：弹簧跟手 + 松手初速（复用 shared 纯逻辑）
│     ├─ anim.ts                 Q 弹挤压 + 甩抛飞行
│     ├─ media-buffer.ts         双缓冲透明视频播放（零空白帧切换）
│     ├─ hitbox.ts               身体命中区几何（640×360 画布 → 实际尺寸）
│     ├─ coords.ts               三套坐标系的换算收口（屏幕/窗口/包围盒）
│     ├─ cursor.ts               全局光标采样通道
│     └─ dom.ts                  最小 DOM 工具 + 大声报错
├─ src-tauri/
│  ├─ src/
│  │  ├─ lib.rs                  启动装配：插件 → 协议 → 配置 → 几何 → 窗口 → 托盘 → 轮询线程
│  │  ├─ main.rs                 二进制入口（release 隐藏控制台）
│  │  ├─ pet_window.rs           窗口创建/定位/移动/穿透翻转（每宠一个局部小窗）
│  │  ├─ tray.rs                 托盘图标与动作分发（左键切换显隐，右键弹菜单）
│  │  ├─ tray_menu.rs            自绘托盘菜单窗：摆位 / 外点关闭 / 按内容变高
│  │  ├─ settings_window.rs      设置窗口的建/显/关（普通窗口）
│  │  ├─ reload.rs               配置热重载与"保存即生效"（拆窗重建）
│  │  ├─ pet_protocol.rs         自定义协议 pet://（提供动画/字体等本地素材）
│  │  ├─ display.rs              显示器几何 + 变化轮询（并集，不是外接矩形）
│  │  ├─ config.rs               配置读取/校验/JSONC 注释剥离/写回/每宠池解析
│  │  ├─ model.rs                数据契约（与 contract.ts 一一对应）
│  │  ├─ commands.rs             前端可调用的命令
│  │  ├─ state.rs                共享状态（配置可在运行期替换）
│  │  ├─ watchdog.rs             取锁/窗口操作计时 + 主线程健康看门狗
│  │  ├─ diagnostics.rs          诊断日志落盘 + 受控自测/探针入口
│  │  └─ bin/logic-smoke.rs      纯逻辑冒烟检查（60 项断言）
│  ├─ examples/make-icon.rs      图标生成工具：SVG → png/ico/托盘 RGBA（resvg 只在 dev-dependencies）
│  ├─ icons/                     design/*.svg 图形源 + 生成的 ico/png/托盘 RGBA
│  └─ capabilities/default.json  最小权限集（宠物/气泡/菜单/设置四类窗口）
├─ reference/shared/             ← 从上游逐字节拷贝的纯逻辑（**零改动**）
├─ config/default-config.jsonc   默认配置模板（编译进 exe，首次运行释放）
├─ assets/webm/                  内置示例动画（编译进 exe）
├─ scripts/                      环境激活、素材导入、窗口探针、**窗口截图**
└─ docs/                         拷贝清单、验证记录、路线图、Tauri 配置说明
```

---

## 五、设计要点（为什么这么做）

这套实现里有几个**不能想当然**的地方，都写在对应源码的注释里，这里给索引：

1. **点击穿透必须换方案**（`pet_window.rs::apply_fallback_hit` + `cursor.ts`）
   Electron 的 `setIgnoreMouseEvents(true, { forward: true })` 在穿透时仍转发鼠标移动，
   页面能自己发现"光标进来了"；**Tauri 没有 forward**，穿透后页面收不到任何事件。
   所以改为：Rust 每 16ms 轮询全局光标 → 事件下发 → 前端做精确命中判定 → 翻转穿透。
   Rust 侧只做"恢复可交互"这一个方向，绝不主动翻回穿透。

2. **跟手坐标用 DOM 事件，不用宿主的光标采样**（`runtime.ts::bindDomInput`）
   曾经的理由是"宿主的采样坐标不可信"——**那条结论后来被复测推翻了**：
   用 `WHALE_PET_DIAG_CURSOR=1` 把宿主读到的值与页面收到的值逐帧对照，
   两条链路逐像素一致，与进程外 `GetCursorPos` 也一致。
   当年之所以看着不一致，是因为拿 `SetCursorPos` 设定的位置当"真实光标"，
   而本机的指针被桌面环境持续驱动，`SetCursorPos` 设过去几百毫秒内就被挪走了。

   现在跟手仍用 `windowOrigin + event.clientX/Y`，理由换成更硬的一条：
   DOM 坐标是**按下那一刻的真实位置**且**零延迟**，而采样通道要走
   "宿主轮询(16ms) → IPC → 页面"三段，跟手会明显发飘。
   （更正过程见 [`docs/VERIFICATION.md`](docs/VERIFICATION.md) 第 6.4 节。）

3. **看门狗不能只看按键位，且拖拽必须做指针捕获**（`runtime.ts::onCursorFrame` / `beginPress`）
   本机的 `GetAsyncKeyState(VK_LBUTTON)` 也会**误报"已松开"**：把它单独当收尾依据，
   真实按下会在 **2ms 内**被收尾，表现为"刚按住就脱手、宠物飞一下再回来"。
   现在的判据是**三重条件**：按键位报松开 + 该状态**早于本次按下** + DOM 指针事件已静默 ≥250ms。
   同时用 `setPointerCapture` 把拖拽期间的指针事件锁在宠物窗口，
   光标滑出窗外也照常收到 `pointermove`——这条才是"拖拽中丢 `pointerup`"的正道解法。
   采样通道因此只剩两个职责：看门狗收尾、以及穿透期间的兜底位置。
   完整的定位过程（含两轮错误判断）见 [`docs/VERIFICATION.md`](docs/VERIFICATION.md) 第 6.4 节。

4. **宠物窗口必须 `focusable(false)`**（`pet_window.rs::create_one`）
   `focusable(true)` 时鼠标在宠物身上按下会**激活窗口并抢走前台焦点**；拖拽结束、窗口恢复
   点击穿透后焦点仍留在宠物上，于是**点别的窗口第一次点击只被用来切换前台**——
   表现为"点击其他页面选中不了"（宠物被拖到屏幕中央、透明窗覆盖大片区域时尤其明显）。
   改成不可聚焦后，窗口照常收鼠标消息（命中区交互不受影响）但永不激活。
   启动与每次穿透翻转都会打印窗口扩展样式（`transparent` / `noactivate`），
   这是判断"窗口此刻是否在吃点击"的唯一真相。

5. **窗口尺寸必须等于宠物包围盒**（`WINDOW_MARGIN_RATIO = 0`）
   窗口只要处于"可交互"态，**整块窗口矩形**都在吃鼠标。早期按 0.5 比例外扩，
   窗口 840×656 里只有 420×236 是宠物，用户的实际感受就是
   "形象周围很大一片空白区域都点不到下层页面"。现在窗口与宠物严格重合。
   代价：气泡（头顶）与右键菜单都需要宠物**周围**的空间——两者都已按"**各自单独开一个窗**"落地：
   `bubble.rs`（不可聚焦 + 整窗点击穿透）与 `menu_window.rs`（不可聚焦 + 可交互，见
   `docs/VERIFICATION.md` 的 ⑤/⑥）。**宠物窗因此从头到尾不改几何**，
   连带消掉了"外扩坐标换算、重复右键偏移、换几何时人物虚影"那一整类问题。
   该常量是 Rust 与前端之间的契约，两边必须同时改。

6. **每只宠物一个小窗，不用全屏透明窗**
   Windows DWM 在"全屏透明 + 视频层"下会黑屏（上游实测），小窗不黑。

7. **busy 期间绝不翻回穿透**（`runtime.ts::reportBusy`）
   拖拽时宠物由弹簧追赶光标、**滞后**于光标，若按几何盲判会在光标滑出包围盒时误判成
   "用户离开了"，正在进行的拖拽当场断掉（pointerup 收不到 → 宠物按旧速度飞走）。
   这条铁律继承自上游的 `setInputBusy`。

8. **位置的唯一真相在 Rust**（`contract.ts::PetRuntime`）
   前端只上报"我希望窗口去哪"，Rust 移动后回传真实位置，前端再据此换算。
   两边各自记账必然累积误差。

8. **三套坐标系只在 `coords.ts` 换算**
   屏幕坐标系（物理/光标/窗口位置）、窗口内容区坐标系（`set_pet_bounds`）、
   包围盒坐标系（命中判定/Q 弹锚点）。混用一次就是半个身位的错位。

9. **抛掷边界用逐屏工作区并集，不用外接矩形**
   显示器摆放不规则时外接矩形含大片空洞，宠物会飞进看不见的地方（上游实测一例占 23.7%）。

10. **`reference/shared` 保持零改动**
    需要适配就在 `src/` 里包一层（例如 `drag.ts` 把 shared 的 `(vx, vy)` 命名
   转换成本项目的契约 `(x, y)`），而不是去改共享层——这样上游修 bug 时能直接 diff 取回。

11. **配置错误大声报错，绝不静默兜底**
    读不到/不合法就报错并在页面显示红条；素材缺失时窗口照建（否则用户连错误都看不到）。

12. **运行期改配置 = 拆窗重建，不做"就地改参数"**（`reload.rs::apply_config`）
    配置能表达"多一只/少一只"，就地改参数无法覆盖这种结构变化；统一走一条路径，
    就不会出现"窗口还是旧的、账本已经是新的"这种半新半旧状态（代价是一次几百毫秒的重建）。

13. **拆窗那一刻必须拦住退出**（`lib.rs` 的 `run` 闭包）
    拆完旧窗、新窗未建的一瞬间窗口数为 0，Tauri 会当成"最后一个窗口被关闭"而退出整个应用。
    只在"正在应用新配置"时 `prevent_exit()`；而托盘的"退出"要先立旗——
    因为 `AppHandle::exit()` **也**会走 `ExitRequested`，否则会变成"点退出退不掉"（真踩过）。

14. **设置窗口保存会重写整个配置文件**（`config.rs::save_config`）
    JSON 不存注释，而模板是带注释的；因此采用"备份上一版 + 文件开头写说明段"，
    并让"手工编辑 → 热重载"这条路继续可用。取舍理由见 `VERIFICATION.md` 9.4。

---

## 六、已知限制

| 限制 | 说明 | 计划 |
| --- | --- | --- |
| ~~本机无法产出安装包~~ | **已解决**：网络恢复后 `tauri build` 成功产出 NSIS + MSI（v0.1.0，见 VERIFICATION 第 14 节） | — |
| `cargo test` 跑不起来 | GNU 工具链下 libtest 可执行文件被加载器拒绝（`0xC0000139`，导入表与主程序一致，属环境问题）。改用 `cargo run --bin logic-smoke` | 换 MSVC 工具链或在正常环境跑 |
| 仅 Windows | 透明窗/穿透/DPI 都按 Windows 验证 | macOS 需 `.mov` 素材 + 签名公证；Linux 合成器差异大 |
| 多显示器跨屏抛掷未做观感回归 | 本机只有一块屏（几何/校验/放行逻辑已就位） | 需双屏机器回归一次 |
| 高 DPI 未做观感回归 | 本机 DPR=1（100%） | 需 125%/150% 的机器各跑一次 |
| 托盘图标的真实点击未被自动化 | 本机鼠标注入不可靠，验证覆盖到"菜单项 → 动作"这一层为止 | 见 `VERIFICATION.md` 9.9 |
| 无跨窗碰撞 | 多宠物可开（设置窗口可增删），但宠物之间不碰撞（`petCollision` 目前是占位开关） | M1 遗留项，见 ROADMAP |
| 无点击积分 | 飞行中被按下的粒子与积分卡未做 | M1 遗留项 |
| AI 能力：适配层/密钥/碎碎念/自检**已完成**，对话输入窗未做 | 默认完全离线；开启后可用 DeepSeek / OpenAI / Ollama / 自定义；设计见 [`docs/LLM.md`](docs/LLM.md)，证据见 `VERIFICATION.md` 第 13 节 | 下一步：宠物角上的对话输入窗 |
| 从 dsh-pet 一键导入未做 | 素材导入已有 `scripts/import-animations.ps1`，设置窗口里的一键入口未做 | M2 遗留项 |

---

## 七、许可

- 代码：MIT（与上游一致）
- 素材（动画/提示词/源视频）：**允许开源使用，禁止商用**（上游约定，本应用沿用）
- 界面图标形状：[Lucide](https://lucide.dev)（ISC 许可），已内联进 src/tray-menu-page.ts 与 src/settings-page.ts；应用图标为本项目自绘（src-tauri/icons/design/）
