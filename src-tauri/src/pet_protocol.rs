//! 自定义协议 `pet://`：给 webview 提供本地素材（动画 / 字体 / 光标图标）。
//!
//! 为什么用自定义协议而不是本地 HTTP 服务：
//!   - 不需要监听端口（端口占用、防火墙弹窗、其它进程访问都是额外的风险面）；
//!   - 不经过网络栈，路径遍历风险由本文件的 `sanitize_join` 单点收口；
//!   - 与 Tauri 的 CSP/权限模型天然配合（协议被登记为 privileged）。
//!
//! 平台差异（务必注意）：
//!   Windows / Android 上自定义协议表现为 `http://pet.localhost/<path>`，
//!   其它平台是 `pet://localhost/<path>`。前端不做平台判断，统一使用 Rust 注入的
//!   `assetBaseUrl`（见 `asset_base_url()`）。
//!
//! 关于 Range 请求：M0 一次性返回整个文件（动画素材 0.2~2MB，可接受）。
//! 若将来启用"长视频/大素材 + 拖动进度条"，需要在此实现 Range 支持。

use std::path::{Component, Path, PathBuf};

use tauri::http::{header, Request, Response, StatusCode};

/// 协议名（同时是 tauri.conf.json 与应用初始化里注册的名字）
pub const SCHEME: &str = "pet";

/// 资源根下的子目录名（与 config.rs 的 `WEBM_DIR_NAME` 保持一致）
const WEBM_SUBDIR: &str = "webm";

/// 前端使用的资源根地址（随平台变化）
pub fn asset_base_url() -> String {
    if cfg!(any(windows, target_os = "android")) {
        // Windows 上自定义协议被解析为 http://<scheme>.localhost
        format!("http://{SCHEME}.localhost")
    } else {
        format!("{SCHEME}://localhost")
    }
}

/// 处理一次资源请求。
///
/// 返回的 `Response` 语义：
///   200 正常文件；400 路径非法（含穿越尝试）；404 文件不存在；500 读取失败。
/// 刻意把"路径非法"与"文件不存在"分开：前者是安全问题（说明 URL 构造有问题），
/// 后者是正常业务情况（用户删了某个动画文件）。
pub fn handle_request(app_data_dir: &Path, request: &Request<Vec<u8>>) -> Response<Vec<u8>> {
    let raw_path = request.uri().path();

    // 协议根路径：给出可读的说明（浏览器直接打开 pet://localhost 时能看到）
    if raw_path == "/" || raw_path.is_empty() {
        return text_response(StatusCode::OK, &format!("whale-pet asset protocol\nroot: /{WEBM_SUBDIR}/<file>"));
    }

    // 只服务 /webm/ 下的素材（M0）；越界一律 400
    let Some(relative) = raw_path.strip_prefix(&format!("/{WEBM_SUBDIR}/")) else {
        return text_response(
            StatusCode::BAD_REQUEST,
            &format!("只支持 /{WEBM_SUBDIR}/ 下的素材请求，收到：{raw_path}"),
        );
    };

    // 百分号解码：中文动画文件名（如 待机.webm）在 URL 里是 %E5%BE%85... 形式
    let decoded = match percent_encoding::percent_decode_str(relative).decode_utf8() {
        Ok(name) => name.into_owned(),
        Err(_) => return text_response(StatusCode::BAD_REQUEST, "素材名不是合法的 UTF-8 百分号编码"),
    };

    let root = app_data_dir.join(WEBM_SUBDIR);
    let Some(full_path) = sanitize_join(&root, &decoded) else {
        return text_response(StatusCode::BAD_REQUEST, "素材路径非法（疑似路径穿越）");
    };

    if !full_path.is_file() {
        return text_response(StatusCode::NOT_FOUND, &format!("素材不存在：{decoded}"));
    }

    // M0 一次性读入：素材体积小，读文件的抖动远低于视频解码本身；
    // 也让调用方不必处理"流式响应在 webview 侧被中断"的边界情况。
    let bytes = match std::fs::read(&full_path) {
        Ok(bytes) => bytes,
        Err(err) => return text_response(StatusCode::INTERNAL_SERVER_ERROR, &format!("读取素材失败：{err}")),
    };

    let mime = mime_for(&full_path);
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, mime)
        .header(header::CONTENT_LENGTH, bytes.len().to_string())
        // 素材是本地文件，允许较长的客户端缓存：动画循环播放时避免反复解码同一条
        .header(header::CACHE_CONTROL, "public, max-age=3600")
        // 明确允许跨源读取（开发模式下页面来自 http://localhost:1420）
        .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
        .body(bytes)
        .unwrap_or_else(|err| text_response(StatusCode::INTERNAL_SERVER_ERROR, &format!("构造响应失败：{err}")))
}

