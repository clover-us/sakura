//! 纯逻辑冒烟检查（可执行文件，不依赖 Rust 测试 harness）。
//!
//! 为什么要单独做这么一个 bin：
//!   本机是 MinGW-w64 + `x86_64-pc-windows-gnu` 工具链，`cargo test` 生成的 libtest
//!   可执行文件在启动时会被 Windows 加载器拒绝（`0xC0000139 STATUS_ENTRYPOINT_NOT_FOUND`），
//!   而同一个 crate 编出来的主程序能正常运行——导入表逐项比对完全一致，属于工具链/环境
//!   层面的问题，不是代码问题。为了让「配置解析、校验、路径防穿越、几何换算」这些纯逻辑
//!   **真的被执行到**（而不是只通过编译），这里用普通 bin 把它们跑一遍。
//!
//! 运行：cargo run --bin logic-smoke
//! 退出码：0 = 全部通过；1 = 有失败（并打印失败项）

use whale_pet_desktop_lib::config::{load_config, AppConfig};
use whale_pet_desktop_lib::model::{PhysicsParams, Rect};
use whale_pet_desktop_lib::pet_protocol::{asset_base_url, mime_for_test, sanitize_path_for_test};

/// 默认配置模板的路径（编译期由 cargo 提供 src-tauri 的绝对路径，避免依赖运行时工作目录）
const DEFAULT_CONFIG_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../config/default-config.jsonc");

/// 读取默认配置模板（读失败直接 panic：模板是交付物的一部分，缺了就该炸）
fn read_default_template() -> String {
    std::fs::read_to_string(DEFAULT_CONFIG_PATH)
        .unwrap_or_else(|err| panic!("读取默认配置模板失败 {DEFAULT_CONFIG_PATH}：{err}"))
}

/// 通过的检查数
static mut PASSED: u32 = 0;
/// 失败的检查数
static mut FAILED: u32 = 0;

/// 断言辅助：失败只记录不 panic，让所有检查都跑完再汇总
fn check(name: &str, condition: bool, detail: &str) {
    // SAFETY: 本程序单线程执行，不存在数据竞争
    unsafe {
        if condition {
            PASSED += 1;
            println!("  [通过] {name}");
        } else {
            FAILED += 1;
            println!("  [失败] {name} —— {detail}");
        }
    }
}

