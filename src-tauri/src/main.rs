// Evita la consola extra en Windows en las builds de release.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    monitor_uso_claude_lib::run()
}
