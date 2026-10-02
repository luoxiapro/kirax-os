// space-cli — Space OS 应用构建命令行工具
// 用法: space-cli new/build/run/package/install
// 仅用于构建宿主端工具，不依赖 Linux 内核。
use std::fs;
use std::path::Path;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let cmd = args.get(1).map(|s| s.as_str()).unwrap_or("--help");
    match cmd {
        "new" => new_app(args.get(2).cloned().unwrap_or_else(|| "app".into())),
        "build" => run_cmd("cargo", &["build", "--release"]),
        "run" => {
            run_cmd("cargo", &["build", "--release"]);
            run_cmd("qemu-system-arm", &[
                "-M", "raspi2b",
                "-kernel", "build/qemu-kernel.elf",
                "-display", "none",
                "-serial", "stdio",
            ]);
        }
        "package" => {
            // 把 release 产物按 .kir 格式打包
            let _ = fs::create_dir_all("build");
            let elf = fs::read("target/thumbv7a-none-eabi/release/app").unwrap_or_default();
            let mut out = Vec::new();
            out.extend_from_slice(b"SKIR");          // 魔数
            out.extend_from_slice(&1u32.to_le_bytes()); // 版本
            out.extend_from_slice(&32u32.to_le_bytes()); // 代码偏移
            out.extend_from_slice(&(elf.len() as u32).to_le_bytes()); // 代码大小
            out.extend_from_slice(&0u32.to_le_bytes()); // 图标偏移
            out.extend_from_slice(&0u32.to_le_bytes()); // 图标大小
            out.extend_from_slice(&0u32.to_le_bytes()); // manifest 偏移
            out.extend_from_slice(&0u32.to_le_bytes()); // manifest 大小
            out.extend_from_slice(&elf);
            fs::write("build/app.kir", out).unwrap();
            println!("[space-cli] 已打包 build/app.kir");
        }
        "install" => {
            let name = args.get(2).cloned().unwrap_or_default();
            println!("[space-cli] 安装 {} → /apps/ (需真机挂载 SD)", name);
        }
        _ => println!("用法: space-cli <new|build|run|package|install>"),
    }
}

fn new_app(name: String) {
    let dir = Path::new("apps").join(&name);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("Cargo.toml"), format!(
        "[package]\nname = \"{}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[[bin]]\nname = \"{}\"\npath = \"src/main.rs\"\n",
        name, name
    )).unwrap();
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(dir.join("src/main.rs"),
        "#![no_std]\n#![no_main]\n// Space OS 应用入口（模板）\n").unwrap();
    println!("[space-cli] 已生成 apps/{}", name);
}

fn run_cmd(cmd: &str, args: &[&str]) {
    let status = std::process::Command::new(cmd).args(args).status().unwrap();
    if !status.success() {
        eprintln!("[space-cli] {} 失败", cmd);
    }
}