fn main() {
    println!("=== whale-pet 纯逻辑冒烟检查 ===");

    // ---- 1. 默认配置模板 ----
    println!("\n[1] 默认配置模板");
    let template = read_default_template();
    match serde_json::from_str::<AppConfig>(&strip(&template)) {
        Ok(config) => {
            check("默认模板可解析", true, "");
            check("默认模板通过校验", config.validate().is_ok(), &format!("{:?}", config.validate().err()));
            check("默认模板为单只宠物", config.pets.len() == 1, &format!("实际 {} 只", config.pets.len()));
            check(
                "默认尺寸 420",
                config.pets.first().map(|p| (p.size - 420.0).abs() < f64::EPSILON).unwrap_or(false),
                "读到的 size 不是 420",
            );
            check(
                "默认物理参数与上游一致",
                (config.physics.gravity - 1400.0).abs() < f64::EPSILON
                    && (config.physics.restitution - 0.78).abs() < f64::EPSILON,
                "gravity/restitution 与上游默认值不一致",
            );
        }
        Err(err) => {
            check("默认模板可解析", false, &format!("{err}"));
        }
    }

    // ---- 2. 配置校验的拒绝路径（"失败要大声报错"） ----
    println!("\n[2] 配置校验拒绝路径");
    let base = r#"{"schemaVersion":1,"physics":{"gravity":1400,"restitution":0.78,"groundFriction":2.5,"ceilingBounce":true,"throwPower":1.0,"petCollision":false}"#;
    let empty_pets = format!("{base},\"pets\":[]}}");
    let bad_id = format!(
        "{base},\"pets\":[{{\"id\":\"a/b\",\"name\":\"x\",\"size\":420,\"idle\":\"\",\"click\":\"\",\"position\":{{\"corner\":\"top-right\",\"marginX\":24,\"marginY\":24}}}}]}}"
    );
    let small_size = format!(
        "{base},\"pets\":[{{\"id\":\"ok\",\"name\":\"x\",\"size\":10,\"idle\":\"\",\"click\":\"\",\"position\":{{\"corner\":\"top-right\",\"marginX\":24,\"marginY\":24}}}}]}}"
    );
    let bad_version = r#"{"schemaVersion":9,"physics":{"gravity":1400,"restitution":0.78,"groundFriction":2.5,"ceilingBounce":true,"throwPower":1.0,"petCollision":false},"pets":[]}"#;

    // 统一用 &str：String 走 String::as_str（不要用 `raw_str.as_str()`，
    // 那是尚未稳定的 `str_as_str` 特性，在 stable 上编不过）
    let cases: [(&str, &str); 4] = [
        ("空宠物列表被拒绝", String::as_str(&empty_pets)),
        ("含路径分隔符的 id 被拒绝", String::as_str(&bad_id)),
        ("过小的 size 被拒绝", String::as_str(&small_size)),
        ("未知 schemaVersion 被拒绝", bad_version),
    ];

    for (name, json) in cases {
        let parsed = serde_json::from_str::<AppConfig>(json);
        let rejected = match parsed {
            Ok(config) => config.validate().is_err(),
            Err(_) => true, // 结构本身不合法也算被拒绝
        };
        check(name, rejected, "本应被拒绝却通过了");
    }

    // ---- 3. 物理参数取值范围 ----
    println!("\n[3] 物理参数取值范围");
    let mut physics = PhysicsParams::default();
    check("默认参数合法", physics.validate().is_ok(), "默认值居然不合法");
    physics.restitution = 1.5;
    check("恢复系数 > 1 被拒绝", physics.validate().is_err(), "越界值被放过");
    physics = PhysicsParams::default();
    physics.gravity = -1.0;
    check("负重力被拒绝", physics.validate().is_err(), "越界值被放过");
    physics = PhysicsParams::default();
    physics.throw_power = 0.0;
    check("力度 0 被拒绝", physics.validate().is_err(), "越界值被放过");

    // ---- 4. Rect 语义 ----
    println!("\n[4] 矩形包含语义（右/下缘不含）");
    let rect = Rect { x: 10.0, y: 20.0, width: 100.0, height: 50.0 };
    check("左上角在内", rect.contains(10.0, 20.0), "");
    check("右下缘内侧在内", rect.contains(109.9, 69.9), "");
    check("右缘不含", !rect.contains(110.0, 35.0), "右缘被算作内部");
    check("下缘不含", !rect.contains(50.0, 70.0), "下缘被算作内部");
    check("左外侧不含", !rect.contains(9.9, 35.0), "");

    // ---- 5. 自定义协议：路径防穿越与 MIME ----
    println!("\n[5] pet:// 协议的路径安全");
    check("普通文件名被接受", sanitize_path_for_test("待机.webm").is_some(), "");
    check("子目录形式被接受", sanitize_path_for_test("sub/a.webm").is_some(), "");
    check("`..` 被拒绝", sanitize_path_for_test("../secret.txt").is_none(), "路径穿越未被拦住");
    check("嵌套穿越被拒绝", sanitize_path_for_test("a/../../b.webm").is_none(), "路径穿越未被拦住");
    check("绝对路径被拒绝", sanitize_path_for_test("/etc/passwd").is_none(), "路径穿越未被拦住");
    check("空路径被拒绝", sanitize_path_for_test("").is_none(), "");
    check("webm 的 MIME 正确", mime_for_test("a.webm") == "video/webm", mime_for_test("a.webm"));
    check("扩展名大小写不敏感", mime_for_test("a.TTF") == "font/ttf", mime_for_test("a.TTF"));

    // ---- 6. 资源根地址的平台形式 ----
    println!("\n[6] 自定义协议 URL 形式");
    let base_url = asset_base_url();
    let expected = if cfg!(windows) { "http://pet.localhost" } else { "pet://localhost" };
    check("平台 URL 形式正确", base_url == expected, &format!("实际 {base_url}，期望 {expected}"));

    // ---- 7. JSONC 注释剥离 ----
    println!("\n[7] JSONC 注释剥离");
    let with_url = "{\"a\":\"http://example.com/x\",\"b\":1/*块*/,\"c\":2//行注释\n}";
    let stripped = strip(with_url);
    check("字符串里的斜杠不被当注释", stripped.contains("http://example.com/x"), &stripped);
    match serde_json::from_str::<serde_json::Value>(&stripped) {
        Ok(value) => {
            check("剥离后可解析", true, "");
            check("块注释后的字段保留", value["b"] == 1, &value["b"].to_string());
            check("行注释后的字段保留", value["c"] == 2, &value["c"].to_string());
        }
        Err(err) => check("剥离后可解析", false, &format!("{err}")),
    }

    // ---- 8. 真实配置文件读取（用一个临时文件跑一遍完整加载路径） ----
    println!("\n[8] 配置文件完整加载路径");
    let temp_dir = std::env::temp_dir().join("whale-pet-smoke");
    let _ = std::fs::create_dir_all(&temp_dir);
    let config_path = temp_dir.join("config.jsonc");
    if std::fs::write(&config_path, &template).is_ok() {
        match load_config(&config_path) {
            Ok(config) => {
                check("从文件加载并校验通过", true, "");
                check("读到的宠物名非空", !config.pets[0].name.is_empty(), "");
            }
            Err(err) => check("从文件加载并校验通过", false, &err),
        }
    } else {
        check("写入临时配置文件", false, "无法写入临时目录");
    }
    // 清理临时文件（失败也无所谓，位于系统临时目录）
    let _ = std::fs::remove_file(&config_path);

    // ---- 8b. 配置带 UTF-8 BOM 也要能读（Windows 记事本 / Set-Content 会加） ----
    println!("\n[8b] 配置文件带 UTF-8 BOM");
    let bom_path = temp_dir.join("config-bom.jsonc");
    let mut with_bom = String::from("\u{feff}");
    with_bom.push_str(&template);
    if std::fs::write(&bom_path, with_bom.as_bytes()).is_ok() {
        match load_config(&bom_path) {
            Ok(_) => check("带 BOM 的配置仍可加载", true, ""),
            Err(err) => check("带 BOM 的配置仍可加载", false, &err),
        }
    } else {
        check("写入带 BOM 的临时配置", false, "无法写入临时目录");
    }
    let _ = std::fs::remove_file(&bom_path);

    // ---- 9. 显示器几何的内在一致性（工作区必须被同序面板包含） ----
    println!("\n[9] 显示器几何一致性校验");
    use whale_pet_desktop_lib::display::validate_geometry;
    use whale_pet_desktop_lib::model::DisplaysSample;

    let screen = |x, y, w, h| Rect { x, y, width: w, height: h };
    let good = DisplaysSample {
        // 面板含任务栏（高 1080），工作区不含（高 1040）——真实 Windows 的典型形状
        areas: vec![screen(0.0, 0.0, 1920.0, 1040.0)],
        panels: vec![screen(0.0, 0.0, 1920.0, 1080.0)],
        primary: screen(0.0, 0.0, 1920.0, 1040.0),
    };
    check("合法几何通过校验", validate_geometry(&good).is_ok(), &format!("{:?}", validate_geometry(&good).err()));

    let length_mismatch = DisplaysSample {
        areas: vec![screen(0.0, 0.0, 1920.0, 1040.0)],
        panels: vec![screen(0.0, 0.0, 1920.0, 1080.0), screen(1920.0, 0.0, 1280.0, 1024.0)],
        primary: screen(0.0, 0.0, 1920.0, 1040.0),
    };
    check(
        "工作区/面板长度不一致被拒绝",
        validate_geometry(&length_mismatch).is_err(),
        "长度不一致（前端按序配对会拿错屏的边界）却被放过",
    );

    let not_contained = DisplaysSample {
        // 工作区跑到了面板外面：说明配对错序（把第二块屏的工作区配给了第一块的面板）
        areas: vec![screen(2000.0, 0.0, 1280.0, 1024.0)],
        panels: vec![screen(0.0, 0.0, 1920.0, 1080.0)],
        primary: screen(0.0, 0.0, 1920.0, 1040.0),
    };
    check(
        "工作区未被面板包含被拒绝",
        validate_geometry(&not_contained).is_err(),
        "错序配对未被检出",
    );

    let empty = DisplaysSample { areas: vec![], panels: vec![], primary: screen(0.0, 0.0, 1920.0, 1040.0) };
    check("空工作区列表被拒绝", validate_geometry(&empty).is_err(), "空列表未被检出");

    let zero_primary = DisplaysSample {
        areas: vec![screen(0.0, 0.0, 1920.0, 1040.0)],
        panels: vec![screen(0.0, 0.0, 1920.0, 1080.0)],
        primary: screen(0.0, 0.0, 0.0, 0.0),
    };
    check("主屏尺寸为 0 被拒绝", validate_geometry(&zero_primary).is_err(), "零尺寸主屏未被检出");

    // ---- 10. 保存配置（设置窗口的写盘路径） ----
    //
    // 这一段是 M2 新增的"设置窗口 → 写盘 → 读回"闭环中**可以纯逻辑验证**的部分：
    // 校验先于写盘、备份上一版、原子写、带注释头的文件仍能被 load_config 读回，
    // 以及"设置窗口重写一遍不会改变配置语义"（Round-trip 稳定）。
    println!("\n[10] 保存配置（设置窗口写盘路径）");
    {
        use whale_pet_desktop_lib::config::{
            config_file_path, load_config as load, migrate_config, save_config,
        };

        let dir = std::env::temp_dir().join("whale-pet-smoke-save");
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::create_dir_all(&dir);
        let path = config_file_path(&dir);

        // ① 先落一份模板（模拟"用户当前的配置"），再调 save_config 覆盖它。
        //    注意先过一遍 `migrate_config`：加载路径会做一次性的结构迁移
        //    （清空旧版 `pets[].idle/click`），拿"迁移前"的模板去比 round-trip
        //    必然不相等——这不是 bug，比的是两个不同的阶段。
        let mut template_config: AppConfig = serde_json::from_str(&strip(&template)).expect("模板应可解析");
        migrate_config(&mut template_config);
        let _ = std::fs::write(&path, &template);

        match save_config(&dir, &template_config) {
            Ok(report) => {
                check("save_config 返回写入路径", report.path == path.display().to_string(), &report.path);
                check("首次保存没有备份（原来没有文件才不备份）", report.backup.is_some(), "模板已存在，应当备份上一版");
                check("写入字节数非零", report.bytes > 0, &report.bytes.to_string());

                // ② 读回：写出去的是"规范化 JSON + 注释头"，load_config 必须能原样吃下
                match load(&path) {
                    Ok(reloaded) => {
                        check("保存后的文件可被 load_config 读回", true, "");
                        let same = canonical(&template_config) == canonical(&reloaded);
                        check("读回的配置与保存前语义一致", same, "round-trip 后配置变了");
                        check("保存后的文件带说明注释头", std::fs::read_to_string(&path).map(|t| t.starts_with("// ==")).unwrap_or(false), "文件开头没有说明段");
                        // 再存一轮：迁移是幂等的，因此这一轮必须**逐字节同义**
                        // （否则"用户每点一次保存，配置就悄悄漂一点"）
                        if save_config(&dir, &reloaded).is_ok() {
                            let again = load(&path).map(|c| canonical(&c)).unwrap_or_default();
                            check("连续两次 round-trip 完全稳定（迁移幂等）", again == canonical(&reloaded), "第二次 round-trip 又变了");
                        } else {
                            check("再次保存成功", false, "第二轮 save_config 失败");
                        }
                    }
                    Err(err) => check("保存后的文件可被 load_config 读回", false, &err),
                }

                // ③ 改动一处再存：备份文件应当等于"改动前"的那一版
                let mut edited = template_config.clone();
                if let Some(pet) = edited.pets.first_mut() {
                    pet.size = 640.0;
                }
                match save_config(&dir, &edited) {
                    Ok(second) => {
                        check("第二次保存仍返回备份路径", second.backup.is_some(), "备份路径为空");
                        let backup_text = std::fs::read_to_string(dir.join(whale_pet_desktop_lib::config::CONFIG_BACKUP_FILE_NAME)).unwrap_or_default();
                        let backup_config = serde_json::from_str::<AppConfig>(&strip(&backup_text));
                        check(
                            "备份里是上一版（改动前的 size）",
                            backup_config.map(|c| c.pets.first().map(|p| (p.size - 420.0).abs() < f64::EPSILON).unwrap_or(false)).unwrap_or(false),
                            "备份内容不是改动前的那一版",
                        );
                        let current = load(&path).map(|c| c.pets.first().map(|p| (p.size - 640.0).abs() < f64::EPSILON).unwrap_or(false)).unwrap_or(false);
                        check("当前文件是新版（改动后的 size）", current, "当前文件没写进新值");
                    }
                    Err(err) => check("第二次保存成功", false, &err),
                }

                // ④ 校验失败时**一个字节都不该写**（用户点保存不能把配置改成起不来的样子）
                let before = std::fs::read_to_string(&path).unwrap_or_default();
                let mut broken = edited.clone();
                if let Some(pet) = broken.pets.first_mut() {
                    pet.size = 10.0; // 低于下限 64
                }
                let rejected = save_config(&dir, &broken).is_err();
                let after = std::fs::read_to_string(&path).unwrap_or_default();
                check("非法配置被 save_config 拒绝", rejected, "越界配置竟然写成功了");
                check("被拒绝时配置文件未被改动", before == after, "文件被写坏了");
            }
            Err(err) => check("save_config 成功", false, &err),
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    // ---- 11. 每只宠物独立的动画池（M2 外观升级） ----
    //
    // 语义很关键：**整体覆盖**，不是逐字段合并。这里把三条边界钉死：
    //  ① 没写覆盖 → 用的是全局那份（指针级别的"同一份"不值得断言，比内容即可）；
    //  ② 写了覆盖 → 用的是自己的，且**全局怎么改都不影响它**；
    //  ③ 覆盖写坏了（idle 空）→ 校验要报错，并且错误信息里带宠物标识。
    println!("\n[11] 每宠独立动画池");
    {
        use whale_pet_desktop_lib::config::{migrate_config as migrate, validate_animation_assets};

        // 用一个最小配置做夹具：全局 idle=["全局待机"]，宠物 A 跟随、宠物 B 覆盖
        let minimal = |pet_extra: &str| {
            format!(
                r#"{{"schemaVersion":1,"physics":{{"gravity":1400,"restitution":0.78,"groundFriction":2.5,"ceilingBounce":true,"throwPower":1.0,"petCollision":false}},
                   "pets":[{{"id":"a","name":"跟随的","size":420,"idle":"","click":"","position":{{"corner":"top-right","marginX":24,"marginY":24}}}},
                           {{"id":"b","name":"自定义的","size":420,"idle":"","click":"","position":{{"corner":"top-right","marginX":24,"marginY":24}}{pet_extra}}}],
                   "animations":{{"idle":["全局待机"],"turn":[],"drag":[],"clicks":["全局点击"],
                     "moves":{{"default":{{}},"actions":[]}},"categories":[],"events":{{}}}},
                   "animationWeights":{{"idle":10,"turn":5,"move":5}}}}"#
            )
        };
        let own_ok = r#","animations":{"idle":["自己的待机"],"turn":[],"drag":[],"clicks":["自己的点击"],
                        "moves":{"default":{},"actions":[]},"categories":[],"events":{}},
                        "animationWeights":{"idle":1,"turn":1,"move":1}"#;
        let own_broken = r#","animations":{"idle":[],"turn":[],"drag":[],"clicks":["x"],
                        "moves":{"default":{},"actions":[]},"categories":[],"events":{}}"#;

        match serde_json::from_str::<AppConfig>(&minimal(own_ok)) {
            Ok(mut config) => {
                migrate(&mut config);
                let follows = &config.pets[0];
                let custom = &config.pets[1];
                check(
                    "没写覆盖的宠物用全局池",
                    config.effective_animations(follows).idle == vec!["全局待机".to_string()],
                    "跟随全局的宠物拿到了别的池",
                );
                check(
                    "写了覆盖的宠物用自己的池",
                    config.effective_animations(custom).idle == vec!["自己的待机".to_string()],
                    "覆盖没生效",
                );
                check(
                    "覆盖是整体替换：全局权重不参与",
                    config.effective_weights(custom).idle == 1.0,
                    "权重没有被覆盖",
                );
                check(
                    "自定义行为可被识别（UI/托盘用它打角标）",
                    !AppConfig::pet_has_custom_behaviour(follows) && AppConfig::pet_has_custom_behaviour(custom),
                    "识别函数结果不对",
                );
                // 全局改掉之后，覆盖的那只不受影响
                config.animations.idle = vec!["全局换掉了".to_string()];
                check(
                    "全局改动不影响已覆盖的宠物",
                    config.effective_animations(custom).idle == vec!["自己的待机".to_string()],
                    "覆盖被全局带着变了",
                );
                check("带覆盖的配置整体校验通过", config.validate().is_ok(), &format!("{:?}", config.validate().err()));
                // 素材警告要能指出是哪只宠物
                let warnings = validate_animation_assets(&config, &["不存在的素材.webm".to_string()]);
                check(
                    "素材警告带宠物作用域",
                    warnings.iter().any(|line| line.contains("宠物 自定义的")),
                    &format!("警告里没有宠物标识：{warnings:?}"),
                );
            }
            Err(err) => check("带覆盖的配置可解析", false, &err.to_string()),
        }

        match serde_json::from_str::<AppConfig>(&minimal(own_broken)) {
            Ok(mut config) => {
                migrate(&mut config);
                let err = config.validate().err().unwrap_or_default();
                check("覆盖写坏了会被校验拒绝", !err.is_empty(), "空 idle 的覆盖竟然通过");
                check(
                    "拒绝信息里指出是哪只宠物",
                    err.contains("自定义的"),
                    &format!("错误信息没有宠物标识：{err}"),
                );
            }
            Err(err) => check("覆盖写坏的配置仍可解析（结构本身合法）", false, &err.to_string()),
        }
    }

    // ---- 汇总 ----
    // SAFETY: 单线程
    let (passed, failed) = unsafe { (PASSED, FAILED) };
    println!("\n=== 结果：通过 {passed} 项，失败 {failed} 项 ===");
    if failed > 0 {
        std::process::exit(1);
    }
}

/// 规范化 JSON：递归按键名排序后序列化。
///
/// 实现已挪到 `config::canonical_json`（热重载判定"配置有没有真的变"也用同一份），
/// 冒烟程序直接复用，避免两处实现逐渐不一致。
fn canonical(config: &AppConfig) -> String {
    whale_pet_desktop_lib::config::canonical_json(config)
}

/// 本地复刻 config.rs 里的注释剥离（那边是私有函数，冒烟程序只验证行为一致即可）
fn strip(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    let mut in_string = false;
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
                    for n in chars.by_ref() {
                        if n == '\n' {
                            out.push('\n');
                            break;
                        }
                    }
                    continue;
                }
                Some('*') => {
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
