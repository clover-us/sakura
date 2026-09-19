// 发行构建隐藏控制台窗口（Windows GUI 子系统）；调试构建保留控制台便于看日志。
// 注意：inner attribute（`#!`）必须是文件里的第一条语句，不能出现在文档注释之后，
// 因此这里的说明用普通注释、属性放在最前面。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! 二进制入口：把平台入口收敛到一行。
//!
//! 之所以 lib 与 bin 分离（Tauri 官方模板同一做法）：
//!   - lib 侧（src/lib.rs）持有全部逻辑，便于单元测试与将来的移动端复用；
//!   - bin 侧只负责调用 lib 的 `run()`，在 Windows 上避免 bin/lib 同名冲突
//!     （cargo 已知问题 rust-lang/cargo#8519）。

fn main() {
    whale_pet_desktop_lib::run()
}
