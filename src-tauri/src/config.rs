//! 桌宠配置：文件结构、加载、首次运行落默认配置。
//!
//! 设计原则（对齐上游 dsh-pet）：
//!   - 配置是**唯一真相**：读到的值要么合法要么报错，绝不"读失败就当默认值"静默兜底；
//!   - 解析失败的报错必须能被用户看到（窗口错误条 + 控制台），因为最常见的故障就是
//!     用户手改 JSON 时写错一个逗号；
//!   - 配置目录固定在应用数据目录（`%APPDATA%\whale-pet-desktop`），不跟随工作目录，
//!     双击 exe 与命令行启动的行为完全一致。
//!
//! 与上游的关系：上游把「宠物列表 / 动画池 / 权重 / 物理参数」放在一个 JSONC 里由 host 合并
//! 并校验（`dsh-pet/src/host/config.ts`）。M0 只保留桌面端真正需要的子集：
//! 宠物外观与位置、动画素材、物理参数；动画池/权重/事件将在 M1 接入。

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::model::PhysicsParams;

/// 首次运行时写入的默认配置（`include_str!` 编译进二进制，不依赖当前工作目录）。
///
/// 放在仓库根的 `config/default-config.jsonc`：仓库里能直接看到并编辑，
/// 但运行时读的是应用数据目录里那一份（见 [`ensure_default_config`]）。
const DEFAULT_CONFIG_TEMPLATE: &str = include_str!("../../config/default-config.jsonc");

/// 配置文件名
pub const CONFIG_FILE_NAME: &str = "config.jsonc";

/// 保存配置前的备份文件名（每次保存前把上一版拷到这里，只保留最近一次）
pub const CONFIG_BACKUP_FILE_NAME: &str = "config.jsonc.bak";

/// 写盘用的临时文件名：先写它再原子改名，避免热重载读到"写了一半"的文件
const CONFIG_TMP_FILE_NAME: &str = "config.jsonc.tmp";

/// 动画素材目录名（位于应用数据目录下，用户可直接放自己的透明动画）
pub const WEBM_DIR_NAME: &str = "webm";

/// 随包内置的示例动画（`include_bytes!` 编译进二进制）。
///
/// 为什么内置示例素材而不是留空目录：
///   用户第一次跑起来时如果 `webm/` 是空的，宠物窗口里什么都没有——这会被误判成"程序坏了"。
///   内置一条待机 + 一条点击回应，装完即可看到活的宠物；完整素材集（上游 106 条动画）
///   用仓库里的 `scripts/import-animations.ps1` 一次性导入。
const SAMPLE_ANIMATIONS: &[(&str, &[u8])] = &[
    ("待机呼吸休闲.webm", include_bytes!("../../assets/webm/待机呼吸休闲.webm")),
    ("点击回应-元气挥手.webm", include_bytes!("../../assets/webm/点击回应-元气挥手.webm")),
];

/// 宠物位置：角落 + 边距（与上游 `pets[].position` 同构）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PositionConfig {
    /// 基准角落
    pub corner: Corner,
    /// 距屏幕左右边缘的边距（像素）
    #[serde(default = "default_margin")]
    pub margin_x: i32,
    /// 距屏幕上下边缘的边距（像素）
    #[serde(default = "default_margin")]
    pub margin_y: i32,
}

/// 基准角落（与上游同构）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Corner {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

/// 单只宠物配置
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PetEntry {
    /// 唯一标识（窗口标签由它派生，如 `pet-main`）
    pub id: String,
    /// 显示名（气泡/调试用）
    pub name: String,
    /// 包围盒宽度（像素）；高度按 9/16 推出
    pub size: f64,
    /// 待机动画文件名（相对 `webm/`，含扩展名）；留空 = 自动挑第一个可用动画
    #[serde(default)]
    pub idle: String,
    /// 点击回应动画文件名；留空 = 点击只做 Q 弹、不切动画
    #[serde(default)]
    pub click: String,
    /// 初始位置
    pub position: PositionConfig,
    /// **单独覆盖**动画池（留空/不写 = 跟随全局 `animations`）
    ///
    /// 为什么允许每只宠物一套：桌宠的"行为"本质上是**这只宠物**的属性——
    /// 小猫该有小猫的动作池、鲸鱼该有鲸鱼的；全局那份改叫"默认值"，
    /// 新增宠物不用重新配一遍。语义是"整体覆盖"而不是"逐字段合并"：
    /// 合并看起来更聪明，但会出现"我明明删掉了某个动作，它还在播"这种查不清的情况
    /// （删除与"未设置"在合并语义下无法区分），所以这里刻意选整段覆盖。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub animations: Option<AnimationsConfig>,
    /// **单独覆盖**动画链权重（留空 = 跟随全局 `animationWeights`）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub animation_weights: Option<AnimationWeights>,
}

