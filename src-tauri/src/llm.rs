//! LLM 适配层（M3）：把"一句话/一轮对话"变成一次 OpenAI 兼容的 HTTP 请求。
//!
//! ## 为什么在 Rust 侧发请求
//!
//! 页面里的 `fetch` 会被 CORS 挡住（各家 LLM 接口都不给浏览器跨域头），
//! 所以在 Rust 侧发；附带好处是**页面 CSP 不需要放行任何外部域名**（少一个安全口子）。
//!
//! ## 为什么用 ureq 而不是 reqwest
//!
//! 我们只需要"发一个 JSON、读一个 JSON"。reqwest 会带进 hyper/tower 一整套；
//! ureq + native-tls（Windows 上就是 schannel）只多 4 个 crate，且不需要 C 工具链。
//! 将来要做打字机式的流式输出，再评估换实现——**只需要改本文件**。
//!
//! ## 失败一律"结构化"
//!
//! 上游的做法值得照抄：失败返回 `reason`（`no-key` / `timeout` / `unauthorized` …），
//! 由调用方决定怎么展示；**绝不用编造的文案冒充模型输出**。

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::config::LlmConfig;

/// 碎碎念的 user 指令（与上游 `host/whisper.ts:32` 逐字一致）
pub const WHISPER_USER_PROMPT: &str = "随便说一句日常碎碎念，一句就好，20 字以内。";

/// 一条对话消息（OpenAI 兼容形状）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

impl ChatMessage {
    pub fn system(content: impl Into<String>) -> Self {
        ChatMessage { role: "system".to_string(), content: content.into() }
    }
    pub fn user(content: impl Into<String>) -> Self {
        ChatMessage { role: "user".to_string(), content: content.into() }
    }
    pub fn assistant(content: impl Into<String>) -> Self {
        ChatMessage { role: "assistant".to_string(), content: content.into() }
    }
}

/// 结构化的失败原因（前端据此显示中文提示；`reason` 用于日志与自检输出）
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    /// 总开关没开
    Disabled,
    /// 这个 provider 需要 key，但本机还没存
    NoKey,
    /// 配置里没有可用模型
    NoModel,
    /// 连不上（DNS/网络/TLS）
    Offline,
    /// 超时
    Timeout,
    /// 401/403
    Unauthorized,
    /// 429
    RateLimited,
    /// 其它 HTTP 状态
    Http(u16),
    /// 返回了 200 但内容不可用（含"模型未返回文本"）
    BadResponse(String),
}

impl Failure {
    /// 机器可读的原因（日志/自检用）
    pub fn reason(&self) -> &'static str {
        match self {
            Failure::Disabled => "disabled",
            Failure::NoKey => "no-key",
            Failure::NoModel => "no-model",
            Failure::Offline => "offline",
            Failure::Timeout => "timeout",
            Failure::Unauthorized => "unauthorized",
            Failure::RateLimited => "rate-limited",
            Failure::Http(_) => "http-error",
            Failure::BadResponse(_) => "bad-response",
        }
    }

    /// 给人看的中文说明（设置窗口/气泡里显示）
    pub fn message(&self) -> String {
        match self {
            Failure::Disabled => "AI 功能还没开启（设置 → AI）".to_string(),
            Failure::NoKey => "还没有填 API key（设置 → AI）".to_string(),
            Failure::NoModel => "还没有选模型（设置 → AI）".to_string(),
            Failure::Offline => "连不上模型服务：检查网络，或确认 baseUrl 是否正确".to_string(),
            Failure::Timeout => "模型服务响应超时".to_string(),
            Failure::Unauthorized => "API key 无效或没有权限（401/403）".to_string(),
            Failure::RateLimited => "请求太频繁或额度用尽（429）".to_string(),
            Failure::Http(code) => format!("模型服务返回 HTTP {code}"),
            Failure::BadResponse(detail) => format!("模型返回的内容无法使用：{detail}"),
        }
    }
}

/// 适配层要用的东西：配置 + 可选的 key
pub struct Client<'a> {
    pub config: &'a LlmConfig,
    /// API key（`provider = ollama` 时可以是 `None`）
    pub api_key: Option<&'a str>,
}

