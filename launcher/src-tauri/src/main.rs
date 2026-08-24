// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    let process_started = std::time::Instant::now();
    if let Some(exit_code) = claude_science_assistant_lib::smoke_exit_code_if_requested(
        std::env::args_os().skip(1),
        process_started,
    ) {
        std::process::exit(exit_code);
    }
    claude_science_assistant_lib::run()
}
