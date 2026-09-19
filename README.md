# whale-pet desktop

把 [whale-pet](https://github.com/PC2005-cloud/whale-pet) 的桌宠做成一个**独立的 Windows 桌面应用**
（Tauri v2）。不依赖 DSH，装完即用：透明置顶小窗、手绘透明动画、拖拽甩抛、点击 Q 弹、点击穿透。

> 本仓库**只读取**上游 `D:\programs\github\whale-pet`，从不修改它。
> 拷贝了哪些文件、为什么拷、怎么同步，见 [`docs/COPY-MANIFEST.md`](docs/COPY-MANIFEST.md)。

---

## 一、当前进度（M0 技术验证：已完成）

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
| 纯逻辑冒烟（38 项断言） | ✅ | `cargo run --bin logic-smoke` → 通过 38、失败 0 |

**尚未实现**（按里程碑排期，见 [`docs/ROADMAP.md`](docs/ROADMAP.md)）：
完整动画链与随机动作、左右转向、屏幕漫游、多宠物与跨窗碰撞、右键级联菜单、点击积分、
设置窗口与托盘完整菜单、碎碎念/对话（LLM）、余额、打包安装包。

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

### 2.3 构建可执行文件

```powershell
pnpm tauri build --no-bundle     # 只产 exe：src-tauri\target\release\whale-pet-desktop.exe
```

> ⚠️ 本机沙箱**无法**产出 `.msi` / NSIS 安装包：Tauri 打包时需要从 GitHub 下载
> NSIS/WiX，而本机所有依赖系统 Schannel 的 HTTPS 请求都会失败。
> `tauri.conf.json` 里已经把 `bundle.targets` 配好（`nsis` + `msi`），
> 在能联网的机器上去掉 `--no-bundle` 即可直接出安装包。

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
cargo run --bin logic-smoke      # 33 项断言：配置解析/校验、路径防穿越、JSONC 注释、几何换算
```

---

## 三、配置

配置目录：`%APPDATA%\com.whalepet.desktop\`

| 文件 | 说明 |
| --- | --- |
| `config.jsonc` | 主配置（支持 `//` 与 `/* */` 注释）。首次运行自动生成，**绝不覆盖你的改动** |
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

- `pets` 数组**每一项 = 一个独立的透明置顶窗口**（多开就多写几项，`id` 必须唯一）
- `idle` / `click` 留空字符串 = 自动挑选目录里排序最靠前的动画
- 任何字段缺失或非法都会**明确报错**（控制台打印文件完整路径），不做静默兜底
- 改完配置**重启应用**生效（M0 尚未做热重载）

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

两个钩子都只用合成采样驱动**与真实输入同一条代码路径**，不改变业务语义；
`autotest=2` 的场景与结果见 [`docs/VERIFICATION.md`](docs/VERIFICATION.md) 第 6.4 节。

---

## 四、目录结构

```
.
├─ index.html                    桌宠窗口页面（透明布局 + 双缓冲 video + 命中区）
├─ src/
│  ├─ main.ts                    入口：取配置 → 建通道 → 装配运行时
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
│  │  ├─ lib.rs                  启动装配：协议 → 配置 → 几何 → 窗口 → 托盘 → 轮询线程
│  │  ├─ main.rs                 二进制入口
│  │  ├─ pet_window.rs           窗口创建/定位/移动/穿透翻转（每宠一个局部小窗）
│  │  ├─ pet_protocol.rs         自定义协议 pet://（提供动画/字体等本地素材）
│  │  ├─ display.rs              显示器几何 + 变化轮询（并集，不是外接矩形）
│  │  ├─ config.rs               配置读取/校验/JSONC 注释剥离/首次运行落默认
│  │  ├─ model.rs                数据契约（与 contract.ts 一一对应）
│  │  ├─ commands.rs             前端可调用的命令
│  │  ├─ state.rs                共享状态
│  │  ├─ diagnostics.rs          诊断日志落盘
│  │  └─ bin/logic-smoke.rs      纯逻辑冒烟检查（33 项断言）
│  └─ capabilities/default.json  最小权限集（只有事件通道）
├─ reference/shared/             ← 从上游逐字节拷贝的纯逻辑（**零改动**）
├─ config/default-config.jsonc   默认配置模板（编译进 exe，首次运行释放）
├─ assets/webm/                  内置示例动画（编译进 exe）
├─ scripts/                      环境激活、素材导入
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

---

## 六、已知限制

| 限制 | 说明 | 计划 |
| --- | --- | --- |
| 本机无法产出安装包 | 沙箱无 HTTPS，下载不了 NSIS/WiX；用 `--no-bundle` 出 exe | 在能联网的机器上直接出包 |
| `cargo test` 跑不起来 | GNU 工具链下 libtest 可执行文件被加载器拒绝（`0xC0000139`，导入表与主程序一致，属环境问题）。改用 `cargo run --bin logic-smoke` | 换 MSVC 工具链或在正常环境跑 |
| 仅 Windows | 透明窗/穿透/DPI 都按 Windows 验证 | macOS 需 `.mov` 素材 + 签名公证；Linux 合成器差异大 |
| 多显示器跨屏抛掷未做观感回归 | 本机只有一块屏（几何/校验/放行逻辑已就位） | 需双屏机器回归一次 |
| 单只宠物 | 多宠物、跨窗碰撞属 M1 | 见 ROADMAP |
| 无右键菜单 | M1 | 上游 `shared/menu.ts` 可复用 |
| 无 LLM 能力 | 碎碎念/对话属 M3 | 需要自带 provider（OpenAI 兼容/DeepSeek/Ollama） |
| CSP 未收紧 | `tauri.conf.json` 里为 `null` | 发布前按 `docs/TAURI-CONFIG.md` 收紧并回归 |

---

## 七、许可

- 代码：MIT（与上游一致）
- 素材（动画/提示词/源视频）：**允许开源使用，禁止商用**（上游约定，本应用沿用）
