// ClipBeam:Tauri 2 入口。无参数启动托盘常驻应用;子命令走联调用 CLI。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use clap::Parser;
use clipbeam_lib::Cli;

fn main() {
    env_logger::init();
    let cli = Cli::parse();
    match cli.command {
        Some(cmd) => clipbeam_lib::run_cli(cmd),
        None => clipbeam_lib::run(),
    }
}
