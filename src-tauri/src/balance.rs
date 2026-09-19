//! 余额 / 用量查询（M3）：只认两个上游登记过的服务商，其余显式"不支持"。
//!
//! ## 为什么要单独一个模块
//!
//! 余额接口与 LLM 接口**不是一回事**：路径不同、鉴权相同但语义不同（一个是查账户，一个是生成），
//! 失败处理也不同——查余额失败**绝不能显示假数字**（用户会据此判断要不要充值）。
//! 所以这里独立实现，并且只**复用** `llm.rs` 的密钥读取与结构化失败类型。
//!
//! ## 上游语义（照抄的部分）
//!
//! | 服务商 | 接口 | 取什么 |
//! | --- | --- | --- |
//! | `deepseek` | `GET https://api.deepseek.com/user/balance` | `balance_infos[0]` 的 currency / total_balance / granted_balance / topped_up_balance |
//! | `opencode-go` | `GET https://opencode.ai/zen/go/v1/usage` | `usage.{rolling,weekly,monthly}` 的 percent / resetsAt |
//!
//! - 超时 **20 秒**，失败重试（上游固定 800ms ×3 次；这里改成**指数退避** 800/1600/3200ms——
//!   对上游接口更客气，语义仍是"重试 + 退避"）；
//! - **档位**（决定播哪条余额动画）与上游同一算法：
//!   - DeepSeek：`已用% = 100 − min(100, 余额 / ¥20 × 100)`（**¥20 视为满额**）；
//!   - OpenCode：`已用% = max(rolling, weekly, monthly)`；
//!   - `index = 100% ? 5 : min(4, floor(已用% / 20))` → 对应 `animations.events.balance` 的 6 条。
//!
//! ## 本项目的偏离
//!
//! - 动画**不下发动画名，只下发档位下标**：动作池是"每只宠物可以不同"的，
//!   由页面按 `animations.events.balance[index]` 取，才不会出现"宿主挑了一条这只宠物没有的动画"；
//! - 未登记的服务商显式返回 `unsupported`（与上游一致），**不做猜路径**。

use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::config::LlmConfig;
use crate::llm::Failure;

/// DeepSeek 满额基准（人民币）：余额 ≥ 该值视为 100%（未消耗）。与上游同值。
pub const DEEPSEEK_FULL_BALANCE_CNY: f64 = 20.0;

/// OpenCode 各窗口的满额度（美元）：业务常量，与上游同值（5 小时 / 周 / 月）
pub const OPENCODE_QUOTA_USD: [(&str, f64); 3] = [("rolling", 12.0), ("weekly", 30.0), ("monthly", 60.0)];

/// 窗口在气泡里的中文名
pub const WINDOW_LABELS: [(&str, &str); 3] = [("rolling", "5 小时"), ("weekly", "本周"), ("monthly", "本月")];

/// 请求超时（秒）：与上游 20s 一致
pub const TIMEOUT_SEC: u64 = 20;
/// 重试次数（首次失败后再试几次）
pub const RETRIES: u32 = 3;
/// 首次退避（毫秒）；之后每次翻倍
pub const BACKOFF_MS: u64 = 800;

/// 支持的服务商
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    DeepSeek,
    OpencodeGo,
}

impl Provider {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim() {
            "deepseek" => Some(Provider::DeepSeek),
            "opencode-go" => Some(Provider::OpencodeGo),
            _ => None,
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Provider::DeepSeek => "deepseek",
            Provider::OpencodeGo => "opencode-go",
        }
    }

/// 预置接口前缀（与上游一致）
    fn default_base(self) -> &'static str {
        match self {
            Provider::DeepSeek => "https://api.deepseek.com",
            Provider::OpencodeGo => "https://opencode.ai",
        }
    }

    /// 接口路径
    fn path(self) -> &'static str {
        match self {
            Provider::DeepSeek => "/user/balance",
            Provider::OpencodeGo => "/zen/go/v1/usage",
        }
    }

    /// 实际请求地址：`llm.balance.baseUrl` 留空就用预置前缀。
    ///
    /// 有覆盖项的理由与 LLM 那边一样：**能对着本地 mock 验证**（余额接口的解析、档位、
    /// 失败分支都不该只能靠真花钱/真连网来验），顺带也支持用户走自建代理。
    fn url(self, base_url: &str) -> String {
        let base = base_url.trim();
        if base.is_empty() {
            format!("{}{}", self.default_base(), self.path())
        } else {
            format!("{}{}", base.trim_end_matches('/'), self.path())
        }
    }
}

