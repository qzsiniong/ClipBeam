//! 脚本目录与内置示例的行为测试（用临时目录，不碰用户真实的配置目录）。

use clipbeam_scripting::scripts::{
    check_script_name, ensure_seed_scripts_in, extension_of, is_valid_script_name, language_of,
    SCRIPT_EXTENSIONS, SEED_SCRIPTS,
};

/// 建一个独立的临时目录。
fn temp_dir(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "clipbeam-scripts-test-{}-{tag}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// 合法/非法脚本名的判定。
#[test]
fn name_validation() {
    for ok in ["a.js", "01-quick-start.js", "demo.TS", "x.mts", "y.cjs", "z.cts"] {
        assert!(is_valid_script_name(ok), "{ok} 应当合法");
    }
    for bad in [
        "",
        "a.txt",
        "noext",
        "../evil.js",
        "sub/dir.js",
        "sub\\dir.js",
        ".hidden.js",
        "a..b.js",
    ] {
        assert!(!is_valid_script_name(bad), "{bad} 应当被拒绝");
    }

    assert!(check_script_name("ok.js").is_ok());
    let err = check_script_name("../evil.js").expect_err("非法名应当报错");
    assert!(err.contains("非法脚本文件名"), "错误信息应当说明原因：{err}");
}

/// 扩展名与语言标签映射。
#[test]
fn extension_and_language_mapping() {
    assert_eq!(extension_of("a.TS").as_deref(), Some("ts"));
    assert_eq!(extension_of("noext"), None);
    assert_eq!(language_of("a.ts"), "ts");
    assert_eq!(language_of("a.mts"), "ts");
    assert_eq!(language_of("a.cts"), "ts");
    assert_eq!(language_of("a.js"), "js");
    assert_eq!(SCRIPT_EXTENSIONS.len(), 6);
}

/// 首次写 seed 会写全部示例，再次调用不覆盖用户改动。
#[test]
fn seeds_are_written_once_and_never_overwritten() {
    let dir = temp_dir("seed");

    let first = ensure_seed_scripts_in(&dir).expect("首次写 seed 失败");
    assert_eq!(first, SEED_SCRIPTS.len(), "首次应当写入全部示例");

    // 用户改过的示例不应被还原
    let edited = "// 用户改过的内容\n";
    std::fs::write(dir.join(SEED_SCRIPTS[0].0), edited).unwrap();

    let second = ensure_seed_scripts_in(&dir).expect("二次写 seed 失败");
    assert_eq!(second, 0, "二次不应再写");
    assert_eq!(
        std::fs::read_to_string(dir.join(SEED_SCRIPTS[0].0)).unwrap(),
        edited
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// 示例脚本自身要能通过类型/语法层面的基本检查（非空、扩展名合法）。
#[test]
fn seed_scripts_are_named_and_non_empty() {
    for (name, source) in SEED_SCRIPTS {
        assert!(is_valid_script_name(name), "示例名 {name} 非法");
        assert!(!source.trim().is_empty(), "示例 {name} 内容为空");
        assert!(
            source.contains("Clipbeam"),
            "示例 {name} 应当使用 Clipbeam 全局对象"
        );
    }
}

/// 示例里的 TS 必须在进程内能转译成 JS（不需要 Node）。
#[test]
fn seed_typescript_transpiles() {
    let (name, source) = SEED_SCRIPTS
        .iter()
        .find(|(name, _)| name.ends_with(".ts"))
        .expect("应当有一个 TS 示例");

    let js = clipbeam_scripting::ts::transpile(source, std::path::Path::new(name))
        .expect("示例 TS 应当能转译");

    assert!(!js.contains("interface "), "interface 应当被剥掉：\n{js}");
    assert!(!js.contains("enum "), "enum 语法应当被转译：\n{js}");
    assert!(js.contains("await"), "顶层 await 应当保留：\n{js}");
}

/// 示例里的 JS 语法必须成立（用引擎真跑一遍最小化版本会依赖文件与宿主，
/// 这里只做静态断言：能被 QuickJS 解析的部分我们通过 TS 转译器的解析阶段验证）。
#[test]
fn seed_javascript_parses_as_javascript() {
    let (name, source) = SEED_SCRIPTS
        .iter()
        .find(|(name, _)| name.ends_with(".js"))
        .expect("应当有一个 JS 示例");

    // 用 .ts 路径解析同一份源码：oxc 允许 JS 在 TS 下解析，能捕获语法错误
    let js = clipbeam_scripting::ts::transpile(source, std::path::Path::new(name))
        .expect("示例 JS 应当能解析");
    assert!(!js.is_empty());
}

/// 直接对临时目录做读写删除（绕过系统配置目录）。
#[test]
fn write_read_delete_roundtrip_in_temp_dir() {
    let dir = temp_dir("crud");
    std::fs::create_dir_all(&dir).unwrap();

    let path = dir.join("demo.js");
    std::fs::write(&path, "console.log(1)\n").unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "console.log(1)\n");

    // 只列出合法扩展名：混进一个 txt 不应出现在结果里
    std::fs::write(dir.join("notes.txt"), "x").unwrap();
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .filter_map(|entry| entry.file_name().to_str().map(str::to_string))
        .filter(|name| is_valid_script_name(name))
        .collect();
    names.sort();
    assert_eq!(names, vec!["demo.js".to_string()]);

    std::fs::remove_file(&path).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}