/// 按扩展名给出 MIME 类型；未知扩展名按二进制流处理（浏览器会下载而不是播放）。
fn mime_for(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).as_deref() {
        Some("webm") => "video/webm",
        Some("mp4") => "video/mp4",
        Some("mov") => "video/quicktime",
        Some("png") => "image/png",
        Some("webp") => "image/webp",
        Some("ttf") => "font/ttf",
        Some("woff") => "font/woff",
        Some("woff2") => "font/woff2",
        _ => "application/octet-stream",
    }
}

/// 把相对路径安全地拼到根目录下；发现路径穿越（`..`、绝对路径、盘符）时返回 `None`。
///
/// 之所以不用 `canonicalize` 后再比较前缀：canonicalize 需要路径已存在，
/// 对"文件不存在"的场景拿不到结果，会把 404 变成 400。这里改成**逐段白名单**：
/// 只接受普通文件名与普通子目录名，从根上排除穿越可能。
fn sanitize_join(root: &Path, relative: &str) -> Option<PathBuf> {
    let candidate = Path::new(relative);
    let mut joined = root.to_path_buf();
    let mut segments = 0usize;

    for component in candidate.components() {
        match component {
            // 普通文件名/目录名：只做一层白名单校验
            Component::Normal(name) => {
                let text = name.to_str()?;
                if text.is_empty() || text == "." || text == ".." {
                    return None;
                }
                joined.push(text);
                segments += 1;
            }
            // 根目录、盘符、`..`、`.` 一律拒绝
            _ => return None,
        }
    }
    // 至少要有一段（`/webm/` 后面的文件名不能为空）
    if segments == 0 {
        return None;
    }
    Some(joined)
}

/// 纯文本响应（错误信息用，内容刻意保持简洁以便在控制台直接读懂）
fn text_response(status: StatusCode, message: &str) -> Response<Vec<u8>> {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
        .body(message.as_bytes().to_vec())
        .unwrap_or_else(|_| Response::new(Vec::new()))
}

// ---------------------------------------------------------------------------
// 测试支持（供 src/bin/logic-smoke.rs 使用）
//
// 本机 GNU 工具链跑不起 `cargo test` 的 libtest 可执行文件（见 logic-smoke.rs 头部说明），
// 因此把需要被冒烟程序验证的私有纯函数通过这一小节暴露出去。
// 这些函数只做字符串/路径判断，`None`/`Some` 的语义原样保留，不改变生产行为。
// ---------------------------------------------------------------------------

/// 测试支持：验证路径安全规则（`Some` = 接受，`None` = 拒绝）
#[doc(hidden)]
pub fn sanitize_path_for_test(relative: &str) -> Option<std::path::PathBuf> {
    sanitize_join(std::path::Path::new("test-root"), relative)
}

/// 测试支持：查询扩展名到 MIME 的映射
#[doc(hidden)]
pub fn mime_for_test(file_name: &str) -> &'static str {
    mime_for(std::path::Path::new(file_name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_join_accepts_plain_filenames() {
        let root = Path::new("C:/data/webm");
        let joined = sanitize_join(root, "待机.webm").expect("普通文件名应被接受");
        assert!(joined.ends_with("待机.webm"));
    }

    #[test]
    fn sanitize_join_rejects_traversal() {
        let root = Path::new("C:/data/webm");
        assert!(sanitize_join(root, "../secret.txt").is_none(), "上级目录必须被拒绝");
        assert!(sanitize_join(root, "a/../../b.webm").is_none(), "嵌套穿越必须被拒绝");
        assert!(sanitize_join(root, "/etc/passwd").is_none(), "绝对路径必须被拒绝");
        assert!(sanitize_join(root, "").is_none(), "空路径必须被拒绝");
    }

    #[test]
    fn mime_types_cover_animation_and_font_assets() {
        assert_eq!(mime_for(Path::new("a.webm")), "video/webm");
        assert_eq!(mime_for(Path::new("a.TTF")), "font/ttf");
        assert_eq!(mime_for(Path::new("a.unknown")), "application/octet-stream");
    }

    #[test]
    fn asset_base_url_matches_platform_convention() {
        let url = asset_base_url();
        if cfg!(windows) {
            assert_eq!(url, "http://pet.localhost");
        } else {
            assert_eq!(url, "pet://localhost");
        }
    }
}
