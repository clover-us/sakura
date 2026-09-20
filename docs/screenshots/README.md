# 界面截图（供视觉验收）

这些图不是"设计稿"，而是**真实运行的窗口截图**，由
[`scripts/capture-window.ps1`](../../scripts/capture-window.ps1) 抓取：

```powershell
pnpm tauri dev                                    # 另一个终端
$env:WHALE_PET_DIAG_SETTINGS = '1'                # 或 nav:physics / nav:animations / nav:system / ownbehaviour
$env:WHALE_PET_DIAG_TRAY_MENU = '6000:picker'     # 托盘菜单（:picker 展开动作点播；:toast 弹失败提示）
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
| `settings-about.png` | 设置窗口 · 关于：56px 的应用图标 + 版本/许可/文档（2026-09-21 换图标后新增） |
| `chat-window.png` | **对话输入窗（M3）**：宠物右上角的单行输入条（回车发送、Esc 关闭；回复走气泡，不在这里堆记录） |
| `settings-ai.png` | 设置窗口 · **AI（M3）**：总开关/碎碎念/对话、服务商与模型、Key 保存与自检（底部状态栏是"自检通过"的实测结果） |
| `tray-menu.png` | 托盘菜单（浅色）：应用图标、显示/隐藏切换项、回到初始位置、动作点播、设置、退出 |
| `tray-menu-picker.png` | 托盘菜单 · 动作点播：**分类默认全部收起**，右侧显示每个分类的条数 |
| `tray-menu-picker-expanded.png` | 托盘菜单 · 点开「待机」之后：只展开被点的分类，窗口自动变高 |
| `tray-menu-toast.png` | 托盘菜单 · **失败提示完整可见**（点「查余额」时 AI 未开启 → 两行提示都在面板里，见 VERIFICATION 21 节） |
| `settings-dark.png` | 设置窗口 · **深色**（`?theme=dark` 强制；默认仍跟随系统） |
| `tray-menu-dark.png` | 托盘菜单 · **深色**（同上） |
| `icon-sizes.png` | **当前应用图标的全尺寸阶梯**（从 `icon.ico` 里逐帧提取：16/24/32/48/64/128/256，浅色/深色两种底） |
| `icon-style-compare.png` | **换图标时的选型依据**：用户给的两套样式（彩色 / 黑描边）× 16/24/32/48/64 × 四种底色——黑描边在深色底上轨道会消失，故选彩色 |
| `icon-candidates.png` | 候选对比图（`cargo run --example make-icon -- --sheet` 产出）。历史候选（鲸鱼那几款）与当前的原子图标同列，**粉线标的是当前采用的那一款** |

> 2026-09-21 换应用图标（原子）后，上表中带界面截图的那些**已按新图标重拍**；
> `icon-candidates.png` 里的鲸鱼候选是历史记录（第 11 节的选型过程），不表示还在用。

界面风格参考了用户提供的截图：**浅色底 + 左侧竖向导航 + 单一粉色强调色 + 细边框 + 大留白**；
深色那一套是通过 `prefers-color-scheme` 切换的同一份结构（本机桌面为浅色，深色由 `?theme=dark` 强制渲染）。

菜单/导航/按钮里的线性图标形状来自 [Lucide](https://lucide.dev)（ISC 许可）；
应用图标（原子轨道）是**用户提供的素材**，图形源在 `src-tauri/icons/design/`（两套样式各留了原图），
由 `cargo run --example make-icon` 栅格化成 `icon.ico` / 托盘 RGBA / 页头 logo 三份。
