//! 表情包（M3）：素材池、随机抽图、对话选图的标记解析。
//!
//! ## 素材与配置的对应关系（与上游一致）
//!
//! ```text
//! <应用数据目录>/memes/<名称>.png   ← 图片本体（由 scripts/import-animations.ps1 -All 导入）
//! config.jsonc 的 "memes": { "<名称>": "内容描述", … }   ← 描述，模型据此选图
//! ```
//!
//! 两条纪律：
//!
//! 1. **池子只认"图真在 + 描述非空"**：用户删了图却忘了删配置时不该报错，
//!    更不该把不存在的图交给模型（那只会换来一个幻觉名字）；
//! 2. **顺序稳定**：按名称排序，这样提示词里的清单在多次请求之间是稳定的
//!    （模型对同一清单的选择更可复现，排查问题也容易）。
//!
//! ## 两种配图策略（上游的设计，照抄）
//!
//! - **碎碎念随机抽**：碎碎念是"随口一句"，没有上下文可选，所以随机抽一张，
//!   把它的描述注入提示词，让那句话配合画面；
//! - **对话让模型挑**：对话有语境，把整张清单交给模型，它用末尾的 `[图:名称]` 标记要图。
//!   解析时**只在池内命中才采纳**，并把标记从正文里剥掉；幻觉名一律忽略但**绝不吞正文**。

use std::collections::HashMap;
use std::path::Path;

use crate::config::AppConfig;

/// 表情包目录名（位于应用数据目录下）
pub const MEMES_DIR: &str = "memes";

/// 一只表情包（配置里的描述 + 磁盘上的文件）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Meme {
    pub name: String,
    pub description: String,
}

/// 从配置 + 磁盘状态算出**可用的**表情包池（按名称排序）。
///
/// 缺图或描述为空的条目会被静默剔除：配置是用户手边的东西，
/// 少一张图不该让碎碎念/对话整体失效。
pub fn pool(config: &AppConfig, app_data_dir: &Path) -> Vec<Meme> {
    let dir = app_data_dir.join(MEMES_DIR);
    let mut pool: Vec<Meme> = config
        .memes
        .iter()
        .filter(|(name, description)| {
            !name.trim().is_empty() && !description.trim().is_empty() && dir.join(format!("{name}.png")).is_file()
        })
        .map(|(name, description)| Meme {
            name: name.trim().to_string(),
            description: description.trim().to_string(),
        })
        .collect();
    pool.sort_by(|a, b| a.name.cmp(&b.name));
    pool
}

/// 图片在页面里的相对地址（`pet.localhost` 的根就是应用数据目录，与动画同源）
pub fn asset_path(name: &str) -> String {
    // 名称可能含中文与空格，按路径段编码；`/` 会被编码掉，避免越出 memes 目录
    let encoded = name
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (byte as char).to_string(),
            _ => format!("%{byte:02X}"),
        })
        .collect::<String>();
    format!("{MEMES_DIR}/{encoded}.png")
}

/// 随机抽一张（碎碎念用）。
///
/// 不引随机数库：拿系统时间的纳秒低位对池长取模就够了——
/// 这里要的是"不要每次都同一张"，不是密码学随机。
pub fn pick_random(pool: &[Meme]) -> Option<&Meme> {
    if pool.is_empty() {
        return None;
    }
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.subsec_nanos() as usize)
        .unwrap_or(0);
    pool.get(nanos % pool.len())
}

/// 碎碎念的 user 文本：开配图时在基础指令后追加"这次会配一张图"的说明（逐字照抄上游）。
pub fn whisper_user_text(base: &str, meme: &Meme) -> String {
    format!(
        "{base}\n这次会配一张表情包一起显示，图的内容是：{}（{}）。\n请让这句话和这张图的情绪/场景自然契合，像是配合画面说出来的；不要描述画面本身。",
        meme.name, meme.description
    )
}

/// 对话配图指令（逐字照抄上游）：把清单交给模型，要求末尾用 `[图:名称]` 标记。
pub fn chat_image_instruction(pool: &[Meme]) -> String {
    let mut text = String::from(
        "\n\n[配图] 回复结尾可选附一张表情包给用户看，从下列清单里挑最贴合当前语境的：\n",
    );
    for meme in pool {
        text.push_str(&format!("- {}：{}\n", meme.name, meme.description));
    }
    text.push_str("\n挑中就在回复最后另起一行写 [图:名称]（名称原样照抄）；没有合适的就完全不要写这个标记。");
    text
}

/// 解析模型回复里的选图标记。
///
/// 返回 `(正文, 选中的名称)`：
/// - 命中的名字**必须在池内**，否则视为幻觉：不配图、正文原样返回（**绝不吞正文**）；
/// - 只认**最后一行**的标记（`[图:名称]` / `[图：名称]`，全角冒号也认），与上游的正则同一语义；
/// - 命中时把那一行从正文里剥掉（不留空行）。
pub fn split_choice(reply: &str, pool: &[Meme]) -> (String, Option<String>) {
    let trimmed_end = reply.trim_end();
    let Some(last_break) = trimmed_end.rfind('\n') else {
        // 只有一行：它本身就是标记行时，正文会是空的——这种情况按"不配图 + 原文"处理，
        // 否则用户会看到空白气泡。
        return match parse_marker_line(trimmed_end, pool) {
            Some(name) => (reply.trim().to_string(), Some(name)),
            None => (reply.trim().to_string(), None),
        };
    };
    let (head, tail) = trimmed_end.split_at(last_break);
    let tail = tail.trim_start_matches('\n');
    match parse_marker_line(tail, pool) {
        Some(name) => (head.trim_end().to_string(), Some(name)),
        None => (reply.trim().to_string(), None),
    }
}

/// 单行是否是合法的选图标记（且名字在池内）
fn parse_marker_line(line: &str, pool: &[Meme]) -> Option<String> {
    let line = line.trim();
    let inner = line.strip_prefix("[图")?;
    let inner = inner.strip_suffix(']')?;
    let inner = inner.trim_start_matches([':', '：']).trim();
    if inner.is_empty() {
        return None;
    }
    pool.iter().find(|meme| meme.name == inner).map(|meme| meme.name.clone())
}

/// 把配置里的表情包池整理成"名称 → 描述"（给设置页展示用）
pub fn describe(pool: &[Meme]) -> HashMap<String, String> {
    pool.iter().map(|meme| (meme.name.clone(), meme.description.clone())).collect()
}