/// 移动动作：一个动作名 + 可选覆盖参数（未写字段取 moves.default）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MoveSpec {
    /// 动画名（对应素材目录里的文件）
    pub name: String,
    /// 覆盖参数：minDist/maxDist/margin/leadSec/tailSec 的任意子集
    ///
    /// `skip_serializing_if`：设置窗口重写配置时不写 `"params": null`（噪音），
    /// 缺字段由上面的 `default` 兜住，读回来仍是 `None`，语义不变。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Map<String, serde_json::Value>>,
}

/// 移动池：默认参数 + 动作列表
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MovesConfig {
    /// 每个动作都先取这套参数（字段：minDist/maxDist/margin/leadSec/tailSec）
    pub default: serde_json::Map<String, serde_json::Value>,
    /// 可选的移动动作（每一项可覆盖 default 的部分字段）
    pub actions: Vec<MoveSpec>,
}

/// 随机动作分类（带文字、镜像会颠倒的分类标 `noMirror: true`）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CategoryConfig {
    /// 分类 id（菜单里作为子菜单标题）
    pub id: String,
    /// 出现权重（各分类 weight 之和应与 animationWeights 的余量相等）
    pub weight: f64,
    /// 该分类的动作名列表
    pub actions: Vec<String>,
    /// 镜像会颠倒（带文字）：宠物朝右时跳过该分类
    #[serde(default)]
    pub no_mirror: bool,
}

/// 事件档位：单个动画名（固定播放）或候选数组（触发时档内随机抽 1）
///
/// 用 `serde_json::Value` 承载：两种形状都能进，具体语义由前端 shared 的
/// `pickSlot` 解释（与上游完全一致，避免在 Rust 侧重复实现一遍规则）。
pub type EventSlot = serde_json::Value;

/// 动画池（与上游 `config.jsonc` 的 animations 段同构）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnimationsConfig {
    /// 待机池（等概率抽）
    pub idle: Vec<String>,
    /// 转向池（每一项都必须是"播完会翻转朝向"的动画）
    pub turn: Vec<String>,
    /// 拖拽池（"被无形抓起悬空"的姿势）
    pub drag: Vec<String>,
    /// 点击回应池
    pub clicks: Vec<String>,
    /// 移动池
    pub moves: MovesConfig,
    /// 随机动作分类
    pub categories: Vec<CategoryConfig>,
    /// 事件动画（不进随机链，只由代码显式触发）：事件名 → 档位数组
    #[serde(default)]
    pub events: std::collections::HashMap<String, Vec<EventSlot>>,
}

/// 动画链顶层权重（idle/turn/move；剩余概率归随机动作分类）
///
/// 注意：`move` 是 Rust 关键字，字段名只能写成 `move_`，再用 `#[serde(rename)]`
/// 映射回 JSON 里的 `move`。
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct AnimationWeights {
    pub idle: f64,
    pub turn: f64,
    #[serde(rename = "move")]
    pub move_: f64,
}

/// 配置文件根结构
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppConfig {
    /// 配置版本号（将来做迁移用；M0 校验为 1）
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    /// 拖拽抛掷物理参数
    pub physics: PhysicsParams,
    /// 宠物列表（至少一只）
    pub pets: Vec<PetEntry>,
    /// 动画池（待机/转向/拖拽/点击/移动/随机分类/事件）。
    ///
    /// 允许缺失：旧版配置（0.x 早期）没有这一段，缺失时回落内置默认（见 `migrate_config`）。
    /// 这保证"用户不重新生成配置也能启动"，而不是一升级就打不开。
    #[serde(default = "default_animations")]
    pub animations: AnimationsConfig,
    /// 动画链顶层权重（缺失时回落 10/5/5，与上游默认一致）
    #[serde(default = "default_animation_weights")]
    pub animation_weights: AnimationWeights,
}

