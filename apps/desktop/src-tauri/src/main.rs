// Keeps the console window from flashing on Windows release builds. Child
// processes get the same treatment when the media engine lands.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    localconvert_desktop_lib::run();
}
