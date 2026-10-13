// Без этого в релизной сборке рядом с окном открывается консоль.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    lsm_lib::run();
}