impl AppConfig {
    /// 校验配置并返回可读错误信息（不做任何"自动修正"）。
    ///
    /// 之所以把校验与读取分开：读取负责 IO 与解析错误（带文件路径与行号），
    /// 校验负责语义错误（数量、取值范围），两类错误的排查方式完全不同。
    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != 1 {
            return Err(format!(
                "schemaVersion 只支持 1，当前为 {}（请对照 config.jsonc 模板修正）",
                self.schema_version
            ));
        }
        if self.pets.is_empty() {
            return Err("pets 不能为空：至少配置一只宠物".to_string());
        }
        for (index, pet) in self.pets.iter().enumerate() {
            let where_ = format!("pets[{index}](id={})", pet.id);
            if pet.id.trim().is_empty() {
                return Err(format!("{where_} 的 id 不能为空"));
            }
            if pet.id.contains(['/', '\\', ':', '*', '?', '"', '<', '>', '|']) {
                return Err(format!("{where_} 的 id 含非法字符（不能用作窗口标签）：{}", pet.id));
            }
            // 尺寸下限保护：小于 32px 时命中区不足 12px，实际不可点击
            if !(pet.size.is_finite() && pet.size >= 64.0 && pet.size <= 2048.0) {
                return Err(format!("{where_} 的 size 必须在 64~2048 之间，当前为 {}", pet.size));
            }
        }
        self.physics.validate()?;
        self.validate_animations()?;
        Ok(())
    }

    /// 校验动画池的自洽性（**不需要素材清单**就能判定的部分）。
    ///
    /// 全局那份与每只宠物的覆盖都要过一遍：覆盖段写坏了同样会让应用起不来，
    /// 而错误信息里必须能看出"是全局还是哪只宠物"的池子有问题。
    pub fn validate_animations(&self) -> Result<(), String> {
        validate_animation_pool(&self.animations, &self.animation_weights, "全局")?;
        for (index, pet) in self.pets.iter().enumerate() {
            let anim = self.effective_animations(pet);
            let weights = self.effective_weights(pet);
            // 只有在"这只宠物确实自定义了"时才单独校验：否则同一份全局池会被重复报错
            if pet.animations.is_some() || pet.animation_weights.is_some() {
                let where_ = format!("宠物 {}（pets[{index}]）", if pet.name.trim().is_empty() { pet.id.as_str() } else { pet.name.as_str() });
                validate_animation_pool(&anim, &weights, &where_)?;
            }
        }
        Ok(())
    }

    /// 取某只宠物**实际生效**的动画池（没写覆盖就用全局）
    pub fn effective_animations(&self, pet: &PetEntry) -> AnimationsConfig {
        pet.animations.clone().unwrap_or_else(|| self.animations.clone())
    }

    /// 取某只宠物**实际生效**的动画链权重（没写覆盖就用全局）
    pub fn effective_weights(&self, pet: &PetEntry) -> AnimationWeights {
        pet.animation_weights.unwrap_or(self.animation_weights)
    }

    /// 这只宠物是否自定义了行为（覆盖了动画池或权重）——UI 与日志都要用
    pub fn pet_has_custom_behaviour(pet: &PetEntry) -> bool {
        pet.animations.is_some() || pet.animation_weights.is_some()
    }
}

/// 校验一份动画池 + 权重；`where_` 是给用户看的作用域（"全局" / "宠物 小鲸鱼"）
fn validate_animation_pool(
    anim: &AnimationsConfig,
    weights: &AnimationWeights,
    where_: &str,
) -> Result<(), String> {
    if anim.idle.is_empty() {
        return Err(format!("{where_}：animations.idle 不能为空（至少需要一个待机动画）"));
    }
    if anim.clicks.is_empty() {
        return Err(format!("{where_}：animations.clicks 不能为空（至少需要一个点击回应动画）"));
    }
    for category in &anim.categories {
        if category.id.trim().is_empty() {
            return Err(format!("{where_}：animations.categories[].id 不能为空"));
        }
        if !(category.weight.is_finite() && category.weight >= 0.0) {
            return Err(format!("{where_}：分类 {} 的 weight 必须为非负数", category.id));
        }
        if category.actions.is_empty() {
            return Err(format!("{where_}：分类 {} 的 actions 不能为空", category.id));
        }
    }
    // 顶层权重与分类权重的关系：三档占比 + 分类占比必须能覆盖 100
    // （不足 100 时剩余概率会被 rollKind 归入 action，语义上不算错误，但通常是配错了）
    let top = weights.idle + weights.turn + weights.move_;
    if !(top.is_finite() && (0.0..=100.0).contains(&top)) {
        return Err(format!(
            "{where_}：animationWeights 的 idle+turn+move 必须在 0~100 之间，当前为 {top}"
        ));
    }
    for (key, slots) in &anim.events {
        if slots.is_empty() {
            return Err(format!("{where_}：animations.events.{key} 不能为空数组"));
        }
    }
    Ok(())
}

/// 配置目录：`<应用数据目录>/`（由调用方传入，便于测试时注入临时目录）
pub fn config_file_path(app_data_dir: &Path) -> PathBuf {
    app_data_dir.join(CONFIG_FILE_NAME)
}

/// 动画素材目录：`<应用数据目录>/webm/`
pub fn webm_dir(app_data_dir: &Path) -> PathBuf {
    app_data_dir.join(WEBM_DIR_NAME)
}

/// 首次运行时把内置模板写到应用数据目录（已存在则原样保留，**绝不覆盖用户改动**）。
///
/// 返回配置文件的最终路径。
pub fn ensure_default_config(app_data_dir: &Path) -> Result<PathBuf, String> {
    let path = config_file_path(app_data_dir);
    if path.exists() {
        return Ok(path);
    }
    fs::create_dir_all(app_data_dir)
        .map_err(|e| format!("创建配置目录失败 {}：{e}", app_data_dir.display()))?;
    fs::write(&path, DEFAULT_CONFIG_TEMPLATE)
        .map_err(|e| format!("写入默认配置失败 {}：{e}", path.display()))?;
    Ok(path)
}

