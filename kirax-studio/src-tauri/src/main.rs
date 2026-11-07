//! KiraX Studio — Tauri 后端入口
//! Windows (MSVC) / Android (cargo-ndk) / Linux 三平台共用同一 Rust 核心。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    space_studio_lib::run()
}