/// 一次查询的结果（给页面用）
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub provider: String,
    /// 气泡里显示的一行文案
    pub text: String,
    /// 已用百分比（0~100，可能 >100 表示透支）
    pub used_percent: f64,
    /// 档位下标（0~5）：页面据此从 `animations.events.balance` 里取动画
    pub animation_index: usize,
}

/// 由"已用百分比"算出档位下标（与上游 `balanceEventIndex` 同一算法）
pub fn animation_index(used_percent: f64) -> usize {
    if used_percent >= 100.0 {
        return 5;
    }
    let index = (used_percent / 20.0).floor();
    if index < 5.0 {
        index.max(0.0) as usize
    } else {
        4
    }
}

/// DeepSeek 余额 → 已用百分比（¥20 视为满额；负数按 0 元处理 = 已用完）
pub fn deepseek_used_percent(total: f64) -> f64 {
    let remaining = (total.max(0.0) / DEEPSEEK_FULL_BALANCE_CNY) * 100.0;
    (100.0 - remaining).clamp(0.0, 100.0)
}

/// 查询余额。失败一律结构化（`disabled` / `unsupported` / `no-key` / `offline` / `timeout` / `http` / `bad-response`）。
pub fn query(config: &LlmConfig, app_data_dir: &Path) -> Result<Snapshot, Failure> {
    if !config.balance.enabled {
        return Err(Failure::Disabled);
    }
    let Some(provider) = Provider::parse(&config.balance.provider) else {
        return Err(Failure::Unsupported);
    };
    let key = crate::llm::load_key(app_data_dir);
    let Some(key) = key else {
        return Err(Failure::NoKey);
    };

    let raw = fetch_with_retry(&provider.url(&config.balance.base_url), &key)?;
    match provider {
        Provider::DeepSeek => parse_deepseek(&raw),
        Provider::OpencodeGo => parse_opencode(&raw),
    }
}

/// 带指数退避的 GET（返回响应体文本）
fn fetch_with_retry(url: &str, key: &str) -> Result<String, Failure> {
    let tls = ureq::tls::TlsConfig::builder()
        .provider(ureq::tls::TlsProvider::NativeTls)
        .build();
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(TIMEOUT_SEC)))
        .tls_config(tls)
        .build()
        .into();

    let mut last: Failure = Failure::Offline;
    for attempt in 0..=RETRIES {
        let request = agent
            .get(url)
            .header("Accept", "application/json")
            // 上游余额接口用的就是这个 User-Agent 风格：写清楚是谁在请求，方便用户自查账单来源
            .header("User-Agent", "whale-pet-balance")
            .header("Authorization", &format!("Bearer {key}"));
        match request.call() {
            Ok(mut response) => match response.body_mut().read_to_string() {
                Ok(text) => return Ok(text),
                Err(err) => last = map_transport(err),
            },
            Err(err) => last = map_transport(err),
        }
        if attempt < RETRIES {
            // 指数退避：800 / 1600 / 3200 ms（最后一次失败后不再等）
            let wait = BACKOFF_MS * 2u64.pow(attempt);
            eprintln!(
                "[whale-pet][余额] 第 {} 次失败（{}），{}ms 后重试",
                attempt + 1,
                last.reason(),
                wait
            );
            std::thread::sleep(Duration::from_millis(wait));
        }
    }
    Err(last)
}

fn map_transport(err: ureq::Error) -> Failure {
    match err {
        ureq::Error::StatusCode(code) => match code {
            401 | 403 => Failure::Unauthorized,
            429 => Failure::RateLimited,
            other => Failure::Http(other),
        },
        ureq::Error::Timeout(_) => Failure::Timeout,
        ureq::Error::ConnectionFailed | ureq::Error::HostNotFound => Failure::Offline,
        ureq::Error::Io(_) | ureq::Error::Tls(_) | ureq::Error::NativeTls(_) => Failure::Offline,
        other => Failure::BadResponse(format!("{other}")),
    }
}

// ---------------------------------------------------------------------------
//  解析
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct DeepSeekBalanceInfo {
    #[serde(default)]
    currency: String,
    #[serde(default)]
    total_balance: String,
    #[serde(default)]
    granted_balance: String,
    #[serde(default)]
    topped_up_balance: String,
}

#[derive(Debug, Deserialize)]
struct DeepSeekBalance {
    #[serde(default)]
    is_available: Option<bool>,
    #[serde(default)]
    balance_infos: Vec<DeepSeekBalanceInfo>,
}