/// 把内置示例动画释放到 `webm/` 目录。
///
/// 规则：**只在文件不存在时写入**——用户自己放的同名动画（哪怕改过）永远不会被覆盖，
/// 删掉文件后下次启动会重新释放一份示例。
///
/// 返回实际释放的文件数（0 表示目录里已经有同名文件，无需处理）。
pub fn ensure_sample_animations(app_data_dir: &Path) -> Result<usize, String> {
    let dir = webm_dir(app_data_dir);
    fs::create_dir_all(&dir).map_err(|e| format!("创建素材目录失败 {}：{e}", dir.display()))?;

    let mut written = 0usize;
    for (name, bytes) in SAMPLE_ANIMATIONS {
        let target = dir.join(name);
        if target.exists() {
            continue;
        }
        fs::write(&target, bytes).map_err(|e| format!("释放示例动画失败 {}：{e}", target.display()))?;
        written += 1;
    }
    Ok(written)
}

/// 校验动画池里的名字都能落到实际素材文件上；返回**警告列表**（空 = 全部命中）。
///
/// 为什么是警告而不是错误：素材是用户自己导入的（`scripts/import-animations.ps1`），
/// 没导入时池里的名字自然查不到。这时应该把"哪些名字查不到"明确列出来，
/// 而不是让应用启动失败——否则用户第一条反馈会是"程序起不来"而不是"我还没导素材"。
pub fn validate_animation_assets(config: &AppConfig, available: &[String]) -> Vec<String> {
    let mut warnings = Vec::new();
    // 用普通函数而不是闭包：闭包会独占地捕获 `warnings`，
    // 导致嵌套遍历（events 的档位是数组）时出现"二次可变借用"编译错误。
    fn check(warnings: &mut Vec<String>, name: &str, origin: &str, available: &[String]) {
        if !animation_file_exists(name, available) {
            warnings.push(format!("{origin} 引用的动画找不到素材：{name}"));
        }
    }

    /// 一份池子逐项查素材
    fn check_pool(warnings: &mut Vec<String>, animations: &AnimationsConfig, scope: &str, available: &[String]) {
        let check = |warnings: &mut Vec<String>, name: &str, origin: &str| {
            check(warnings, name, origin, available);
        };
        for name in &animations.idle {
            check(warnings, name, &format!("{scope} animations.idle"));
        }
        for name in &animations.turn {
            check(warnings, name, &format!("{scope} animations.turn"));
        }
        for name in &animations.drag {
            check(warnings, name, &format!("{scope} animations.drag"));
        }
        for name in &animations.clicks {
            check(warnings, name, &format!("{scope} animations.clicks"));
        }
        for spec in &animations.moves.actions {
            check(warnings, &spec.name, &format!("{scope} animations.moves.actions"));
        }
        for category in &animations.categories {
            let origin = format!("{scope} 分类 {}", category.id);
            for name in &category.actions {
                check(warnings, name, &origin);
            }
        }
        for (event, slots) in &animations.events {
            let origin = format!("{scope} events.{event}");
            for slot in slots {
                match slot {
                    serde_json::Value::String(name) => check(warnings, name, &origin),
                    serde_json::Value::Array(names) => {
                        for name in names {
                            if let Some(text) = name.as_str() {
                                check(warnings, text, &origin);
                            }
                        }
                    }
                    _ => warnings.push(format!("{origin} 的档位取值既不是字符串也不是数组")),
                }
            }
        }
    }

    // 全局池（**只在有宠物真的跟随全局时才查**，否则同一份池会被查 N 遍、警告刷屏）
    let anyone_uses_global = config.pets.iter().any(|pet| !AppConfig::pet_has_custom_behaviour(pet));
    if anyone_uses_global {
        check_pool(&mut warnings, &config.animations, "全局", available);
    }
    // 每只宠物的覆盖池：错误信息里带上宠物标识，用户一眼知道该改哪儿
    for pet in &config.pets {
        if !AppConfig::pet_has_custom_behaviour(pet) {
            continue;
        }
        let label = if pet.name.trim().is_empty() { pet.id.clone() } else { pet.name.clone() };
        let scope = format!("宠物 {label} 自带的");
        check_pool(&mut warnings, &config.effective_animations(pet), &scope, available);
    }
    warnings
}

