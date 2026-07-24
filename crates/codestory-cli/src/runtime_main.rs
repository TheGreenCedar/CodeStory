#[cfg(any(target_os = "linux", target_os = "windows"))]
fn retain_native_runtime_seed_marker() {
    // SAFETY: this reads one initialized, aligned immutable static. The volatile
    // access keeps its relocation and marker bytes in the final executable.
    let _ =
        unsafe { std::ptr::read_volatile(&codestory_llama_sys::NATIVE_ENGINE_GENERATION_MARKER) };
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
fn main() -> std::process::ExitCode {
    retain_native_runtime_seed_marker();
    codestory_cli::run()
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
fn main() -> std::process::ExitCode {
    eprintln!("codestory-cli-runtime is internal to dynamic native runtime packages");
    std::process::ExitCode::FAILURE
}