/// 解析 DeepSeek `/user/balance`
pub fn parse_deepseek(raw: &str) -> Result<Snapshot, Failure> {
    let body: DeepSeekBalance = serde_json::from_str(raw)
        .map_err(|err| Failure::BadResponse(format!("余额响应不是合法 JSON：{err}")))?;
    let Some(info) = body.balance_infos.first() else {
        return Err(Failure::BadResponse("余额响应缺少 balance_infos".to_string()));
    };
    let total: f64 = info
        .total_balance
        .trim()
        .parse()
        .map_err(|_| Failure::BadResponse(format!("余额字段不是数字：{:?}", info.total_balance)))?;
    let used = deepseek_used_percent(total);
    let unavailable = if body.is_available == Some(false) { "（账户不可用）" } else { "" };
    let text = format!(
        "余额 {}{}（赠送 {} · 充值 {}）",
        format_money(&info.currency, &info.total_balance),
        unavailable,
        money_or_dash(&info.currency, &info.granted_balance),
        money_or_dash(&info.currency, &info.topped_up_balance),
    );
    Ok(Snapshot {
        provider: Provider::DeepSeek.id().to_string(),
        text,
        used_percent: used,
        animation_index: animation_index(used),
    })
}

#[derive(Debug, Default, Deserialize)]
struct UsageWindow {
    #[serde(default)]
    percent: Option<f64>,
    #[serde(default, rename = "resetsAt")]
    resets_at: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct OpencodeUsage {
    #[serde(default)]
    rolling: Option<UsageWindow>,
    #[serde(default)]
    weekly: Option<UsageWindow>,
    #[serde(default)]
    monthly: Option<UsageWindow>,
}

#[derive(Debug, Default, Deserialize)]
struct OpencodeBody {
    #[serde(default)]
    usage: Option<OpencodeUsage>,
}

/// 解析 OpenCode `/zen/go/v1/usage`
pub fn parse_opencode(raw: &str) -> Result<Snapshot, Failure> {
    let body: OpencodeBody = serde_json::from_str(raw)
        .map_err(|err| Failure::BadResponse(format!("用量响应不是合法 JSON：{err}")))?;
    let usage = body.usage.ok_or_else(|| Failure::BadResponse("用量响应缺少 usage".to_string()))?;
    let windows = [
        ("rolling", usage.rolling.unwrap_or_default()),
        ("weekly", usage.weekly.unwrap_or_default()),
        ("monthly", usage.monthly.unwrap_or_default()),
    ];

    let percent_of = |name: &str| -> Result<f64, Failure> {
        windows
            .iter()
            .find(|(key, _)| *key == name)
            .and_then(|(_, window)| window.percent)
            .ok_or_else(|| Failure::BadResponse(format!("用量响应缺少 {name}.percent")))
    };
    let used = ["rolling", "weekly", "monthly"]
        .iter()
        .map(|name| percent_of(name))
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .fold(0.0_f64, f64::max);

    // 文案：三个窗口的百分比 + "最紧的那个还剩多少钱、什么时候重置"
    let mut parts = Vec::new();
    for (key, label) in WINDOW_LABELS {
        let window = windows.iter().find(|(name, _)| *name == key).map(|(_, w)| w);
        let percent = window.and_then(|w| w.percent).unwrap_or(0.0);
        parts.push(format!("{label} {:.0}%", percent));
    }
    let mut text = format!("用量 {}", parts.join(" · "));

    // 最紧窗口 = 剩余额度最少的那个（与上游 urgentWindow 同一判据）
    let urgent = windows
        .iter()
        .filter_map(|(key, window)| {
            let quota = OPENCODE_QUOTA_USD.iter().find(|(name, _)| name == key).map(|(_, q)| *q)?;
            let percent = window.percent?;
            let remaining = quota * (100.0 - percent) / 100.0;
            let label = WINDOW_LABELS.iter().find(|(name, _)| name == key).map(|(_, l)| *l)?;
            Some((label, remaining, window.resets_at.clone()))
        })
        .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
    if let Some((label, remaining, resets_at)) = urgent {
        text.push_str(&format!("；最紧「{label}」剩 ${remaining:.2}"));
        let reset = reset_in_text(resets_at.as_deref());
        if !reset.is_empty() {
            text.push_str(&format!("（{reset}）"));
        }
    }

    Ok(Snapshot {
        provider: Provider::OpencodeGo.id().to_string(),
        text,
        used_percent: used,
        animation_index: animation_index(used),
    })
}

/// 金额显示：带货币符号（`¥12.34` / `$1.00`），未知货币就直接拼代码
pub fn format_money(currency: &str, amount: &str) -> String {
    let symbol = match currency.trim().to_ascii_uppercase().as_str() {
        "CNY" | "RMB" => "¥",
        "USD" => "$",
        _ => "",
    };
    let trimmed = amount.trim();
    if symbol.is_empty() {
        format!("{trimmed} {}", currency.trim())
    } else {
        format!("{symbol}{trimmed}")
    }
}

fn money_or_dash(currency: &str, amount: &str) -> String {
    if amount.trim().is_empty() {
        format_money(currency, "0")
    } else {
        format_money(currency, amount)
    }
}

/// 距离重置还有多久（与上游同一口径：小时/分钟/已重置）
pub fn reset_in_text(resets_at: Option<&str>) -> String {
    let Some(raw) = resets_at else {
        return String::new();
    };
    let Ok(parsed) = humantime_lite(raw) else {
        return String::new();
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    if parsed <= now {
        return "已重置".to_string();
    }
    let delta = parsed - now;
    let hours = delta / 3600;
    let minutes = (delta % 3600) / 60;
    if hours > 0 {
        format!("{hours} 小时后重置")
    } else {
        format!("{minutes} 分钟后重置")
    }
}

/// 仅解析 `YYYY-MM-DDTHH:MM:SS[.fff][Z]`（UTC）——够用且不引时间库。
///
/// 余额接口的 `resetsAt` 就是这种 ISO-8601 UTC 形状；解析不了就返回空串（不猜、不显示假时间）。
fn humantime_lite(raw: &str) -> Result<u64, ()> {
    let bytes = raw.as_bytes();
    if bytes.len() < 19 {
        return Err(());
    }
    let num = |from: usize, to: usize| -> Result<i64, ()> {
        raw.get(from..to).ok_or(())?.parse::<i64>().map_err(|_| ())
    };
    let (year, month, day) = (num(0, 4)?, num(5, 7)?, num(8, 10)?);
    let (hour, minute, second) = (num(11, 13)?, num(14, 16)?, num(17, 19)?);
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return Err(());
    }
    // 从 1970-01-01 起算的天数（含闰年），再乘 86400
    let mut days: i64 = 0;
    for y in 1970..year {
        days += if is_leap(y) { 366 } else { 365 };
    }
    const MONTH_DAYS: [i64; 12] = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    for m in 0..(month as usize - 1) {
        days += MONTH_DAYS[m];
        if m == 1 && is_leap(year) {
            days += 1;
        }
    }
    days += day - 1;
    let seconds = days * 86400 + hour * 3600 + minute * 60 + second;
    if seconds < 0 {
        return Err(());
    }
    Ok(seconds as u64)
}

fn is_leap(year: i64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

/// 该服务商是否在当前配置里可用（设置页据此显示状态）
pub fn provider_supported(provider: &str) -> bool {
    Provider::parse(provider).is_some()
}

// ---------------------------------------------------------------------------
//  下发到页面
// ---------------------------------------------------------------------------

/// 发给宠物页的余额事件
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BalanceEvent {
    pub pet_label: String,
    /// 气泡文案
    pub text: String,
    /// 档位下标（0~5）：页面从 `animations.events.balance` 里按下标取动画
    pub animation_index: usize,
    pub provider: String,
}

/// 查一次并让宠物"说"出来（气泡 + 档位动画）。
///
/// 与碎碎念走**同一条展示链路**（气泡窗），差别只在动画档位是"按下标取"而不是随机抽。
pub fn query_and_say(app: &tauri::AppHandle, label: &str) -> Result<Snapshot, Failure> {
    use tauri::Manager;
    let (config, app_data_dir) = {
        let state = app.state::<crate::state::AppState>();
        (state.config_snapshot(), state.app_data_dir.clone())
    };
    let snapshot = query(&config.llm, &app_data_dir)?;
    let Some(window) = app.get_webview_window(label) else {
        eprintln!("[whale-pet] 找不到宠物窗口 {label}，余额没处显示：{}", snapshot.text);
        return Ok(snapshot);
    };
    use tauri::Emitter;
    let payload = BalanceEvent {
        pet_label: label.to_string(),
        text: snapshot.text.clone(),
        animation_index: snapshot.animation_index,
        provider: snapshot.provider.clone(),
    };
    if let Err(err) = window.emit(crate::pet_window::EVENT_BALANCE, payload) {
        eprintln!("[whale-pet] 余额下发失败 {label}：{err}");
    }
    Ok(snapshot)
}