/// 动画名是否能在素材清单里找到（带/不带扩展名都认，`.webm` 与 `.mov` 都认）。
///
/// 前端最终拼 URL 时用的也是这套匹配规则（素材清单以"不含扩展名的基名"下发）。
pub fn animation_file_exists(name: &str, available: &[String]) -> bool {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return false;
    }
    // 1) 配置里写了带扩展名的完整文件名
    if available.iter().any(|file| file == trimmed) {
        return true;
    }
    // 2) 只写了基名（上游配置的写法）：补两种扩展名各试一次
    let lower = trimmed.to_ascii_lowercase();
    if lower.ends_with(".webm") || lower.ends_with(".mov") {
        return false;
    }
    available
        .iter()
        .any(|file| file.eq_ignore_ascii_case(&format!("{trimmed}.webm")) || file.eq_ignore_ascii_case(&format!("{trimmed}.mov")))
}

/// 完整加载流程：读文件 → 去 BOM → 剥注释 → 解析 → 校验（含动画池）
pub fn load_config(path: &Path) -> Result<AppConfig, String> {
    let raw = fs::read_to_string(path).map_err(|e| format!("读取配置失败 {}：{e}", path.display()))?;
    // 容错 UTF-8 BOM：Windows 记事本 / PowerShell 的 `Set-Content -Encoding UTF8`
    // 都会给文件开头加 BOM，而 BOM 不是合法 JSON 起始字符——不加这一步，
    // "用记事本改一下配置"就会让应用起不来（实测踩过）。
    let raw = raw.strip_prefix('\u{feff}').unwrap_or(&raw);
    let without_comments = strip_jsonc_comments(raw);
    let mut config: AppConfig = serde_json::from_str(&without_comments)
        .map_err(|e| format!("解析配置失败 {}（第 {} 行第 {} 列）：{e}", path.display(), e.line(), e.column()))?;
    migrate_config(&mut config);
    config.validate()?;
    Ok(config)
}

/// 把配置序列化成**按键名排序**的规范 JSON。
///
/// 用途只有一个，但很关键：判断"磁盘上的配置"与"当前生效的配置"是不是同一份语义。
///
/// 为什么不直接比 `serde_json::to_string`：配置里的 `animations.events` 是 `HashMap`，
/// 它的序列化顺序**每个进程、每次都不一样**（随机种子）。直接比字符串的话，
/// "读到同一份配置"永远判为"变了"——热重载会在启动时白重建一次窗口，
/// 而这恰好会踩到"拆窗瞬间窗口数归零 → Tauri 认为最后一个窗口关了 → 应用退出"。
pub fn canonical_json(config: &AppConfig) -> String {
    /// 递归按键名排序（数组保持顺序：动画池的顺序有语义）
    fn sort(value: serde_json::Value) -> serde_json::Value {
        match value {
            serde_json::Value::Object(map) => {
                let mut keys: Vec<String> = map.keys().cloned().collect();
                keys.sort();
                let mut sorted = serde_json::Map::new();
                for key in keys {
                    if let Some(inner) = map.get(&key) {
                        sorted.insert(key.clone(), sort(inner.clone()));
                    }
                }
                serde_json::Value::Object(sorted)
            }
            serde_json::Value::Array(items) => {
                serde_json::Value::Array(items.into_iter().map(sort).collect())
            }
            other => other,
        }
    }

    match serde_json::to_value(config) {
        Ok(value) => serde_json::to_string(&sort(value)).unwrap_or_default(),
        // 理论上到不了这里（AppConfig 都是可序列化的普通结构）
        Err(err) => {
            eprintln!("[whale-pet] 配置序列化失败（规范 JSON）：{err}");
            String::new()
        }
    }
}

/// 保存配置的结果（回给设置窗口，用来告诉用户"写到哪了、备份在哪"）
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveConfigReport {
    /// 配置文件路径
    pub path: String,
    /// 备份文件路径（原来没有配置文件时为 `None`）
    pub backup: Option<String>,
    /// 写入的字节数
    pub bytes: usize,
}

/// 设置窗口保存配置时写在文件开头的说明。
///
/// 为什么要写这一段：JSON 本身不存注释，而内置模板（`config/default-config.jsonc`）
/// 是**带大量注释**的。设置窗口把配置规范化重写之后，那些注释会消失——
/// 与其让用户某天打开文件发现"注释没了、不知道字段啥意思"，不如在文件开头说清楚：
/// 这份是谁写的、上一版备份在哪、手写注释会被下一次保存替换、怎么查字段含义。
const SAVE_HEADER: &str = r#"// ============================================================================
//  这份 config.jsonc 由「设置窗口」写入（规范化 JSON；JSON 不存注释，故本段说明之后
//  没有字段级注释）。
// ----------------------------------------------------------------------------
//   - 保存前的上一版已备份为同目录下的 config.jsonc.bak（只保留最近一次）；
//   - 字段含义速查：pets 宠物列表 / physics 抛掷物理 / animations 动画池 /
//     animationWeights 动画链权重（idle/turn/move）；schemaVersion 目前只支持 1。
//     带完整注释的模板见仓库 config/default-config.jsonc。
//   - **手写编辑本文件同样生效**：应用会监听文件变化并自动热重载（约 1 秒内），
//     无需重启；改坏了（解析/校验失败）会保留当前配置并在日志里给出原因；
//   - 想保住手写注释：把改动写在别处、需要时手工合并，或直接改本文件后不再从
//     设置窗口保存（保存会整体重写）。
// ============================================================================
"#;

