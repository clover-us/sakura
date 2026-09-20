//! API key 的本地加密存储（Windows DPAPI）。
//!
//! ## 为什么密钥不能放配置文件里
//!
//! `config.jsonc` 是**用户会备份、会贴给别人看、会放进 git** 的文件（本项目自己也鼓励手改它）。
//! 把 `sk-...` 明文写进去，等于把账单交给别人。所以密钥单独存一个文件，
//! 而且**落盘前先用系统级密钥加密**。
//!
//! ## 为什么用 DPAPI 而不是 keyring crate
//!
//! 计划里原来写的是 `keyring`（Windows 凭据管理器）。实际权衡后改用 **DPAPI 直接调 Win32**
//! （`CryptProtectData` / `CryptUnprotectData`）：
//!   - 效果等价：同样绑定"当前 Windows 用户"，换用户/换机器都解不开；
//!   - 代价为零：不需要新依赖（本项目对依赖的态度是"能自己写 40 行就不引一个 crate"）；
//!   - 缺点也诚实记下来：密钥存在**我们自己的文件**里（`llm-key.bin`），
//!     而不是系统凭据管理器里，所以"用户能在凭据管理器里看到/删除它"这件事做不到。
//!
//! ## 两条纪律
//!
//! 1. **日志里永远不出现明文**：要打日志用 [`mask`]（只留前 3 位与后 4 位）；
//! 2. 密钥文件解不开（比如换了 Windows 账户）时不崩溃、不静默：返回可读错误，
//!    让用户重新填一次。

use std::ffi::c_void;
use std::path::{Path, PathBuf};

/// 密钥文件名（位于应用数据目录）
pub const KEY_FILE_NAME: &str = "llm-key.bin";

/// 只留头尾、中间打码：唯一允许出现在日志里的形态
pub fn mask(key: &str) -> String {
    let key = key.trim();
    let chars: Vec<char> = key.chars().collect();
    if chars.len() <= 8 {
        return "****".to_string();
    }
    let head: String = chars.iter().take(3).collect();
    let tail: String = chars.iter().skip(chars.len() - 4).collect();
    format!("{head}…{tail}（共 {} 字符）", chars.len())
}

/// 密钥存储（一个文件 + 系统级加密）
pub struct SecretStore {
    path: PathBuf,
}

impl SecretStore {
    /// 绑定到应用数据目录下的 `llm-key.bin`
    pub fn new(app_data_dir: &Path) -> Self {
        SecretStore { path: app_data_dir.join(KEY_FILE_NAME) }
    }

    /// 密钥文件路径（设置窗口要显示给用户看）
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 是否已经存过密钥
    pub fn exists(&self) -> bool {
        self.path.is_file()
    }

    /// 加密保存（覆盖旧的）
    pub fn save(&self, plaintext: &str) -> Result<(), String> {
        let trimmed = plaintext.trim();
        if trimmed.is_empty() {
            return Err("密钥不能为空".to_string());
        }
        let blob = protect(trimmed.as_bytes())?;
        std::fs::write(&self.path, blob)
            .map_err(|e| format!("写入密钥文件失败 {}：{e}", self.path.display()))?;
        eprintln!(
            "[whale-pet] 已保存 API key（{}，DPAPI 加密后 {} 字节）",
            mask(trimmed),
            std::fs::metadata(&self.path).map(|m| m.len()).unwrap_or(0)
        );
        Ok(())
    }

    /// 读取并解密；文件不存在返回 `Ok(None)`
    pub fn load(&self) -> Result<Option<String>, String> {
        if !self.path.is_file() {
            return Ok(None);
        }
        let blob = std::fs::read(&self.path)
            .map_err(|e| format!("读取密钥文件失败 {}：{e}", self.path.display()))?;
        let plain = unprotect(&blob)?;
        let text = String::from_utf8(plain).map_err(|_| "密钥文件内容不是合法 UTF-8".to_string())?;
        if text.trim().is_empty() {
            return Ok(None);
        }
        Ok(Some(text))
    }