impl<'a> Client<'a> {
    pub fn new(config: &'a LlmConfig, api_key: Option<&'a str>) -> Self {
        Client { config, api_key }
    }

    /// 组装 system prompt：人设 + 名字声明（与上游 `host/index.ts:342-349` 同一拼法）
    pub fn system_prompt(&self, pet_name: &str) -> String {
        let persona = self.config.effective_persona();
        let name = pet_name.trim();
        if name.is_empty() {
            persona
        } else {
            format!("{persona}\n你的名字是“{name}”。")
        }
    }

    /// 把历史 + 本轮输入拼成请求里的 messages
    pub fn build_messages(
        &self,
        pet_name: &str,
        history: &[ChatMessage],
        user_text: &str,
    ) -> Vec<ChatMessage> {
        let mut messages = Vec::with_capacity(history.len() + 2);
        messages.push(ChatMessage::system(self.system_prompt(pet_name)));
        messages.extend(history.iter().cloned());
        messages.push(ChatMessage::user(user_text));
        messages
    }

    /// 请求体（单独抽出来是为了能被冒烟检查直接断言，不必真的发请求）
    pub fn request_body(&self, messages: &[ChatMessage]) -> Result<serde_json::Value, Failure> {
        let model = self.config.effective_model().map_err(|_| Failure::NoModel)?;
        Ok(serde_json::json!({
            "model": model,
            "messages": messages,
            "temperature": self.config.temperature,
            // 与上游一致：不传 maxTokens（推理模型会把思考算进预算，显式小上限会截断正文）
            "stream": false,
        }))
    }

    /// 发一次 chat/completions，返回助手回复的纯文本
    pub fn complete(&self, messages: &[ChatMessage]) -> Result<String, Failure> {
        if !self.config.enabled {
            return Err(Failure::Disabled);
        }
        let needs_key = crate::config::provider_needs_key(self.config.provider.trim());
        let key = self.api_key.map(str::trim).filter(|k| !k.is_empty());
        if needs_key && key.is_none() {
            return Err(Failure::NoKey);
        }

        let base = self.config.effective_base_url().map_err(|_| Failure::NoModel)?;
        let url = format!("{base}/chat/completions");
        let body = self.request_body(messages)?;

        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(self.config.timeout_sec)))
            .build()
            .into();

        let mut request = agent
            .post(&url)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json")
            // 上游余额接口用的就是这个 User-Agent 风格；写清楚是谁在请求，方便用户自查
            .header("User-Agent", "whale-pet-desktop");
        if let Some(key) = key {
            request = request.header("Authorization", &format!("Bearer {key}"));
        }

        let mut response = match request.send_json(&body) {
            Ok(response) => response,
            Err(err) => return Err(map_transport_error(err)),
        };

        // 先从 body 里把文本抠出来；状态码已经由 ureq 处理（非 2xx 走上面的错误分支）
        let text = response
            .body_mut()
            .read_to_string()
            .map_err(|err| map_transport_error(err))?;
        extract_assistant_text(&text)
    }
}

/// 把 ureq 的传输层错误映射成结构化失败
fn map_transport_error(err: ureq::Error) -> Failure {
    match err {
        ureq::Error::StatusCode(code) => match code {
            401 | 403 => Failure::Unauthorized,
            429 => Failure::RateLimited,
            other => Failure::Http(other),
        },
        ureq::Error::Timeout(_) => Failure::Timeout,
        ureq::Error::ConnectionFailed | ureq::Error::HostNotFound => Failure::Offline,
        ureq::Error::Io(io) if io.kind() == std::io::ErrorKind::TimedOut => Failure::Timeout,
        ureq::Error::Io(_) | ureq::Error::Tls(_) | ureq::Error::NativeTls(_) => Failure::Offline,
        ureq::Error::Json(err) => Failure::BadResponse(format!("JSON 解析失败：{err}")),
        other => Failure::BadResponse(format!("{other}")),
    }
}