/// 保存配置到 `<应用数据目录>/config.jsonc`。
///
/// 三步，顺序不能变：
///   1. **先校验**：宁可不写盘，也不要把一份过不了校验的配置落下去
///      （否则下次启动直接起不来，而用户刚刚只是在界面上点了一下"保存"）；
///   2. 备份上一版到 `config.jsonc.bak`（存在才备份）；
///   3. 先写临时文件再原子改名——热重载线程每秒都在看这个文件，
///      直接覆写会让它有机会读到"写了一半"的 JSON。
pub fn save_config(app_data_dir: &Path, config: &AppConfig) -> Result<SaveConfigReport, String> {
    config.validate()?;

    let path = config_file_path(app_data_dir);
    let backup = if path.is_file() {
        let backup_path = app_data_dir.join(CONFIG_BACKUP_FILE_NAME);
        fs::copy(&path, &backup_path)
            .map_err(|e| format!("备份配置失败 {}：{e}", backup_path.display()))?;
        Some(backup_path)
    } else {
        None
    };

    let body = serde_json::to_string_pretty(config).map_err(|e| format!("序列化配置失败：{e}"))?;
    let text = format!("{SAVE_HEADER}{body}\n");

    let tmp_path = app_data_dir.join(CONFIG_TMP_FILE_NAME);
    fs::write(&tmp_path, text.as_bytes())
        .map_err(|e| format!("写入临时配置失败 {}：{e}", tmp_path.display()))?;
    fs::rename(&tmp_path, &path).map_err(|e| {
        // 改名失败时留下 tmp 会让用户困惑，顺手清掉（清不掉也不影响报错信息）
        let _ = fs::remove_file(&tmp_path);
        format!("替换配置文件失败 {}：{e}", path.display())
    })?;

    Ok(SaveConfigReport {
        path: path.display().to_string(),
        backup: backup.map(|p| p.display().to_string()),
        bytes: text.len(),
    })
}

/// 每只宠物的待机 / 点击回应动画（**从动画池派生**，不需要逐只写配置）。
///
/// 设计取舍：上游把这两个名字写在 `pets[]` 里（历史原因），但它们本质上是"池里的第一个"。
/// 本项目改为从池派生：
///   - 用户不必在 pets 里重复写动画名（新增宠物不会因为漏写而报错）；
///   - 动画池只有一个事实来源（`animations.idle` / `animations.clicks`）。
///
/// `pets[].idle` / `pets[].click` 仍保留为可选字段：写了就用它做首帧 / 单次点击动画。
pub struct PetAnimationDefaults {
    /// 待机动画（该宠物**实际生效**的池里的第一个）
    pub idle: String,
    /// 点击回应动画（同上）
    pub click: String,
}

/// 从"某只宠物实际生效的动画池"派生默认动作名
///
/// 注意必须传**宠物**而不是全局配置：每只宠物可以有自己的一套池，
/// 用全局池派生会让"换了池的宠物"首帧仍然是全局池里的动画。
pub fn pet_animation_defaults(pet: &PetEntry, config: &AppConfig) -> PetAnimationDefaults {
    let animations = config.effective_animations(pet);
    PetAnimationDefaults {
        idle: animations.idle.first().cloned().unwrap_or_default(),
        click: animations.clicks.first().cloned().unwrap_or_default(),
    }
}

/// 兼容旧配置：清空 `pets[].idle` / `pets[].click`，让下游统一走"从池派生"这一条路。
///
/// 老版本这两个字段是必填的（例如 `"idle": "待机呼吸休闲.webm"`）。留着它们会造成
/// "配置写死的动画"与"动画池"两个事实来源，出现不一致时很难排查；
/// 清空后动画一律来自池——用户仍可自定义：把文件放进 `webm/` 并在池里引用它即可。
pub fn migrate_pet_animation_fields(config: &mut AppConfig) {
    for pet in &mut config.pets {
        pet.idle.clear();
        pet.click.clear();
    }
}

