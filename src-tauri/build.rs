/**
 * Tauri 构建脚本：由 tauri-build 生成上下文（配置/权限/资源清单）。
 *
 * 刻意不调用 tauri_build::try_build(Attributes::new().windows_attributes(...))，
 * 因为本 M0 阶段不需要自定义清单项（无管理员权限、无 COM 注册）。
 * 日后要加「每显示器 DPI 感知」「Windows 11 圆角」等清单项时，
 * 在这里改用 try_build 并配 Attributes 即可。
 */
fn main() {
    tauri_build::build()
}
