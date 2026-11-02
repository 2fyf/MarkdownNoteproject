// Keep both local debug builds and release builds free of an extra console.
// The project launcher captures stdout/stderr in project-local log files.
#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

fn main() {
    markdownnote_lib::run();
}
