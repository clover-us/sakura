//! 对话记忆（M3）：`memory.json` 全量保存，只把最近 N 轮送进上下文。
//!
//! ## 结构（比上游少一层 `assetRoot`）
//!
//! 上游是 `{ <assetRoot>: { <petId>: { messages: [...] } } }`——多出来的那层是因为它同时服务
//! 多个素材根。本应用只有一个素材目录，所以直接按宠物 id 存：
//!
//! ```json
//! { "main": { "messages": [ { "role": "user", "content": "…", "ts": 1700000000000 } ] } }
//! ```
//!
//! ## 三条纪律（都来自上游踩过的坑）
//!
//! 1. **全量保存不删**：送进上下文的只是"最近 N 轮"，文件里一条都不丢——记忆是用户资产；
//! 2. **坏了先备份再重建**：`memory.json` 解析失败时备份成 `memory.json.bak-<时间戳>`
//!    （备份失败只告警），然后从空记忆开始，绝不让程序起不来；
//! 3. **写盘串行化**：碎碎念与对话可能同时来，一把锁 + 读改写，避免互相覆盖。

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::llm::ChatMessage;

/// 记忆文件名（位于应用数据目录）
pub const MEMORY_FILE_NAME: &str = "memory.json";

/// 一条历史消息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMemoryMessage {
    /// `user` / `assistant`
    pub role: String,
    pub content: String,
    /// 毫秒时间戳（前端展示顺序用）
    #[serde(default)]
    pub ts: u64,
}

/// 单只宠物的记忆
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct PetMemory {
    #[serde(default)]
    messages: Vec<ChatMemoryMessage>,
}

/// 整个记忆文件
type MemoryFile = std::collections::HashMap<String, PetMemory>;

/// 记忆存储（一把锁保证写盘串行）
pub struct MemoryStore {
    path: PathBuf,
    lock: Mutex<()>,
}

impl MemoryStore {
    pub fn new(app_data_dir: &Path) -> Self {
        MemoryStore { path: app_data_dir.join(MEMORY_FILE_NAME), lock: Mutex::new(()) }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 取某只宠物的全部记忆（文件不存在/坏掉 → 空）
    pub fn all(&self, pet_id: &str) -> Vec<ChatMemoryMessage> {
        let _guard = self.lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        self.load_file()
            .get(pet_id)
            .map(|memory| memory.messages.clone())
            .unwrap_or_default()
    }

    /// 取最近 `rounds` 轮（1 轮 = 1 问 1 答）→ 变成请求用的消息
    pub fn recent_messages(&self, pet_id: &str, rounds: u32) -> Vec<ChatMessage> {
        let all = self.all(pet_id);
        let take = (rounds as usize).saturating_mul(2);
        let start = all.len().saturating_sub(take);
        all[start..]
            .iter()
            .map(|message| ChatMessage {
                role: message.role.clone(),
                content: message.content.clone(),
            })
            .collect()
    }

    /// 追加一条（读-改-写，全程持锁）
    pub fn append(&self, pet_id: &str, role: &str, content: &str) -> Result<(), String> {
        let _guard = self.lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut file = self.load_file();
        let entry = file.entry(pet_id.to_string()).or_default();
        entry.messages.push(ChatMemoryMessage {
            role: role.to_string(),
            content: content.to_string(),
            ts: now_ms(),
        });
        self.save_file(&file)
    }

    /// 同时写入一轮问答（少一次写盘，也少一次"只写进去半轮"的机会）
    pub fn append_round(&self, pet_id: &str, user_text: &str, reply: &str) -> Result<(), String> {
        let _guard = self.lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut file = self.load_file();
        let entry = file.entry(pet_id.to_string()).or_default();
        let ts = now_ms();
        entry.messages.push(ChatMemoryMessage { role: "user".into(), content: user_text.to_string(), ts });
        entry.messages.push(ChatMemoryMessage { role: "assistant".into(), content: reply.to_string(), ts });
        self.save_file(&file)
    }

    /// 清空某只宠物的记忆
    pub fn clear(&self, pet_id: &str) -> Result<(), String> {
        let _guard = self.lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut file = self.load_file();
        file.remove(pet_id);
        self.save_file(&file)
    }

    /// 读文件；坏掉就备份并重建（**不返回错误**：记忆坏了不该让功能不可用）
    fn load_file(&self) -> MemoryFile {
        let Ok(raw) = std::fs::read_to_string(&self.path) else {
            return MemoryFile::new();
        };
        match serde_json::from_str::<MemoryFile>(&raw) {
            Ok(file) => file,
            Err(err) => {
                let stamp = now_ms();
                let backup = self.path.with_extension(format!("json.bak-{stamp}"));
                eprintln!(
                    "[whale-pet] 记忆文件解析失败（{err}），备份到 {} 后重建空记忆",
                    backup.display()
                );
                if let Err(copy_err) = std::fs::copy(&self.path, &backup) {
                    // 备份失败只告警：不能因为备份不了就不让用户继续用
                    eprintln!("[whale-pet] 记忆备份失败：{copy_err}");
                }
                MemoryFile::new()
            }
        }
    }

    fn save_file(&self, file: &MemoryFile) -> Result<(), String> {
        let text = serde_json::to_string_pretty(file)
            .map_err(|err| format!("序列化记忆失败：{err}"))?;
        std::fs::write(&self.path, text.as_bytes())
            .map_err(|err| format!("写入记忆失败 {}：{err}", self.path.display()))
    }
}

/// 当前毫秒时间戳
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}
