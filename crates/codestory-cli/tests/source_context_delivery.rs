#![cfg(unix)]
mod test_support;

use serde_json::{Value, json};
use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};
use std::time::{Duration, Instant};

fn invoke(arguments: &[&str], input: &[u8], env: &[(&str, &str)]) -> Output {
    let mut command = test_support::cli_command();
    command
        .args(arguments)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
    for (name, value) in env {
        command.env(name, value);
    }
    let mut child = command.spawn().unwrap();
    let group = child.id() as i32;
    let mut stdout = child.stdout.take().unwrap();
    let mut stderr = child.stderr.take().unwrap();
    let out = std::thread::spawn(move || {
        let mut b = Vec::new();
        stdout.read_to_end(&mut b).unwrap();
        b
    });
    let err = std::thread::spawn(move || {
        let mut b = Vec::new();
        stderr.read_to_end(&mut b).unwrap();
        b
    });
    let _ = child.stdin.take().unwrap().write_all(input);
    let deadline = Instant::now() + Duration::from_secs(5);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= deadline {
            // This test assigned its child a new group; only this fixture's
            // descendants belong to it. Never signal a user or shared group.
            unsafe {
                libc::kill(-group, libc::SIGKILL);
            }
            let _ = child.wait();
            panic!("owned delivery fixture exceeded five seconds");
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    // A private-only fixture writer may intentionally outlive the original
    // shell. Reap only the process group this invocation created.
    unsafe {
        libc::kill(-group, libc::SIGKILL);
    }
    Output {
        status,
        stdout: out.join().unwrap(),
        stderr: err.join().unwrap(),
    }
}

fn project() -> (tempfile::TempDir, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(temp.path()).unwrap();
    (temp, root)
}

fn context(root: &Path, observations: Vec<Value>) -> Output {
    invoke(
        &["internal-source-context", "context"],
        &serde_json::to_vec(&json!({"project":root,"cwd":root,"observations":observations}))
            .unwrap(),
        &[],
    )
}

fn row(path: &str, line: usize, text: &str) -> Value {
    json!({"path":path,"physical_line":line,"text":text})
}

fn payload(root: &Path, script: &str) -> String {
    let shim = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../plugins/codestory/hooks/source-context-bin")
        .canonicalize()
        .expect("canonical packaged shim directory");
    serde_json::to_vec(
        &json!({"version":1,"project":root,"cwd":root,"command":script,"shim_directory":shim}),
    )
    .unwrap()
    .iter()
    .map(|byte| format!("{byte:02x}"))
    .collect()
}

#[test]
fn context_renders_exact_crlf_unicode_eof_and_deduplicates_whole_units() {
    let (_temp, root) = project();
    let body = "@wrap\r\nasync def build():\r\n    return '雪🙂'";
    fs::write(root.join("source.py"), body).unwrap();
    let output = context(
        &root,
        vec![
            row("source.py", 1, "@wrap\r"),
            row("source.py", 3, "    return '雪🙂'"),
        ],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout).unwrap();
    assert_eq!(text.matches(body).count(), 1);
    assert!(text.contains("physical_lines=1-3"));
    assert!(text.contains("complete_AsyncFunctionDef"));
    assert!(text.contains(&format!("exact_bytes={}", body.len())));
    assert!(text.contains("post-search"));
    assert!(text.contains("host truncation unknown"));
    assert!(text.len() - body.len() <= 2048);
}

#[test]
fn rendering_uses_one_shared_source_and_metadata_cap_without_clipped_units() {
    let (_temp, root) = project();
    let mut observations = Vec::new();
    let mut bodies = Vec::new();
    for n in 0..3 {
        let name = format!("source_{n}.py");
        let body = format!("def build_{n}():\n    return '{}'\n", "x".repeat(5900));
        fs::write(root.join(&name), &body).unwrap();
        observations.push(row(&name, 1, &format!("def build_{n}():")));
        bodies.push(body);
    }
    let output = context(&root, observations);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains(&bodies[0]) && text.contains(&bodies[1]));
    assert!(
        !text.contains("def build_2():"),
        "a declined unit must not be clipped"
    );
    assert!(text.contains("decline"));
    let source = bodies
        .iter()
        .filter(|body| text.contains(body.as_str()))
        .map(|body| body.len())
        .sum::<usize>();
    assert!(source <= 12288 && text.len() - source <= 2048);
}

#[test]
fn mismatch_or_unsupported_context_returns_no_extra_source() {
    let (_temp, root) = project();
    fs::write(root.join("source.py"), "def build():\n    return 1\n").unwrap();
    let output = context(&root, vec![row("source.py", 2, "    return 2")]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stdout.is_empty());
}

