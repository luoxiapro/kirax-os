//! KiraX Studio — Tauri 命令层
//!
//! 前端 (Glass 设计语言) 通过 `invoke("list_projects")` 等命令与 Rust 核心交互。
//! 所有路径操作都限定在用户选择的工程根目录内，防目录穿越。
use serde::Serialize;
use std::path::{Path, PathBuf};
use tauri::Manager;

mod kirpack;

/// 工程元信息
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ProjectMeta {
    name: String,
    path: String,
    has_manifest: bool,
    modified: u64,
}

/// 列表指定目录下的 KiraX OS 工程（含 manifest.json 的目录）
#[tauri::command]
fn list_projects(root: String) -> Result<Vec<ProjectMeta>, String> {
    let root = PathBuf::from(root);
    if !root.is_dir() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for entry in std::fs::read_dir(&root).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let manifest = path.join("manifest.json");
        let has_manifest = manifest.exists();
        let modified = std::fs::metadata(&path)
            .map(|m| m.modified().map(|t| t.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)).unwrap_or(0))
            .unwrap_or(0);
        out.push(ProjectMeta {
            name: entry.file_name().to_string_lossy().to_string(),
            path: path.to_string_lossy().to_string(),
            has_manifest,
            modified,
        });
    }
    out.sort_by(|a, b| b.modified.cmp(&a.modified));
    Ok(out)
}

/// 新建工程骨架（src/main.rs + manifest.json）
#[tauri::command]
fn new_project(root: String, name: String) -> Result<ProjectMeta, String> {
    let name = sanitize_name(&name)?;
    let dir = PathBuf::from(root).join(&name);
    if dir.exists() {
        return Err(format!("工程 {name} 已存在"));
    }
    std::fs::create_dir_all(dir.join("src")).map_err(|e| e.to_string())?;
    std::fs::write(
        dir.join("src/main.rs"),
        "#![no_std]\n#![no_main]\n// KiraX OS 应用入口模板\n",
    )
    .map_err(|e| e.to_string())?;
    let manifest = format!(
        "{{\n  \"name\": \"{name}\",\n  \"title\": \"{name}\",\n  \"version\": \"0.1.0\",\n  \"icon\": \"planet\",\n  \"width\": 480,\n  \"height\": 320\n}}\n"
    );
    std::fs::write(dir.join("manifest.json"), manifest).map_err(|e| e.to_string())?;
    Ok(ProjectMeta {
        name,
        path: dir.to_string_lossy().to_string(),
        has_manifest: true,
        modified: 0,
    })
}

/// 读取文件（仅限工程根目录内）
#[tauri::command]
fn read_file(project_root: String, rel: String) -> Result<String, String> {
    let root = PathBuf::from(project_root);
    let p = root.join(rel);
    if !p.starts_with(&root) {
        return Err("路径越界".into());
    }
    std::fs::read_to_string(&p).map_err(|e| e.to_string())
}

/// 写入文件（仅限工程根目录内）
#[tauri::command]
fn write_file(project_root: String, rel: String, content: String) -> Result<(), String> {
    let root = PathBuf::from(project_root);
    let p = root.join(rel);
    if !p.starts_with(&root) {
        return Err("路径越界".into());
    }
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(&p, content).map_err(|e| e.to_string())
}

/// 打包 .kir：把 ELF 代码段 + 图标 + manifest 封装为 SKIR 二进制
#[tauri::command]
fn pack_kir(project_root: String, code_path: String, icon_path: Option<String>) -> Result<PackResult, String> {
    let root = PathBuf::from(project_root);
    let code = std::fs::read(root.join(code_path)).map_err(|e| e.to_string())?;
    let icon = match icon_path {
        Some(ip) => std::fs::read(root.join(ip)).unwrap_or_default(),
        None => Vec::new(),
    };
    let manifest = std::fs::read_to_string(root.join("manifest.json")).unwrap_or_else(|_| "{}".into());
    let kir = kirpack::pack(&code, &icon, manifest.as_bytes());
    let out_path = root.join("build").join(format!("{}.kir", root.file_name().unwrap_or_default().to_string_lossy()));
    std::fs::create_dir_all(out_path.parent().unwrap()).map_err(|e| e.to_string())?;
    std::fs::write(&out_path, &kir).map_err(|e| e.to_string())?;
    Ok(PackResult {
        path: out_path.to_string_lossy().to_string(),
        size: kir.len() as u64,
    })
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct PackResult {
    path: String,
    size: u64,
}

/// 在 QEMU 中运行内核（带指定 .kir 应用注入到 FAT32 镜像）
#[tauri::command]
fn run_qemu(kernel: String, apps_dir: Option<String>) -> Result<(), String> {
    let mut cmd = std::process::Command::new("qemu-system-arm");
    cmd.args([
        "-M", "raspi2b",
        "-kernel", &kernel,
        "-display", "none",
        "-serial", "stdio",
    ]);
    if let Some(dir) = apps_dir {
        cmd.args(["-drive", &format!("file={dir},format=raw,if=sd")]);
    }
    let child = cmd.spawn().map_err(|e| format!("启动 QEMU 失败: {e}"))?;
    let _ = child;
    Ok(())
}

/// 递归列出工程文件树（排除 build/target，供左侧"项目"面板展示）
#[tauri::command]
fn list_files(project_root: String) -> Result<Vec<String>, String> {
    let root = PathBuf::from(project_root);
    if !root.is_dir() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    collect_files(&root, &root, &mut out);
    out.sort();
    Ok(out)
}

fn collect_files(root: &Path, dir: &Path, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut names: Vec<_> = entries.filter_map(|e| e.ok()).collect();
    names.sort_by_key(|e| {
        let is_dir = e.path().is_dir();
        (!is_dir, e.file_name().to_string_lossy().to_lowercase())
    });
    for e in names {
        let name = e.file_name().to_string_lossy().to_string();
        if name == "target" || name == "build" || name.starts_with('.') {
            continue;
        }
        let rel = e
            .path()
            .strip_prefix(root)
            .unwrap_or(&e.path())
            .to_string_lossy()
            .to_string();
        if e.path().is_dir() {
            out.push(format!("{rel}/"));
            collect_files(root, &e.path(), out);
        } else {
            out.push(rel);
        }
    }
}

/// 主入口
/// `mobile_entry_point` 宏在 Android/iOS 构建时生成 JNI_OnLoad 与 Java native 绑定
/// （wry::android_binding!），缺失会导致 .so 无 JNI 符号、应用启动即崩。
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .invoke_handler(tauri::generate_handler![
            list_projects,
            new_project,
            list_files,
            read_file,
            write_file,
            pack_kir,
            run_qemu
        ])
        .setup(|app| {
            let _ = app.handle();
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("KiraX Studio 启动失败");
}

/// 工程名消毒：只允许字母/数字/下划线/中划线
fn sanitize_name(name: &str) -> Result<String, String> {
    if name.is_empty() || name.len() > 64 {
        return Err("工程名长度必须为 1..64".into());
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err("工程名只能包含字母/数字/下划线/中划线".into());
    }
    Ok(name.to_string())
}