/// 剥掉 JSONC 注释（保留字符串字面量里的 `//`，例如 URL 与文件名里的斜杠）。
///
/// 之所以自己写而不是引三方 crate：规则很小（引号状态机 + 两种注释），
/// 引入依赖的成本高于收益；同时避免"注释剥错导致配置内容被吃掉"这类隐蔽问题。
fn strip_jsonc_comments(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    // 是否处于字符串字面量内部（内部不识别注释）
    let mut in_string = false;
    // 是否处于转义状态（字符串内的 \" 不结束字符串）
    let mut escaped = false;

    while let Some(c) = chars.next() {
        if in_string {
            out.push(c);
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
            continue;
        }
        if c == '"' {
            in_string = true;
            out.push(c);
            continue;
        }
        if c == '/' {
            match chars.peek() {
                Some('/') => {
                    // 行注释：吃到行尾（保留换行，便于报错时行号准确）
                    for n in chars.by_ref() {
                        if n == '\n' {
                            out.push('\n');
                            break;
                        }
                    }
                    continue;
                }
                Some('*') => {
                    // 块注释：吃到 */；注释内换行照样保留，保证行号不漂移
                    chars.next();
                    let mut prev = '\0';
                    for n in chars.by_ref() {
                        if n == '\n' {
                            out.push('\n');
                        }
                        if prev == '*' && n == '/' {
                            break;
                        }
                        prev = n;
                    }
                    continue;
                }
                _ => {}
            }
        }
        out.push(c);
    }
    out
}

/// 默认边距（像素）
const fn default_margin() -> i32 {
    24
}

/// 默认配置版本
const fn default_schema_version() -> u32 {
    1
}

// ---------------------------------------------------------------------------
// 内置默认（旧配置缺失新字段时的回落来源）
//
// 为什么从**内置模板本身**解析、而不是在 Rust 里再抄一份常量：
//   模板是唯一事实来源（由 scripts/sync-animation-pool.mjs 从上游同步 100+ 个动作名），
//   再抄一份必然漂移。解析失败时退化为"最小可用池"，保证程序仍能启动。
// ---------------------------------------------------------------------------

/// 解析内置模板得到默认的动画池（失败时退化为最小可用池）
pub fn default_animations() -> AnimationsConfig {
    parse_template::<AnimationsConfig>("animations").unwrap_or(AnimationsConfig {
        idle: vec!["待机呼吸休闲".to_string()],
        turn: Vec::new(),
        drag: Vec::new(),
        clicks: vec!["点击回应-元气挥手".to_string()],
        moves: MovesConfig { default: serde_json::Map::new(), actions: Vec::new() },
        categories: Vec::new(),
        events: std::collections::HashMap::new(),
    })
}

/// 解析内置模板得到默认的顶层权重（失败时回落上游默认值 10/5/5）
pub fn default_animation_weights() -> AnimationWeights {
    parse_template::<AnimationWeights>("animationWeights").unwrap_or(AnimationWeights { idle: 10.0, turn: 5.0, move_: 5.0 })
}

/// 从内置模板里解析某个顶层字段
fn parse_template<T: serde::de::DeserializeOwned>(field: &str) -> Option<T> {
    let root: serde_json::Value = serde_json::from_str(&strip_jsonc_comments(DEFAULT_CONFIG_TEMPLATE)).ok()?;
    serde_json::from_value(root.get(field)?.clone()).ok()
}

