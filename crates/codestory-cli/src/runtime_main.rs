#[cfg(any(target_os = "linux", target_os = "windows"))]
#[used]
static NATIVE_RUNTIME_SEED_MARKER: &&[u8] = &codestory_llama_sys::NATIVE_ENGINE_GENERATION_MARKER;

#[cfg(any(target_os = "linux", target_os = "windows"))]
fn main() -> std::process::ExitCode {
    codestory_cli::run()
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
fn main() -> std::process::ExitCode {
    eprintln!("codestory-cli-runtime is internal to dynamic native runtime packages");
    std::process::ExitCode::FAILURE
}
