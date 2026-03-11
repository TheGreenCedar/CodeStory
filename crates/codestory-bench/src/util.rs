use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

pub fn generate_synthetic_project(file_count: usize) -> anyhow::Result<TempDir> {
    generate_repo_scale_project(file_count, 1, 1)
}

pub fn generate_repo_scale_project(
    file_count: usize,
    methods_per_file: usize,
    callsites_per_file: usize,
) -> anyhow::Result<TempDir> {
    let temp_dir = tempfile::tempdir()?;
    let root = temp_dir.path();

    for i in 0..file_count {
        let file_path = root.join(format!("file_{}.cpp", i));
        let content =
            generate_cpp_file_content(i, file_count, methods_per_file, callsites_per_file);
        fs::write(file_path, content)?;
    }

    Ok(temp_dir)
}

fn generate_cpp_file_content(
    index: usize,
    file_count: usize,
    methods_per_file: usize,
    callsites_per_file: usize,
) -> String {
    let mut prototypes = String::new();
    for callsite_idx in 0..callsites_per_file {
        let target_file = (index + callsite_idx + 1) % file_count.max(1);
        prototypes.push_str(&format!("void helper_{target_file}_{callsite_idx}();\n"));
    }

    let mut methods = String::new();
    for method_idx in 0..methods_per_file {
        methods.push_str(&format!(
            r#"
    void method_{index}_{method_idx}() {{
        int total = {method_idx};
"#,
        ));
        for callsite_idx in 0..callsites_per_file {
            let target_file = (index + callsite_idx + 1) % file_count.max(1);
            methods.push_str(&format!("        helper_{target_file}_{callsite_idx}();\n"));
        }
        methods.push_str("    }\n");
    }

    let mut helpers = String::new();
    for helper_idx in 0..callsites_per_file.max(1) {
        helpers.push_str(&format!(
            r#"
void helper_{index}_{helper_idx}() {{
    int value = {helper_idx};
    value += {index};
}}
"#
        ));
    }

    format!(
        r#"
{prototypes}

class Class_{index} {{
public:
{methods}
}};

{helpers}
"#,
        index = index
    )
}

pub fn generate_compile_commands(root: &Path, file_count: usize) -> anyhow::Result<()> {
    let mut commands = Vec::new();
    for i in 0..file_count {
        let file_name = format!("file_{}.cpp", i);
        commands.push(serde_json::json!({
            "directory": root.to_string_lossy(),
            "command": format!("clang++ -c {}", file_name),
            "file": file_name
        }));
    }

    let db_path = root.join("compile_commands.json");
    fs::write(db_path, serde_json::to_string_pretty(&commands)?)?;
    Ok(())
}

pub fn collect_files_with_extension(root: &Path, extension: &str) -> Vec<PathBuf> {
    let mut files = fs::read_dir(root)
        .unwrap()
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == extension))
        .collect::<Vec<_>>();
    files.sort();
    files
}

pub fn append_benchmark_markers(
    files: &[PathBuf],
    start_index: usize,
    count: usize,
    label: &str,
) -> anyhow::Result<Vec<PathBuf>> {
    if files.is_empty() || count == 0 {
        return Ok(Vec::new());
    }

    let touch_count = count.min(files.len());
    let mut touched = Vec::with_capacity(touch_count);
    for offset in 0..touch_count {
        let file_index = (start_index + offset) % files.len();
        let path = files[file_index].clone();
        let mut content = fs::read_to_string(&path)?;
        content.push_str(&format!(
            "\nvoid benchmark_touch_{label}_{start_index}_{offset}() {{}}\n"
        ));
        fs::write(&path, content)?;
        touched.push(path);
    }
    Ok(touched)
}

pub fn generate_grounding_project(
    file_count: usize,
    fanout: usize,
    helpers_per_file: usize,
) -> anyhow::Result<TempDir> {
    let temp_dir = tempfile::tempdir()?;
    let root = temp_dir.path();
    let src = root.join("src");
    fs::create_dir_all(&src)?;

    let mod_lines = (0..file_count)
        .map(|idx| format!("mod module_{idx};"))
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(
        src.join("main.rs"),
        format!(
            "mod shared;\n{mod_lines}\nfn main() {{ println!(\"hello\"); let _ = shared::shared_helper(); }}\n"
        ),
    )?;
    fs::write(
        src.join("shared.rs"),
        "pub struct SharedConfig { pub enabled: bool }\npub fn shared_helper() -> bool { true }\n",
    )?;

    for idx in 0..file_count {
        let cross_calls = (0..fanout)
            .map(|offset| {
                let target = (idx + offset + 1) % file_count.max(1);
                let helper_idx = offset % helpers_per_file.max(1);
                format!("        crate::module_{target}::helper_{target}_{helper_idx}();")
            })
            .collect::<Vec<_>>()
            .join("\n");
        let helpers = (0..helpers_per_file.max(1))
            .map(|helper_idx| {
                format!(
                    r#"
pub fn helper_{idx}_{helper_idx}() -> bool {{
    crate::shared::shared_helper()
}}
"#
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        let content = format!(
            r#"
pub struct Controller{idx} {{
    pub enabled: bool,
    pub bias: usize,
}}

impl Controller{idx} {{
    pub fn new() -> Self {{
        Self {{ enabled: crate::shared::shared_helper(), bias: {idx} }}
    }}

    pub fn check_winner_{idx}(&self) -> bool {{
{cross_calls}
        self.enabled && helper_{idx}_0()
    }}
}}

{helpers}
"#
        );
        fs::write(src.join(format!("module_{idx}.rs")), content)?;
    }

    Ok(temp_dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collect_files_with_extension_returns_sorted_matches() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        fs::write(temp_dir.path().join("b.cpp"), "int main() { return 0; }\n")?;
        fs::write(temp_dir.path().join("a.cpp"), "int main() { return 0; }\n")?;
        fs::write(temp_dir.path().join("ignore.rs"), "fn main() {}\n")?;

        let files = collect_files_with_extension(temp_dir.path(), "cpp");
        assert_eq!(files.len(), 2);
        assert!(files[0].ends_with("a.cpp"));
        assert!(files[1].ends_with("b.cpp"));
        Ok(())
    }

    #[test]
    fn append_benchmark_markers_rotates_across_files() -> anyhow::Result<()> {
        let temp_dir = generate_repo_scale_project(3, 1, 1)?;
        let files = collect_files_with_extension(temp_dir.path(), "cpp");

        let touched = append_benchmark_markers(&files, 2, 2, "rotation")?;
        assert_eq!(touched.len(), 2);
        assert!(touched[0].ends_with("file_2.cpp"));
        assert!(touched[1].ends_with("file_0.cpp"));

        let updated = fs::read_to_string(&touched[0])?;
        assert!(updated.contains("benchmark_touch_rotation_2_0"));
        Ok(())
    }

    #[test]
    fn generate_grounding_project_writes_cross_module_fixture() -> anyhow::Result<()> {
        let temp_dir = generate_grounding_project(4, 2, 3)?;
        let src = temp_dir.path().join("src");
        let rust_files = collect_files_with_extension(&src, "rs");

        assert_eq!(rust_files.len(), 6);
        let main_rs = fs::read_to_string(src.join("main.rs"))?;
        assert!(main_rs.contains("mod module_0;"));
        assert!(main_rs.contains("mod module_3;"));

        let module_rs = fs::read_to_string(src.join("module_0.rs"))?;
        assert!(module_rs.contains("crate::module_1::helper_1_0();"));
        assert!(module_rs.contains("crate::module_2::helper_2_1();"));
        Ok(())
    }
}