    /// 删除密钥（设置窗口的"清除"按钮）
    pub fn clear(&self) -> Result<(), String> {
        if self.path.is_file() {
            std::fs::remove_file(&self.path)
                .map_err(|e| format!("删除密钥文件失败 {}：{e}", self.path.display()))?;
            eprintln!("[whale-pet] 已清除 API key");
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
//  Win32 DPAPI（只声明用到的两个函数，不引入 windows-sys）
// ---------------------------------------------------------------------------

#[repr(C)]
struct DataBlob {
    cb_data: u32,
    pb_data: *mut u8,
}

#[cfg(windows)]
mod ffi {
    use super::{c_void, DataBlob};

    /// 不在界面上弹任何提示（我们的调用都发生在后台线程上）
    pub const CRYPTPROTECT_UI_FORBIDDEN: u32 = 0x1;

    #[link(name = "crypt32")]
    extern "system" {
        pub fn CryptProtectData(
            data_in: *const DataBlob,
            data_descr: *const u16,
            optional_entropy: *const DataBlob,
            reserved: *const c_void,
            prompt: *const c_void,
            flags: u32,
            data_out: *mut DataBlob,
        ) -> i32;
        pub fn CryptUnprotectData(
            data_in: *const DataBlob,
            data_descr_out: *mut *mut u16,
            optional_entropy: *const DataBlob,
            reserved: *const c_void,
            prompt: *const c_void,
            flags: u32,
            data_out: *mut DataBlob,
        ) -> i32;
    }

    #[link(name = "kernel32")]
    extern "system" {
        pub fn LocalFree(mem: *mut c_void) -> *mut c_void;
    }
}

/// 用当前 Windows 用户的密钥加密
#[cfg(windows)]
fn protect(plain: &[u8]) -> Result<Vec<u8>, String> {
    unsafe {
        let mut input = DataBlob { cb_data: plain.len() as u32, pb_data: plain.as_ptr() as *mut u8 };
        let mut output = DataBlob { cb_data: 0, pb_data: std::ptr::null_mut() };
        let ok = ffi::CryptProtectData(
            &mut input,
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            ffi::CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        );
        if ok == 0 {
            return Err(format!("DPAPI 加密失败（GetLastError={}）", std::io::Error::last_os_error()));
        }
        let bytes = std::slice::from_raw_parts(output.pb_data, output.cb_data as usize).to_vec();
        ffi::LocalFree(output.pb_data as *mut c_void);
        Ok(bytes)
    }
}

/// 解密（只能由加密它的同一个 Windows 用户解开）
#[cfg(windows)]
fn unprotect(blob: &[u8]) -> Result<Vec<u8>, String> {
    if blob.is_empty() {
        return Err("密钥文件是空的".to_string());
    }
    unsafe {
        let mut input = DataBlob { cb_data: blob.len() as u32, pb_data: blob.as_ptr() as *mut u8 };
        let mut output = DataBlob { cb_data: 0, pb_data: std::ptr::null_mut() };
        let ok = ffi::CryptUnprotectData(
            &mut input,
            std::ptr::null_mut(),
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            ffi::CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        );
        if ok == 0 {
            return Err(
                "密钥解密失败：它由另一个 Windows 账户加密（DPAPI 绑定用户）。请在设置里重新填写 API key。"
                    .to_string(),
            );
        }
        let bytes = std::slice::from_raw_parts(output.pb_data, output.cb_data as usize).to_vec();
        ffi::LocalFree(output.pb_data as *mut c_void);
        Ok(bytes)
    }
}

/// 非 Windows：暂时不做加密存储（本项目当前只支持 Windows，见 `docs/DEVELOPMENT.md` 的"仅 Windows"）
#[cfg(not(windows))]
fn protect(_plain: &[u8]) -> Result<Vec<u8>, String> {
    Err("当前平台未实现密钥加密存储（本项目目前只支持 Windows）".to_string())
}

#[cfg(not(windows))]
fn unprotect(_blob: &[u8]) -> Result<Vec<u8>, String> {
    Err("当前平台未实现密钥解密（本项目目前只支持 Windows）".to_string())
}
