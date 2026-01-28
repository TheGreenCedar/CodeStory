use std::fs;
use std::path::Path;
use tempfile::TempDir;

pub fn generate_synthetic_project(file_count: usize) -> anyhow::Result<TempDir> {
    let temp_dir = tempfile::tempdir()?;
    let root = temp_dir.path();

    for i in 0..file_count {
        let file_path = root.join(format!("file_{}.cpp", i));
        let content = generate_cpp_file_content(i);
        fs::write(file_path, content)?;
    }

    Ok(temp_dir)
}

fn generate_cpp_file_content(index: usize) -> String {
    format!(
        r#"
class Class_{index} {{
public:
    void method_{index}() {{
        // Some logic
    }}
    
    void call_other() {{
        // Call method from previous file if it exists
        // This simulates cross-file dependencies
    }}
}};
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
