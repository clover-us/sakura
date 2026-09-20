# 图标图形源（`icons/design/`）

应用图标（exe / 任务栏 / 托盘图标 / 安装包 / 两处页头 logo）**都出自这个目录**，
由 `cd src-tauri; cargo run --example make-icon` 生成产物。改图标 = 改这里的文件 + 跑一次工具。

## 当前用的是哪一份（2026-09-21 起）

| 文件 | 是什么 |
| --- | --- |
| `app-icon.png` | **当前图形源**：256×256 的位图（用户给的"原子轨道"图标，彩色那款） |
| `app-icon-16.png`<br>`app-icon-32.png`<br>`app-icon-64.png` | **该尺寸的原图帧**：用户素材里本来就给了 16/32/64，工具会**直接使用**它们，不再重采样（重采样会让细笔画淡一截，而托盘用的正是 32px 那张）。命名是约定：`<主图名>-<边长>.png` |

其余尺寸（24/48/128/1024）由工具从 `app-icon.png` 现算：缩小走面积平均、线性光里取平均，
放大走双线性。**能用原图就用原图**这条只对"用户给了的尺寸"生效。

## 其余文件

| 路径 | 用途 |
| --- | --- |
| `alt/atom-line-{16,32,64}.png` | 用户给的**另一套样式**（黑色描边）的原图。没选它：深色任务栏/深色界面上轨道会整个消失（选型依据见 `docs/VERIFICATION.md` 22.1） |
| `candidates/` | 候选对比图（`--sheet`）的来源。`atom-colour.png` / `atom-line.png` 是这次两套样式的 256px 原图；`a-peek.svg` … `g-pet-peek-tall-ears.svg` 是 2026-09-19 那轮鲸鱼候选（历史记录） |
| `app-icon.svg` | **上一代**图形源（手写的鲸鱼矢量图，2026-09-19 ~ 2026-09-21）。留作历史，工具不再读它 |

## 换图标的三种用法

```powershell
cd src-tauri
cargo run --example make-icon                       # 用 app-icon.png（+ 同名原图帧）
cargo run --example make-icon -- --png 别的图.png     # 换位图源（同名 <名>-<尺寸>.png 仍会被优先使用）
cargo run --example make-icon -- --svg 别的图.svg     # 换回矢量源
cargo run --example make-icon -- --sheet             # 只出候选对比图，不动正式产物
```

产物里 `icons/icon.ico` 是给 exe/任务栏/安装包的，`icons/tray-32.rgba` 是托盘运行时读的裸 RGBA，
`src/assets/app-logo.png` 是两处页头 logo 引用的那张。

> 踩过的坑（第 11 节记录过一次，2026-09-21 又撞了一次）：改完 `icons/icon.ico` 直接
> `cargo build`，**exe 里的图标不会更新**——它由构建脚本嵌入，而构建脚本只在 `tauri.conf.json`
> 变化时重跑。碰一下那个文件的修改时间再构建（或 `cargo clean -p whale-pet-desktop`）。
