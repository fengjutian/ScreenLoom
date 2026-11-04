//! 把 `assets/screenloom.ico` 注入 exe 的 **PE 资源节**。
//!
//! 走 Windows SDK 的 `rc.exe` + MSVC 的 `cvtres.exe`，**不引入任何新依赖**
//! （`embed-resource` 那类 crate 会带来 cc/vswhom/winreg 等一串传递依赖）。
//!
//! 工具找不到时**跳过注入**而不是让构建失败 —— exe 少个图标而已，不该挡住开发。
//! 也可用环境变量指定：`SCREENLOOM_RC` / `SCREENLOOM_CVTRES`。

use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=assets/screenloom.ico");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=SCREENLOOM_RC");
    println!("cargo:rerun-if-env-changed=SCREENLOOM_CVTRES");

    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let ico = manifest.join("assets").join("screenloom.ico");
    if !ico.exists() {
        println!("cargo:warning=screenloom: assets/screenloom.ico 不存在，跳过图标注入");
        return;
    }

    let Some(rc) = find_tool("rc.exe") else {
        println!("cargo:warning=screenloom: 找不到 rc.exe，跳过图标注入");
        return;
    };
    let Some(cvtres) = find_tool("cvtres.exe") else {
        println!("cargo:warning=screenloom: 找不到 cvtres.exe，跳过图标注入");
        return;
    };

    let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    // .rc 里路径要用双反斜杠（RC 字符串里 `\\` 表示一个反斜杠）
    let ico_path = ico.to_string_lossy().replace('\\', "\\\\");
    let rc_file = out_dir.join("screenloom.rc");
    if let Err(e) = std::fs::write(&rc_file, format!("1 ICON \"{ico_path}\"\n")) {
        println!("cargo:warning=screenloom: 写 .rc 失败（{e}），跳过图标注入");
        return;
    }

    let res = out_dir.join("screenloom.res");
    let obj = out_dir.join("screenloom_res.obj");

    let ok = Command::new(&rc)
        .arg("/nologo")
        .arg("/fo")
        .arg(&res)
        .arg(&rc_file)
        .current_dir(&out_dir)
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !ok || !res.exists() {
        println!("cargo:warning=screenloom: rc.exe 失败，跳过图标注入");
        return;
    }

    let ok = Command::new(&cvtres)
        .arg("/nologo")
        .arg(format!("/out:{}", obj.display()))
        .arg(&res)
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !ok || !obj.exists() {
        println!("cargo:warning=screenloom: cvtres.exe 失败，跳过图标注入");
        return;
    }

    println!("cargo:rustc-link-arg-bins={}", obj.display());
}

/// 找一个可执行工具：环境变量 → PATH → 已知安装根目录递归搜。
fn find_tool(name: &str) -> Option<PathBuf> {
    let var = format!(
        "SCREENLOOM_{}",
        name.trim_end_matches(".exe")
            .to_uppercase()
            .replace('.', "_")
    );
    if let Ok(v) = std::env::var(&var) {
        let p = PathBuf::from(v);
        if p.is_file() {
            return Some(p);
        }
    }

    if let Ok(path) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path) {
            let p = dir.join(name);
            if p.is_file() {
                return Some(p);
            }
        }
    }

    // Program Files(x86) 下有 "Windows Kits" 与 "Microsoft Visual Studio"
    let mut roots: Vec<PathBuf> = Vec::new();
    for var in ["ProgramFiles(x86)", "ProgramFiles", "ProgramW6432"] {
        if let Ok(v) = std::env::var(var) {
            roots.push(PathBuf::from(v));
        }
    }
    let mut hits: Vec<PathBuf> = Vec::new();
    for r in &roots {
        for sub in ["Windows Kits", "Microsoft Visual Studio"] {
            let dir = r.join(sub);
            if dir.is_dir() {
                collect_files(&dir, name, 12, &mut hits);
            }
        }
    }
    pick_best(hits, name)
}

/// 在命中列表里挑最合适的那个：优先原生 x64 宿主工具链。
fn pick_best(hits: Vec<PathBuf>, name: &str) -> Option<PathBuf> {
    if hits.is_empty() {
        return None;
    }
    let s = name.to_ascii_lowercase();
    let score = |p: &Path| -> i32 {
        let t = p.to_string_lossy().replace('/', "\\").to_ascii_lowercase();
        let mut v = 0;
        if t.contains("\\hostx64\\x64\\") {
            v += 100;
        } else if t.contains("\\x64\\") {
            v += 50;
        }
        if t.contains("\\x86\\") {
            v -= 60;
        }
        if s == "rc.exe" && t.contains("windows kits") {
            v += 20;
        }
        if s == "cvtres.exe" && t.contains("msvc") {
            v += 20;
        }
        v
    };
    hits.into_iter().max_by_key(|p| score(p))
}

/// 递归收集同名文件（限制深度，避免扫穿整盘）。
fn collect_files(dir: &Path, name: &str, depth: usize, out: &mut Vec<PathBuf>) {
    if depth == 0 || out.len() > 64 {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            collect_files(&p, name, depth - 1, out);
        } else if p.file_name().and_then(|s| s.to_str()) == Some(name) {
            out.push(p);
        }
    }
}
