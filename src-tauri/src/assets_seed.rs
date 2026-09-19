//! 随包素材的释放（打包相关）。
//!
//! ## 为什么需要这一步
//!
//! 动画（106 条）、表情包（27 张）、气泡字体这些素材**默认不入库**（几十 MB，随时可从上游重新取），
//! 安装包只能打仓库里存在的文件，所以出"自带素材的安装包"的顺序是：
//!
//! ```text
//! scripts\import-animations.ps1 -Stage   → 把上游素材复制进仓库 assets/（暂存，不入库）
//! & 'D:\tools\Tauri\nodejs\tauri.cmd' build → 按 tauri.conf.json 的 bundle.resources 打进安装包
//! 首次运行（本模块）                      → 把随包素材**只补缺失地**释放到应用数据目录
//! ```
//!
//! ## 为什么是"释放到数据目录"而不是"直接从资源目录读"
//!
//! 数据目录是本项目素材的**唯一真相**：用户可以往里丢自己的动画、删掉不想要的、
//! 用 `scripts\import-animations.ps1` 重新导入。如果让运行时"有时读资源目录、有时读数据目录"，
//! 就会出现"我明明删了这条动画它还在"这种解释不清的状态。释放一次之后，两边行为完全一致。
//!
//! ## 三条纪律
//!
//! 1. **只补缺失**：目标文件已存在就跳过（用户改过的素材永远不被覆盖）；
//! 2. **失败不致命**：素材释放失败只记一行日志——应用照样能起（用户还能手动 import）；
//! 3. **开发构建下静默**：`cargo run` 时资源目录里通常没有 assets，这时不刷屏。

use std::path::{Path, PathBuf};

/// 随包资源里的素材根目录名（与 `tauri.conf.json` 的 `bundle.resources` 目标一致）
const RESOURCE_ASSETS_SUBDIR: &str = "assets";

/// 会随包释放的素材子目录（与数据目录同名）
pub const ASSET_SUBDIRS: [&str; 4] = ["webm", "memes", "pic", "fonts"];

/// 一次释放的结果（日志与自检用）
#[derive(Debug, Default, Clone)]
pub struct SeedReport {
    /// 新释放的文件数
    pub copied: usize,
    /// 因为已存在而跳过的文件数
    pub skipped: usize,
    /// 实际使用的资源目录（`None` = 没找到随包素材，例如开发构建）
    pub source: Option<PathBuf>,
}

impl SeedReport {
    /// 一行可读摘要
    pub fn summary(&self) -> String {
        match &self.source {
            Some(dir) => format!(
                "随包素材：释放 {} 个、已存在 {} 个（来源 {}）",
                self.copied,
                self.skipped,
                dir.display()
            ),
            None => "随包素材：资源目录里没有素材（开发构建或精简包），跳过".to_string(),
        }
    }
}

/// 定位随包素材目录。
///
/// 两种可能的位置都要认：
/// - `<资源目录>/assets/...`（`bundle.resources` 用映射形式时的目标路径）；
/// - `<资源目录>/_up_/assets/...`（Tauri 打包"应用目录之外"的相对路径时加的 `_up_` 前缀）。
///
/// 认不出来就返回 `None`（不是错误：开发构建本来就没有）。
pub fn locate(resource_dir: &Path) -> Option<PathBuf> {
    for candidate in [
        resource_dir.join(RESOURCE_ASSETS_SUBDIR),
        resource_dir.join("_up_").join(RESOURCE_ASSETS_SUBDIR),
    ] {
        if ASSET_SUBDIRS.iter().any(|dir| candidate.join(dir).is_dir()) {
            return Some(candidate);
        }
    }
    None
}

/// 把随包素材释放到数据目录（只补缺失）
pub fn seed(resource_dir: &Path, app_data_dir: &Path) -> SeedReport {
    let mut report = SeedReport::default();
    let Some(source) = locate(resource_dir) else {
        return report;
    };
    report.source = Some(source.clone());

    for subdir in ASSET_SUBDIRS {
        let from = source.join(subdir);
        if !from.is_dir() {
            continue;
        }
        let to = app_data_dir.join(subdir);
        if let Err(err) = std::fs::create_dir_all(&to) {
            eprintln!("[whale-pet] 创建素材目录失败 {}：{err}", to.display());
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&from) else {
            eprintln!("[whale-pet] 读取随包素材失败 {}", from.display());
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let Some(name) = path.file_name() else { continue };
            let target = to.join(name);
            // 只补缺失：用户改过/替换过的素材永远不被覆盖
            if target.exists() {
                report.skipped += 1;
                continue;
            }
            match std::fs::copy(&path, &target) {
                Ok(_) => report.copied += 1,
                Err(err) => eprintln!("[whale-pet] 释放素材失败 {}：{err}", target.display()),
            }
        }
    }
    report
}