/// 从 OpenAI 兼容响应里取出助手文本。
///
/// - 标准形状：`choices[0].message.content` 是字符串；
/// - 有些实现（以及带多模态的）给的是**内容块数组** `[{type:"text",text:"…"}]`，这里也认；
/// - 拿不到任何文本 → `BadResponse`，**不返回空串**（空串会被前端当成"成功但没说话"，
///   与上游 `'模型未返回文本'` 的处理一致）。
pub fn extract_assistant_text(raw: &str) -> Result<String, Failure> {
    let value: serde_json::Value = serde_json::from_str(raw)
        .map_err(|err| Failure::BadResponse(format!("响应不是合法 JSON：{err}")))?;

    // 有些网关把错误写在 200 的 body 里
    if let Some(error) = value.get("error") {
        let message = error
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("接口返回了 error 字段");
        return Err(Failure::BadResponse(message.to_string()));
    }

    let content = value
        .get("choices")
        .and_then(|choices| choices.get(0))
        .and_then(|choice| choice.get("message"))
        .and_then(|message| message.get("content"));

    let text = match content {
        Some(serde_json::Value::String(text)) => text.clone(),
        Some(serde_json::Value::Array(blocks)) => blocks
            .iter()
            .filter_map(|block| block.get("text").and_then(|t| t.as_str()))
            .collect::<Vec<_>>()
            .join(""),
        _ => String::new(),
    };

    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err(Failure::BadResponse("模型未返回文本".to_string()));
    }
    Ok(trimmed.to_string())
}

/// 碎碎念的 user 文本（开配图时上游会追加一段说明；本阶段先不做配图）
pub fn whisper_user_text() -> String {
    WHISPER_USER_PROMPT.to_string()
}

// ---------------------------------------------------------------------------
//  高层入口：给命令层与定时器共用（都在这一处收口，避免两处各拼一套）
// ---------------------------------------------------------------------------

/// 从本机密钥库读 API key。
///
/// 读不出来（没存过 / 换了 Windows 账户解不开）**不返回错误**，而是返回 `None`：
/// 上层会得到统一的 `no-key`，让用户去设置里重填——这比抛一堆底层错误清晰得多。
pub fn load_key(app_data_dir: &std::path::Path) -> Option<String> {
    match crate::secret::SecretStore::new(app_data_dir).load() {
        Ok(key) => key,
        Err(err) => {
            eprintln!("[whale-pet] 读取 API key 失败（按未配置处理）：{err}");
            None
        }
    }
}

/// 生成一句碎碎念
pub fn whisper(
    config: &LlmConfig,
    app_data_dir: &std::path::Path,
    pet_name: &str,
) -> Result<String, Failure> {
    let key = load_key(app_data_dir);
    let client = Client::new(config, key.as_deref());
    let messages = client.build_messages(pet_name, &[], &whisper_user_text());
    client.complete(&messages)
}

/// 一轮对话（含记忆读写）：成功后把这一轮写进 `memory.json`
pub fn chat(
    config: &LlmConfig,
    app_data_dir: &std::path::Path,
    pet_id: &str,
    pet_name: &str,
    user_text: &str,
) -> Result<String, Failure> {
    let text = user_text.trim();
    if text.is_empty() {
        return Err(Failure::BadResponse("消息为空".to_string()));
    }
    if text.chars().count() > 2000 {
        // 与上游同值（host/index.ts:786-797）
        return Err(Failure::BadResponse("消息过长（限 2000 字）".to_string()));
    }

    let memory = crate::memory::MemoryStore::new(app_data_dir);
    let history = memory.recent_messages(pet_id, config.chat.memory_rounds);
    let key = load_key(app_data_dir);
    let client = Client::new(config, key.as_deref());
    let messages = client.build_messages(pet_name, &history, text);
    let reply = client.complete(&messages)?;

    // 记忆写失败不影响这次回复（用户已经看到答案了），只在日志里说明
    if let Err(err) = memory.append_round(pet_id, text, &reply) {
        eprintln!("[whale-pet] 对话记忆写入失败：{err}");
    }
    Ok(reply)
}

/// 连通性自检：用最小代价真发一次请求（碎碎念那句提示词就是最小代价）
pub fn selftest(
    config: &LlmConfig,
    app_data_dir: &std::path::Path,
    pet_name: &str,
) -> Result<String, Failure> {
    if !config.enabled {
        return Err(Failure::Disabled);
    }
    whisper(config, app_data_dir, pet_name)
}
