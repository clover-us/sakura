//! 本地 mock LLM 服务端（**开发/验证工具，不属于应用运行路径**）。
//!
//! 用法：
//!
//! ```powershell
//! cargo run --bin mock-llm            # 默认监听 127.0.0.1:8787
//! cargo run --bin mock-llm -- 9000    # 指定端口
//! ```
//!
//! 然后把配置指向它，即可**不花一分钱、不需要真实 key** 跑通整条链路：
//!
//! ```jsonc
//! "llm": { "enabled": true, "provider": "custom",
//!          "baseUrl": "http://127.0.0.1:8787", "model": "mock", "timeoutSec": 30,
//!          "whisper": { "enabled": true, "intervalSec": 30 },
//!          "chat": { "enabled": true, "memoryRounds": 5 } }
//! ```
//!
//! 路由（按前缀切换行为，方便验证各条失败分支）：
//!
//! | 路径 | 行为 |
//! | --- | --- |
//! | `/chat/completions` | 200 + 正常回复 |
//! | `/unauthorized/chat/completions` | 401（验证 `unauthorized`） |
//! | `/rate-limited/chat/completions` | 429（验证 `rate-limited`） |
//! | `/empty/chat/completions` | 200 但 content 为空（验证"模型未返回文本"） |
//! | `/broken/chat/completions` | 200 但不是 JSON（验证 `bad-response`） |
//! | `/slow/chat/completions` | 睡 6 秒再回（验证超时；把 timeoutSec 调到 5 即可触发） |
//!
//! **它会把收到的 system/user 原文打出来** —— 这是验证"请求组装是否符合设计"
//! （人设 + 名字声明 + 最近 N 轮 + 本轮输入）最直接的证据。
//!
//! 实现上用 `std::net::TcpListener` 手写最小 HTTP/1.1：只处理 `POST` + `Content-Length`，
//! 不引入任何依赖（验证工具不该给项目增加依赖面）。

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread;
use std::time::Duration;

fn main() -> std::io::Result<()> {
    let port: u16 = std::env::args().nth(1).and_then(|value| value.parse().ok()).unwrap_or(8787);
    let listener = TcpListener::bind(("127.0.0.1", port))?;
    println!("[mock-llm] 监听 http://127.0.0.1:{port}");
    println!("[mock-llm] 把 llm.provider 设成 custom、baseUrl 设成 http://127.0.0.1:{port} 即可");
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                thread::spawn(move || {
                    if let Err(err) = handle(stream) {
                        eprintln!("[mock-llm] 处理请求失败：{err}");
                    }
                });
            }
            Err(err) => eprintln!("[mock-llm] 接受连接失败：{err}"),
        }
    }
    Ok(())
}

fn handle(mut stream: TcpStream) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);

    // ---- 请求行 ----
    let mut request_line = String::new();
    reader.read_line(&mut request_line)?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let path = parts.next().unwrap_or("/").to_string();

    // ---- 头（只需要 Content-Length）----
    let mut content_length = 0usize;
    loop {
        let mut line = String::new();
        reader.read_line(&mut line)?;
        let trimmed = line.trim_end();
        if trimmed.is_empty() {
            break;
        }
        if let Some(value) = trimmed.to_ascii_lowercase().strip_prefix("content-length:") {
            content_length = value.trim().parse().unwrap_or(0);
        }
    }

    // ---- body ----
    let mut body = vec![0u8; content_length];
    if content_length > 0 {
        reader.read_exact(&mut body)?;
    }
    let body_text = String::from_utf8_lossy(&body).to_string();

    println!("[mock-llm] {method} {path}");
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(&body_text) {
        println!("[mock-llm]   model={}", value.get("model").and_then(|m| m.as_str()).unwrap_or("-"));
        if let Some(messages) = value.get("messages").and_then(|m| m.as_array()) {
            for message in messages {
                let role = message.get("role").and_then(|r| r.as_str()).unwrap_or("?");
                let content = message.get("content").and_then(|c| c.as_str()).unwrap_or("");
                println!("[mock-llm]   {role}: {content}");
            }
        }
    } else {
        println!("[mock-llm]   body（非 JSON）：{body_text}");
    }

    // ---- 按路径决定行为 ----
    if path.contains("/unauthorized/") {
        return respond(&mut stream, 401, "application/json", r#"{"error":{"message":"invalid api key"}}"#);
    }
    if path.contains("/rate-limited/") {
        return respond(&mut stream, 429, "application/json", r#"{"error":{"message":"rate limit exceeded"}}"#);
    }
    if path.contains("/empty/") {
        return respond(
            &mut stream,
            200,
            "application/json",
            r#"{"choices":[{"message":{"role":"assistant","content":"   "}}]}"#,
        );
    }
    if path.contains("/broken/") {
        return respond(&mut stream, 200, "application/json", "这不是 JSON");
    }
    if path.contains("/slow/") {
        thread::sleep(Duration::from_secs(6));
    }

    // 正常回复：把最后一条 user 内容回显进去，便于一眼看出"记忆/上下文有没有带上"
    let reply = format!("（mock 回复）收到 {} 条消息", count_messages(&body_text));
    let payload = serde_json::json!({
        "id": "mock-1",
        "object": "chat.completion",
        "model": "mock",
        "choices": [{ "index": 0, "message": { "role": "assistant", "content": reply }, "finish_reason": "stop" }],
        "usage": { "prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2 }
    });
    respond(&mut stream, 200, "application/json", &payload.to_string())
}

fn count_messages(body: &str) -> usize {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|value| value.get("messages").and_then(|m| m.as_array().map(|a| a.len())))
        .unwrap_or(0)
}

fn respond(stream: &mut TcpStream, status: u16, content_type: &str, body: &str) -> std::io::Result<()> {
    let reason = match status {
        200 => "OK",
        401 => "Unauthorized",
        429 => "Too Many Requests",
        _ => "Error",
    };
    let response = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.as_bytes().len()
    );
    stream.write_all(response.as_bytes())?;
    stream.flush()
}
