use std::process::Command;

#[test]
fn report_command_help_names_markdown_and_json_exports() {
    let output = Command::new(env!("CARGO_BIN_EXE_codestory-cli"))
        .arg("report")
        .arg("--help")
        .output()
        .expect("run report help");

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("help output is utf8");
    assert!(stdout.contains("Generate a repo report or machine graph export"));
    assert!(stdout.contains("--format <FORMAT>"));
    assert!(stdout.contains("--output-file <PATH>"));
    assert!(stdout.contains("--limit <N>"));
}
