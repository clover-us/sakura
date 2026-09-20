# 拷贝清单与同步约定（与 `D:\programs\github\whale-pet` 的关系）

> **本仓库从不修改上游仓库。** 所有来自上游的内容都是**只读拷贝**，
> 上游任何时刻保持原样（已用 `git status --porcelain` 确认工作区干净）。

## 一、来源与基线

| 项 | 值 |
| --- | --- |
| 上游仓库 | `D:\programs\github\whale-pet`（GitHub: `PC2005-cloud/whale-pet`） |
| 上游子包 | `dsh-pet/`（DSH 插件，npm 包名 `dsh-pet`，版本 0.2.11） |
| **拷贝基线 commit** | `d3988fa52fccaae8249d94ba31e9e4bae073362d` |
| 基线提交说明 | `fix(helper): 宿主退出/管道断开后自行退出，不再弹框卡死（issue #56）`（2026-09-18） |

日后要从上游取更新时，用上面的 commit 做 diff（见第四节）。

## 二、已拷贝的文件

### 2.1 `reference/shared/` —— 纯逻辑层（**逐字节原样拷贝，零改动**）

| 文件 | 上游路径 | 为什么需要 |
| --- | --- | --- |
| `physics.ts` | `dsh-pet/src/shared/physics.ts` | 拖拽弹簧、松手初速估算、逐屏抛掷积分、Q 弹曲线。**桌宠手感的核心**，必须与上游同源才能保证手感一致 |
| `displays.ts` | `dsh-pet/src/shared/displays.ts` | 多显示器"工作区并集"几何（外接矩形含空洞，不能当边界） |
| `constants.ts` | `dsh-pet/src/shared/constants.ts` | 画布几何常量（`HIT_BOX` / `FEET_Y` / `DRAG_THRESHOLD` / `PET_REF_WIDTH`） |
| `types.ts` | `dsh-pet/src/shared/types.ts` | 上游的类型模型（`PhysicsParams` / `Rect` / `Corner` / `Animations` / `Category` 等） |
| `pickers.ts` | `dsh-pet/src/shared/pickers.ts` | 动画链的选择规则（`rollKind` 掷骰 / `pick` / `pickCategoryAction` / `pickWeightedCategory`）。**动画链的"随机感与权重"全靠它**，重写一份必然与上游手感分叉 |
| `motion.ts` | `dsh-pet/src/shared/motion.ts` | 移动几何（`planMove` 落点可达性 + `anchorPixel` 角落定位）。漫游与"回到初始位置"共用 |
| `menu.ts` | `dsh-pet/src/shared/menu.ts` | 右键级联菜单（`buildMenuTree` 树 + `MENU_CSS` 样式 + `mountContextMenu` 级联渲染）。**菜单外观与级联行为因此与上游严格一致**；桌面端只在外层补"窗口外扩"与动作落地 |

依赖闭环检查（已确认无遗漏）：
`physics.ts` → `types.ts` + `constants.ts` + `displays.ts`；
`motion.ts` → `pickers.ts` + `displays.ts` + `types.ts`；
`menu.ts` → `types.ts`（含两处 DOM 渲染，上游已在文件头注明是"共享层的唯一例外"）。
**没有**引入 `chat.ts` / `notify.ts` / `work-status.ts` / `score*.ts` 等依赖上游页面环境或会话事件的模块。

### 2.2 `assets/webm/` —— 示例动画（编译进二进制的种子素材）

| 文件 | 大小 | 作用 |
| --- | --- | --- |
| `待机呼吸休闲.webm` | 431 KB | 待机循环动画 |
| `点击回应-元气挥手.webm` | 412 KB | 点击回应动画 |

