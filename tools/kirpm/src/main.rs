// kirpm — .kir 包管理器
// 用法: kirpm install/list/remove
use std::fs;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(|s| s.as_str()).unwrap_or("--help") {
        "install" => {
            let name = args.get(2).cloned().unwrap_or_default();
            println!("[kirpm] 安装 {} → /apps/{}.kir", name, name);
            // 实际：从仓库下载 .kir，校验魔数 SKIR，写入 /apps/
        }
        "list" => {
            println!("[kirpm] 已安装应用:");
            if let Ok(entries) = fs::read_dir("apps") {
                for e in entries.flatten() {
                    println!("  - {}", e.file_name().to_string_lossy());
                }
            }
        }
        "remove" => {
            let name = args.get(2).cloned().unwrap_or_default();
            println!("[kirpm] 移除 {} (删除 /apps/{}.kir)", name, name);
        }
        _ => println!("用法: kirpm <install|list|remove> [name]"),
    }
}
