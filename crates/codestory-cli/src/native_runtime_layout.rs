pub(crate) const NATIVE_RUNTIME_SEED_MARKER_PREFIX: &[u8] = b"codestory-native-runtime-seed-v1|id=";
pub(crate) const NATIVE_RUNTIME_SEED_MARKER_SUFFIX: &[u8] = b"|end";
#[cfg(target_os = "windows")]
pub(crate) const NATIVE_RUNTIME_EXECUTABLE: &str = "codestory-cli-runtime.exe";
#[cfg(not(target_os = "windows"))]
pub(crate) const NATIVE_RUNTIME_EXECUTABLE: &str = "codestory-cli-runtime";
pub(crate) const NATIVE_RUNTIME_SEEDS_DIR: &str = ".codestory-native-seeds";
pub(crate) const NATIVE_RUNTIME_GENERATIONS_DIR: &str = "codestory-native-generations";
pub(crate) const NATIVE_RUNTIME_CURRENT_FILE: &str = "codestory-native-current-generation-v1.txt";
pub(crate) const NATIVE_RUNTIME_FILE_LIST: &str = "codestory-native-runtime-files-v1.txt";