这两条通过 `include_bytes!` 编译进可执行文件，首次运行时释放到
`%APPDATA%\com.whalepet.desktop\webm\`（只在文件缺失时写，绝不覆盖用户的同名文件）。
完整素材集（106 条）用 `scripts/import-animations.ps1` 一次性导入。

### 2.3 刻意**没有**拷贝的东西

| 上游内容 | 不拷的原因 |
| --- | --- |
| `runtime/electron-helper/*` | 那是 Electron 主进程/preload/自定义 scheme，本项目是 Tauri，**整层不可复用** |
| `src/client/*`（React overlay / 设置页） | 依赖 DSH 的 UI 运行时（`@deepseek-ai/dsh-client-*`），独立应用用不到 |
| `src/host/*`（DSH 插件宿主） | 依赖 `webServer` / `credentials` / `llm` 等 DSH 服务；本项目自带 host 层（Rust） |
| `shared/menu.ts`、`chat.ts`、`notify.ts`、`work-status.ts` | 依赖 DSH 页面环境或会话事件（本项目定位为"完全独立的纯桌宠"） |
| `assets/memes`、`fonts`、`pic`、`preview` | M0 用不到；接表情包/气泡字体时再按需拷 |

## 三、哪些是"借鉴而非拷贝"

以下内容**参考了上游的结论与参数**，但在本仓库重写（因为运行环境不同）：

| 上游实现 | 本仓库对应 | 关系 |
| --- | --- | --- |
| `HIT_BOX`（`shared/constants.ts`） | `src-tauri/src/pet_window.rs` 的 `CANVAS_HIT_BOX` | **契约复制**：数值必须与上游一致，已加注释标注来源 |
| 窗口外扩余量 `WINDOW_MARGIN_RATIO = 0.5` | `pet_window.rs` 与 `src/renderer/coords.ts` | **契约复制**：两处必须一致（一边造窗口、一边换算坐标） |
| `inputBusy` 保护（`preload.js` / `main.js`） | `pet_window.rs::apply_fallback_hit` + `runtime.ts::reportBusy` | **语义继承**：Tauri 没有 `setIgnoreMouseEvents(forward)`，方案改为"Rust 轮询光标 + 前端精确判定"，但"busy 期间绝不翻回穿透"这条铁律原样继承 |
| 每宠一个局部小窗（避免 DWM 黑屏） | `pet_window.rs::create_one` | **结论继承**：上游实测全屏透明窗会黑屏，直接采用小窗方案 |
| 「配置是唯一真相，失败大声报错」 | `config.rs` + `renderer/dom.ts::showFatalError` | **约定继承** |
| 跟手弹簧 `springStep`（K=200 / C=30，ζ≈1.06） | `src/renderer/drag.ts` 的 `DRAG_FOLLOW_K/C`（K=600，C=2√K） | **有意偏离（唯一一处物理常量）**：公式一字不差照抄，只改 K/C 的比例——上游 `v·C/K = 0.15s` 的随动滞后在快速拖动时"太飘"（用户实测），改成 `0.082s`（滞后降到 55%），阻尼仍按临界配置不 overshoot。`reference/shared/physics.ts` 保持**逐字节零改动**；这颗旋钮在 `drag.ts` 里是单个常量 |
| 面板描边（共享 `MENU_CSS` 里 `.dsh-pet-menu-column` 的 `box-shadow`） | `src/menu-page.ts` 的 `MENU_CSS_OVERRIDE` | **有意偏离（外观微调）**：共享样式只给了 `0 8px 28px` 的投影，面板压在浅色壁纸/白底上时边界不够利落，桌面端追加一圈 `0 0 0 1px rgba(0,0,0,.05)` 的贴边描边。做法是"注入时把覆盖串追加在 `MENU_CSS` 之后"（同优先级后者生效），所以 `reference/shared/menu.ts` 仍是**逐字节零改动**；上游浏览器端不受影响 |

## 四、如何从上游取更新

```powershell
# 1) 看上游从基线到现在改了哪些文件
git -C D:\programs\github\whale-pet log --oneline d3988fa..HEAD -- dsh-pet/src/shared

# 2) 只看某个被拷贝文件的具体改动（逐字节拷贝，可直接 diff）
git -C D:\programs\github\whale-pet diff d3988fa..HEAD -- dsh-pet/src/shared/physics.ts

# 3) 手工把需要的改动应用回本仓库（保持"零改动拷贝"这一约定：
#    不要在 reference/shared 里改业务逻辑；需要适配就写在本仓库的 src/ 里）
```

**纪律**：`reference/shared/**` 保持与上游逐字节一致，
一旦发现必须改动，说明该逻辑不适合放在共享层——
应该在 `src/` 里包一层适配（现有例子：`src/renderer/drag.ts` 把 shared 的 `(vx, vy)`
命名转换成本项目的桥接契约 `(x, y)`，而不是去改 `physics.ts`）。