/// 把配置补齐到"当前结构"（幂等）：旧版配置缺什么就补什么。
///
/// 目前补两件事：
///   1. 清空旧版 `pets[].idle/click`（动画一律从池派生，见 [`migrate_pet_animation_fields`]）；
///   2. 动画池为空（旧配置没有这一段）时回落内置默认。
///
/// 之所以要做迁移而不是直接报错：升级后"打不开程序"对用户是最差的体验，
/// 而这类缺失有明确的、安全的默认值。
pub fn migrate_config(config: &mut AppConfig) {
    migrate_pet_animation_fields(config);
    if config.animations.idle.is_empty() {
        let defaults = default_animations();
        eprintln!(
            "[whale-pet] 配置缺少可用的 animations.idle（旧版配置？）→ 回落内置默认动画池（{} 个分类动作）",
            defaults.categories.iter().map(|c| c.actions.len()).sum::<usize>()
        );
        config.animations = defaults;
    }
    if config.animation_weights.idle + config.animation_weights.turn + config.animation_weights.move_ <= 0.0 {
        config.animation_weights = default_animation_weights();
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    /// 最小可用的 animations 段（测试夹具）：只放必需的池，避免每个用例重复长 JSON
    const MIN_ANIMATIONS: &str = r#""animations":{"idle":["待机"],"turn":[],"drag":[],"clicks":["点击"],
        "moves":{"default":{"minDist":10,"maxDist":20,"margin":5,"leadSec":0,"tailSec":0},"actions":[]},
        "categories":[],"events":{}}"animationWeights":{"idle":10,"turn":5,"move":5}"#;

    /// 拼一个最小合法配置（各用例只替换自己关心的片段）
    fn minimal_config(pets: &str) -> String {
        format!(
            r#"{{"schemaVersion":1,"physics":{{"gravity":1400,"restitution":0.78,"groundFriction":2.5,"ceilingBounce":true,"throwPower":1.0,"petCollision":false}},"pets":{pets},{MIN_ANIMATIONS}}}"#
        )
    }

    #[test]
    fn default_template_parses_and_validates() {
        let config: AppConfig =
            serde_json::from_str(&strip_jsonc_comments(DEFAULT_CONFIG_TEMPLATE)).expect("模板应可解析");
        config.validate().expect("模板应通过校验");
        assert_eq!(config.pets.len(), 1, "M0 模板默认单只宠物");
        // 动画池必须来自上游同步（100+ 个动作名不能丢）
        let category_actions: usize = config.animations.categories.iter().map(|c| c.actions.len()).sum();
        assert!(category_actions >= 50, "分类动作数偏少（{category_actions}），动画池可能没同步");
        assert!(!config.animations.moves.actions.is_empty(), "移动池不能为空");
    }

    #[test]
    fn comment_stripping_keeps_slashes_inside_strings() {
        let raw = r#"{"a":"http://example.com/x","b":1/*块注释*/,"c":2//行注释
        }"#;
        let stripped = strip_jsonc_comments(raw);
        assert!(stripped.contains("http://example.com/x"), "URL 不能被当注释剥掉：{stripped}");
        let value: serde_json::Value = serde_json::from_str(&stripped).expect("剥离后应可解析");
        assert_eq!(value["b"], 1);
        assert_eq!(value["c"], 2);
    }

    #[test]
    fn empty_pet_list_is_rejected() {
        let config: AppConfig = serde_json::from_str(&minimal_config("[]")).expect("结构应可解析");
        assert!(config.validate().is_err(), "空 pets 必须被拒绝");
    }

    #[test]
    fn invalid_pet_id_is_rejected() {
        let json = minimal_config(
            r#"[{"id":"a/b","name":"x","size":420,"idle":"","click":"","position":{"corner":"top-right","marginX":24,"marginY":24}}]"#,
        );
        let config: AppConfig = serde_json::from_str(&json).expect("结构应可解析");
        let err = config.validate().expect_err("含路径分隔符的 id 必须被拒绝");
        assert!(err.contains("非法字符"), "错误信息应说明原因：{err}");
    }

    #[test]
    fn empty_idle_pool_is_rejected() {
        let json = r#"{"schemaVersion":1,"physics":{"gravity":1400,"restitution":0.78,"groundFriction":2.5,"ceilingBounce":true,"throwPower":1.0,"petCollision":false},"pets":[{"id":"a","name":"x","size":420,"idle":"","click":"","position":{"corner":"top-left","marginX":1,"marginY":1}}],"animations":{"idle":[],"turn":[],"drag":[],"clicks":["点击"],"moves":{"default":{},"actions":[]},"categories":[],"events":{}},"animationWeights":{"idle":10,"turn":5,"move":5}}"#;
        let config: AppConfig = serde_json::from_str(json).expect("结构应可解析");
        let err = config.validate().expect_err("空 idle 池必须被拒绝");
        assert!(err.contains("idle"), "错误信息应指出 idle：{err}");
    }

    #[test]
    fn animation_asset_warnings_list_missing_names() {
        let config: AppConfig = serde_json::from_str(&minimal_config(
            r#"[{"id":"a","name":"x","size":420,"idle":"","click":"","position":{"corner":"top-left","marginX":1,"marginY":1}}]"#,
        ))
        .expect("结构应可解析");

        // 素材清单为空 → idle 与 clicks 里的名字都该被报出来
        let warnings = validate_animation_assets(&config, &[]);
        assert_eq!(warnings.len(), 2, "两个池各一个名字，应报两条：{warnings:?}");

        // 带上扩展名的素材清单应当命中（上游配置写的是不带扩展名的基名）
        let available = vec!["待机.webm".to_string(), "点击.webm".to_string()];
        assert!(validate_animation_assets(&config, &available).is_empty(), "带扩展名的清单应全部命中");

        // .mov 素材同样要认（macOS 变体）
        let mov = vec!["待机.mov".to_string(), "点击.mov".to_string()];
        assert!(validate_animation_assets(&config, &mov).is_empty(), ".mov 素材也必须被认出来");
    }

    #[test]
    fn animation_name_matching_handles_extensions() {
        let available = vec!["待机呼吸休闲.webm".to_string()];
        assert!(animation_file_exists("待机呼吸休闲", &available), "基名应命中");
        assert!(animation_file_exists("待机呼吸休闲.webm", &available), "全名应命中");
        assert!(!animation_file_exists("不存在的动画", &available), "不存在的名字不应命中");
        assert!(!animation_file_exists("待机呼吸休闲.mp4", &available), "写成别的扩展名不应命中");
        assert!(!animation_file_exists("  ", &available), "空白名不应命中");
    }
}