#[test]
fn capture_executes_original_compound_command_once_with_exact_streams_and_status() {
    let (_temp, root) = project();
    let marker = root.join("executions");
    let script = format!(
        "printf x >> '{}'; printf 'raw-out'; printf 'raw-err' >&2; exit 7",
        marker.display()
    );
    let encoded = payload(&root, &script);
    let output = invoke(
        &["internal-source-context", "capture", "--payload", &encoded],
        b"",
        &[],
    );
    assert_eq!(
        output.status.code(),
        Some(7),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"raw-out");
    assert_eq!(output.stderr, b"raw-err");
    assert_eq!(fs::read(marker).unwrap(), b"x");
}

#[test]
fn captured_actual_rg_rows_enrich_only_after_original_consumer_and_final_marker() {
    let (_temp, root) = project();
    let body = "def build():\n    value = 'literal source'\n    return value\n";
    fs::write(root.join("source.py"), body).unwrap();
    // This uses the ordinary post-login external rg through the packaged shim,
    // not an absolute bypass or a fake pre-startup PATH binding. Real rg is a
    // prerequisite of this selected host's process fixture.
    let script = "rg -Hn --color=never --no-heading 'return value' source.py | /usr/bin/head -n 1; printf 'ORIGINAL-COMMAND-DONE\\n'";
    let encoded = payload(&root, script);
    let output = invoke(
        &["internal-source-context", "capture", "--payload", &encoded],
        b"",
        &[],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stderr.is_empty(),
        "original stderr must remain exact"
    );
    let prefix = b"source.py:3:    return value\nORIGINAL-COMMAND-DONE\n";
    assert!(
        output.stdout.starts_with(prefix),
        "raw consumer output/final marker changed: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    let text = String::from_utf8(output.stdout).unwrap();
    assert_eq!(
        text.matches(body).count(),
        1,
        "capture must actually invoke runtime source selection"
    );
    assert!(
        text.find(body).unwrap() >= prefix.len(),
        "source entered the existing pipeline or preceded command completion"
    );
    assert!(text.contains("physical_lines=1-3") && text.contains("complete_FunctionDef"));
}

#[test]
fn captured_rust_rg_rows_append_the_exact_owner_after_the_original_command() {
    let (_temp, root) = project();
    let body =
        "/// Build a value.\n#[inline]\nfn build() {\n    let value = \"literal source\";\n}\n";
    fs::write(root.join("source.rs"), body).unwrap();
    let script = "rg -Hn --color=never --no-heading 'let value' source.rs | /usr/bin/head -n 1; printf 'ORIGINAL-COMMAND-DONE\\n'";
    let encoded = payload(&root, script);
    let output = invoke(
        &["internal-source-context", "capture", "--payload", &encoded],
        b"",
        &[],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    let prefix = b"source.rs:4:    let value = \"literal source\";\nORIGINAL-COMMAND-DONE\n";
    assert!(
        output.stdout.starts_with(prefix),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    let text = String::from_utf8(output.stdout).unwrap();
    assert_eq!(
        text.matches(body).count(),
        1,
        "Rust must reach verified runtime selection through the unchanged transport"
    );
    assert!(text.find(body).unwrap() >= prefix.len());
    assert!(text.contains("physical_lines=1-5") && text.contains("complete_function_item"));
    assert!(text.contains(&format!("exact_bytes={}", body.len())));
    assert!(body.len() <= 12288 && text.len() - prefix.len() - body.len() <= 2048);
}

#[test]
fn actual_single_rust_operand_with_and_without_h_delivers_the_same_owner() {
    let (_temp, root) = project();
    let body = "fn build() {\n    let value = 1;\n}\n";
    fs::write(root.join("source.rs"), body).unwrap();
    for (flags, prefix) in [
        ("-Hn", "source.rs:2:    let value = 1;\nDONE\n"),
        ("-n", "2:    let value = 1;\nDONE\n"),
    ] {
        let script = format!(
            "rg {flags} --color=never --no-heading 'let value' source.rs; printf 'DONE\\n'"
        );
        let encoded = payload(&root, &script);
        let output = invoke(
            &["internal-source-context", "capture", "--payload", &encoded],
            b"",
            &[],
        );
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.stderr.is_empty());
        assert!(output.stdout.starts_with(prefix.as_bytes()));
        let extra = std::str::from_utf8(&output.stdout[prefix.len()..]).unwrap();
        assert!(
            extra.contains(body),
            "actual {flags} source rows must bind the single .rs operand"
        );
    }
}

#[test]
fn actual_child_config_environment_controls_eligibility_even_after_shell_unsets_it() {
    let (_temp, root) = project();
    let early = format!("fn early() {{\n{}}}\n", "    red();\n".repeat(80));
    let late = "fn late() {\n    blue();\n}\n";
    fs::write(root.join("a.rs"), &early).unwrap();
    fs::write(root.join("z.rs"), late).unwrap();
    fs::write(root.join("rg-config"), "--ignore-case\n").unwrap();
    let query = "rg --sort path -Hn --color=never --no-heading 'red|blue' a.rs z.rs";
    // The successful raw query is identical in both cases. The config's
    // existence makes effective flags unknown, even though it does not change
    // these lowercase matches. Later shell exports cannot replace child proof.
    for (setup, expected_late) in [
        ("export RIPGREP_CONFIG_PATH=rg-config;", false),
        ("unset RIPGREP_CONFIG_PATH;", true),
    ] {
        let script = format!(
            "{setup} {query}; unset RIPGREP_CONFIG_PATH; printf 'ORIGINAL-COMMAND-DONE\\n'"
        );
        let encoded = payload(&root, &script);
        let output = invoke(
            &["internal-source-context", "capture", "--payload", &encoded],
            b"",
            &[],
        );
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.stderr.is_empty());
        let text = String::from_utf8(output.stdout).unwrap();
        assert!(text.starts_with("a.rs:2:    red();\n"));
        let marker = "ORIGINAL-COMMAND-DONE\n";
        let at = text.find(marker).unwrap() + marker.len();
        assert_eq!(
            text[..at].lines().count(),
            82,
            "all81 raw rows and final marker must survive"
        );
        let extra = &text[at..];
        assert_eq!(
            extra.contains(late),
            expected_late,
            "eligibility must use the actual rg child's config environment, not later/root state"
        );
        if expected_late {
            assert!(extra.contains(&early));
        }
    }
}

#[test]
fn capture_preserves_pipeline_substitution_and_file_redirection_consumers() {
    let (_temp, root) = project();
    let script = "printf 'first\\nsecond\\n' | /usr/bin/head -n 1; value=$(printf 'sub'); printf '%s' \"$value\"; printf 'file-only' > saved; printf 'err-only' >&2";
    let encoded = payload(&root, script);
    let output = invoke(
        &["internal-source-context", "capture", "--payload", &encoded],
        b"",
        &[],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"first\nsub");
    assert_eq!(output.stderr, b"err-only");
    assert_eq!(fs::read(root.join("saved")).unwrap(), b"file-only");
}

#[test]
fn inherited_private_only_writer_and_malformed_frames_cannot_stall_original_output() {
    let (_temp, root) = project();
    let script = "printf '{}' >&$CODESTORY_SOURCE_CONTEXT_FD; (/bin/sleep 30) >/dev/null 2>&1 & printf 'raw'";
    let encoded = payload(&root, script);
    let output = invoke(
        &["internal-source-context", "capture", "--payload", &encoded],
        b"",
        &[],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"raw");
    assert!(output.stderr.is_empty());
}

#[test]
fn rg_mode_keeps_actual_operands_once_and_raw_nonzero_streams() {
    let (_temp, root) = project();
    let rg = root.join("real-rg");
    let marker = root.join("argv");
    fs::write(&rg,"#!/bin/sh\nprintf '%s\\n' \"$@\" >> \"$FIXTURE_ARGV\"\nprintf 'actual-rows\\n'\nprintf 'actual-err' >&2\nexit 3\n").unwrap();
    fs::set_permissions(&rg, fs::Permissions::from_mode(0o755)).unwrap();
    let output = invoke(
        &[
            "internal-source-context",
            "rg",
            "--",
            "-n",
            "two words",
            "--",
            "-file.py",
        ],
        b"",
        &[
            ("CODESTORY_SOURCE_CONTEXT_BOUND_RG", rg.to_str().unwrap()),
            ("FIXTURE_ARGV", marker.to_str().unwrap()),
        ],
    );
    assert_eq!(
        output.status.code(),
        Some(3),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"actual-rows\n");
    assert_eq!(output.stderr, b"actual-err");
    assert_eq!(fs::read(marker).unwrap(), b"-n\ntwo words\n--\n-file.py\n");
}

#[test]
fn setup_carries_only_process_local_bindings_and_does_not_execute_script() {
    let (_temp, root) = project();
    let marker = root.join("must-not-run");
    let script = format!("printf bad > '{}'", marker.display());
    let encoded = payload(&root, &script);
    let rg = root.join("rg");
    fs::write(&rg, "#!/bin/sh\nexit 0\n").unwrap();
    fs::set_permissions(&rg, fs::Permissions::from_mode(0o755)).unwrap();
    let output = invoke(
        &[
            "internal-source-context",
            "setup",
            "--payload",
            &encoded,
            "--rg",
            rg.to_str().unwrap(),
            "--original-path",
            "/usr/bin:/bin",
        ],
        b"",
        &[],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!marker.exists());
    assert!(!output.stdout.is_empty());
}

#[test]
fn hidden_route_does_not_change_public_help_or_accept_unknown_modes() {
    let help = invoke(&["--help"], b"", &[]);
    assert!(help.status.success());
    assert!(!String::from_utf8_lossy(&help.stdout).contains("internal-source-context"));
    let invalid = invoke(&["internal-source-context", "unknown-mode"], b"", &[]);
    assert!(!invalid.status.success());
}
