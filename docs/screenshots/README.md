# 界面截图（供视觉验收）

这些图不是"设计稿"，而是**真实运行的窗口截图**，由
[`scripts/capture-window.ps1`](../../scripts/capture-window.ps1) 抓取：

```powershell
pnpm tauri dev                                    # 另一个终端
$env:WHALE_PET_DIAG_SETTINGS = '1'                # 或 nav:physics / nav:animations / nav:system / ownbehaviour
$env:WHALE_PET_DIAG_TRAY_MENU = '6000:picker'     # 托盘菜单（:picker 展开动作点播）
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\capture-window.ps1 -TitleLike '设置'
```

> 走的是**屏幕合成截图**：WebView2 的内容用 `PrintWindow` 抓不到（返回全黑，DirectComposition 的已知行为）。
> 抓之前脚本会把目标窗口提到前台，因此**图里如果看到桌宠压在上面，那是真的被它压着**
> （宠物窗是 `always_on_top`）。

| 图 | 内容 |
| --- | --- |
| `settings-pet.png` | 设置窗口 · 宠物页：左侧导航（带 Lucide 图标）、基本信息、行为（跟随全局 / 单独设置）、移除 |
| `settings-physics.png` | 设置窗口 · 物理参数：四个数值 + 两个勾选行（这里的勾选行曾是"被挤成竖排文字"的 bug） |
| `settings-animations.png` | 设置窗口 · 动画池默认值：待机/转向/拖拽/点击回应池 + 移动池 + 分类 + 权重 |
| `settings-system.png` | 设置窗口 · 启动与系统：开机自启、文件与位置、运行方式 |
| `settings-ai.png` | 设置窗口 · **AI（M3）**：总开关/碎碎念/对话、服务商与模型、Key 保存与自检（底部状态栏是"自检通过"的实测结果） |
| `tray-menu.png` | 托盘菜单（浅色）：鲸鱼 logo、显示/隐藏切换项、回到初始位置、动作点播、设置、退出 |
| `tray-menu-picker.png` | 托盘菜单 · 动作点播：**分类默认全部收起**，右侧显示每个分类的条数 |
| `tray-menu-picker-expanded.png` | 托盘菜单 · 点开「待机」之后：只展开被点的分类，窗口自动变高 |
| `settings-dark.png` | 设置窗口 · **深色**（`?theme=dark` 强制；默认仍跟随系统） |
| `tray-menu-dark.png` | 托盘菜单 · **深色**（同上） |
| `icon-candidates.png` | 图标候选（256px 放大 + 64/32/16 真实像素）。前两款是鲸鱼（贴合当前素材），其余**不指向物种**；**用户选定并已采用第 4 款 `d-pet-peek`**（小生物趴在任务栏上），图里它那一列下方有粉色标记线 |

界面风格参考了用户提供的截图：**浅色底 + 左侧竖向导航 + 单一粉色强调色 + 细边框 + 大留白**；
深色那一套是通过 `prefers-color-scheme` 切换的同一份结构（本机桌面为浅色，深色未被真机渲染验证过）。

菜单/导航/按钮里的线性图标形状来自 [Lucide](https://lucide.dev)（ISC 许可），
应用图标是矢量 SVG 手绘（`src-tauri/icons/design/app-icon.svg`，由 `cargo run --example make-icon` 栅格化）。
