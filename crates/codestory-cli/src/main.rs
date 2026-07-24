#[cfg(any(target_os = "linux", target_os = "windows", test))]
#[cfg_attr(not(any(target_os = "linux", target_os = "windows")), allow(dead_code))]
mod native_launcher;
#[cfg(any(target_os = "linux", target_os = "windows", test))]
#[cfg_attr(not(any(target_os = "linux", target_os = "windows")), allow(dead_code))]
mod native_runtime_layout;

#[cfg(any(target_os = "linux", target_os = "windows"))]
fn main() -> std::process::ExitCode {
    native_launcher::run()
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
fn main() -> std::process::ExitCode {
    codestory_cli::run()
}
