use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Deserialize, Clone)]
pub struct CompileCommand {
    pub directory: PathBuf,
    pub command: Option<String>,
    pub arguments: Option<Vec<String>>,
    pub file: PathBuf,
}

/// C/C++ language standard
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CxxStandard {
    C89,
    C99,
    C11,
    C17,
    C23,
    Cxx98,
    Cxx03,
    Cxx11,
    Cxx14,
    Cxx17,
    Cxx20,
    Cxx23,
}

impl CxxStandard {
    fn from_flag(flag: &str) -> Option<Self> {
        match flag {
            "-std=c89" | "-std=c90" | "-std=iso9899:1990" => Some(Self::C89),
            "-std=c99" | "-std=iso9899:1999" | "-std=gnu99" => Some(Self::C99),
            "-std=c11" | "-std=iso9899:2011" | "-std=gnu11" => Some(Self::C11),
            "-std=c17" | "-std=c18" | "-std=gnu17" | "-std=gnu18" => Some(Self::C17),
            "-std=c2x" | "-std=c23" | "-std=gnu2x" | "-std=gnu23" => Some(Self::C23),
            "-std=c++98" | "-std=gnu++98" => Some(Self::Cxx98),
            "-std=c++03" | "-std=gnu++03" => Some(Self::Cxx03),
            "-std=c++11" | "-std=gnu++11" | "-std=c++0x" | "-std=gnu++0x" => Some(Self::Cxx11),
            "-std=c++14" | "-std=gnu++14" | "-std=c++1y" | "-std=gnu++1y" => Some(Self::Cxx14),
            "-std=c++17" | "-std=gnu++17" | "-std=c++1z" | "-std=gnu++1z" => Some(Self::Cxx17),
            "-std=c++20" | "-std=gnu++20" | "-std=c++2a" | "-std=gnu++2a" => Some(Self::Cxx20),
            "-std=c++23" | "-std=gnu++23" | "-std=c++2b" | "-std=gnu++2b" => Some(Self::Cxx23),
            _ => None,
        }
    }
}

/// Parsed compilation information for a single file
#[derive(Debug, Clone, Default)]
pub struct CompilationInfo {
    pub file: PathBuf,
    pub working_directory: PathBuf,
    pub include_paths: Vec<PathBuf>,
    pub system_include_paths: Vec<PathBuf>,
    pub defines: HashMap<String, Option<String>>,
    pub standard: Option<CxxStandard>,
    pub other_flags: Vec<String>,
}

pub struct CompilationDatabase {
    commands: Vec<CompileCommand>,
    index: HashMap<PathBuf, usize>,
}

impl CompilationDatabase {
    /// Load a compilation database from a JSON file (compile_commands.json).
    pub fn load<P: AsRef<Path>>(path: P) -> Result<Self> {
        let content =
            fs::read_to_string(path.as_ref()).context("Failed to read compile_commands.json")?;

        let commands: Vec<CompileCommand> =
            serde_json::from_str(&content).context("Failed to parse compile_commands.json")?;

        // Build index
        let mut index = HashMap::new();
        for (i, cmd) in commands.iter().enumerate() {
            let full_path = if cmd.file.is_absolute() {
                cmd.file.clone()
            } else {
                cmd.directory.join(&cmd.file)
            };
            index.insert(full_path, i);
        }

        Ok(Self { commands, index })
    }

    /// Get all file paths listed in the database.
    pub fn get_files(&self) -> Vec<PathBuf> {
        self.index.keys().cloned().collect()
    }

    /// Get the number of entries
    pub fn len(&self) -> usize {
        self.commands.len()
    }

    /// Check if database is empty
    pub fn is_empty(&self) -> bool {
        self.commands.is_empty()
    }

    /// Get compile info for a specific file.
    pub fn get_info_for_file(&self, path: &Path) -> Option<&CompileCommand> {
        self.index.get(path).map(|&i| &self.commands[i])
    }

    /// Parse compilation info for a file
    pub fn get_parsed_info(&self, path: &Path) -> Option<CompilationInfo> {
        let cmd = self.get_info_for_file(path)?;
        Some(Self::parse_compile_command(cmd))
    }

    /// Parse a compile command into structured info
    fn parse_compile_command(cmd: &CompileCommand) -> CompilationInfo {
        let args = if let Some(ref arguments) = cmd.arguments {
            arguments.clone()
        } else if let Some(ref command) = cmd.command {
            Self::split_command(command)
        } else {
            return CompilationInfo {
                file: cmd.file.clone(),
                working_directory: cmd.directory.clone(),
                ..Default::default()
            };
        };

        let mut info = CompilationInfo {
            file: cmd.file.clone(),
            working_directory: cmd.directory.clone(),
            ..Default::default()
        };

        let mut iter = args.iter().peekable();
        while let Some(arg) = iter.next() {
            match arg.as_str() {
                "-I" => {
                    if let Some(path) = iter.next() {
                        info.include_paths
                            .push(Self::resolve_path(&cmd.directory, path));
                    }
                }
                "-isystem" => {
                    if let Some(path) = iter.next() {
                        info.system_include_paths
                            .push(Self::resolve_path(&cmd.directory, path));
                    }
                }
                "-D" => {
                    if let Some(define) = iter.next() {
                        Self::parse_define(&mut info.defines, define);
                    }
                }
                arg if arg.starts_with("-I") => {
                    let path = &arg[2..];
                    if !path.is_empty() {
                        info.include_paths
                            .push(Self::resolve_path(&cmd.directory, path));
                    }
                }
                arg if arg.starts_with("-D") => {
                    let define = &arg[2..];
                    if !define.is_empty() {
                        Self::parse_define(&mut info.defines, define);
                    }
                }
                arg if arg.starts_with("-std=") => {
                    info.standard = CxxStandard::from_flag(arg);
                }
                arg if arg.starts_with("-isystem") => {
                    let path = &arg[8..];
                    if !path.is_empty() {
                        info.system_include_paths
                            .push(Self::resolve_path(&cmd.directory, path));
                    }
                }
                // Skip common flags we don't need
                "-c" | "-o" | "-g" | "-O0" | "-O1" | "-O2" | "-O3" | "-Os" | "-Oz" => {}
                arg if arg.starts_with("-W") => {} // Warnings
                arg if arg.starts_with("-f") => {} // Feature flags
                arg if arg.starts_with("-m") => {} // Machine flags
                // Store other flags
                _ => {
                    if !arg.starts_with("-") || arg.len() > 1 {
                        info.other_flags.push(arg.clone());
                    }
                }
            }
        }

        info
    }

    /// Split a command string into arguments (simple shell-like splitting)
    fn split_command(command: &str) -> Vec<String> {
        let mut args = Vec::new();
        let mut current = String::new();
        let mut in_quotes = false;
        let mut quote_char = '"';

        for ch in command.chars() {
            match ch {
                '"' | '\'' if !in_quotes => {
                    in_quotes = true;
                    quote_char = ch;
                }
                c if c == quote_char && in_quotes => {
                    in_quotes = false;
                }
                ' ' | '\t' if !in_quotes => {
                    if !current.is_empty() {
                        args.push(current.clone());
                        current.clear();
                    }
                }
                _ => {
                    current.push(ch);
                }
            }
        }
        if !current.is_empty() {
            args.push(current);
        }
        args
    }

    /// Resolve a potentially relative path
    fn resolve_path(base: &Path, path: &str) -> PathBuf {
        let p = PathBuf::from(path);
        if p.is_absolute() { p } else { base.join(p) }
    }

    /// Parse a define (-DFOO or -DFOO=bar)
    fn parse_define(defines: &mut HashMap<String, Option<String>>, define: &str) {
        if let Some(eq_pos) = define.find('=') {
            let name = define[..eq_pos].to_string();
            let value = define[eq_pos + 1..].to_string();
            defines.insert(name, Some(value));
        } else {
            defines.insert(define.to_string(), None);
        }
    }

    /// Try to find a compilation database in common locations
    pub fn find_in_directory(root: &Path) -> Option<PathBuf> {
        let candidates = [
            root.join("compile_commands.json"),
            root.join("build/compile_commands.json"),
            root.join("cmake-build-debug/compile_commands.json"),
            root.join("cmake-build-release/compile_commands.json"),
            root.join("out/compile_commands.json"),
            root.join(".build/compile_commands.json"),
        ];

        candidates.into_iter().find(|candidate| candidate.exists())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_split_command() {
        let cmd = "clang++ -I/usr/include -DFOO=bar -std=c++17 -c main.cpp";
        let args = CompilationDatabase::split_command(cmd);
        assert!(args.contains(&"-I/usr/include".to_string()));
        assert!(args.contains(&"-DFOO=bar".to_string()));
        assert!(args.contains(&"-std=c++17".to_string()));
    }

    #[test]
    fn test_parse_define() {
        let mut defines = HashMap::new();
        CompilationDatabase::parse_define(&mut defines, "FOO");
        CompilationDatabase::parse_define(&mut defines, "BAR=123");

        assert_eq!(defines.get("FOO"), Some(&None));
        assert_eq!(defines.get("BAR"), Some(&Some("123".to_string())));
    }

    #[test]
    fn test_cxx_standard_parsing() {
        assert_eq!(
            CxxStandard::from_flag("-std=c++17"),
            Some(CxxStandard::Cxx17)
        );
        assert_eq!(
            CxxStandard::from_flag("-std=gnu++20"),
            Some(CxxStandard::Cxx20)
        );
        assert_eq!(CxxStandard::from_flag("-std=c11"), Some(CxxStandard::C11));
        assert_eq!(CxxStandard::from_flag("-std=invalid"), None);
    }

    #[test]
    fn test_parse_compile_command() {
        let cmd = CompileCommand {
            directory: PathBuf::from("/home/user/project"),
            file: PathBuf::from("src/main.cpp"),
            command: Some(
                "clang++ -Iinclude -isystem/usr/local/include -DFOO=1 -std=c++14 -c src/main.cpp"
                    .to_string(),
            ),
            arguments: None,
        };

        let info = CompilationDatabase::parse_compile_command(&cmd);
        assert_eq!(info.file, PathBuf::from("src/main.cpp"));
        assert_eq!(
            info.include_paths,
            vec![PathBuf::from("/home/user/project/include")]
        );
        assert_eq!(
            info.system_include_paths,
            vec![PathBuf::from("/usr/local/include")]
        );
        assert_eq!(info.defines.get("FOO"), Some(&Some("1".to_string())));
        assert_eq!(info.standard, Some(CxxStandard::Cxx14));
    }

    #[test]
    fn test_parse_compile_command_with_args() {
        let cmd = CompileCommand {
            directory: PathBuf::from("/home/user/project"),
            file: PathBuf::from("src/main.cpp"),
            command: None,
            arguments: Some(vec![
                "clang++".to_string(),
                "-I".to_string(),
                "include2".to_string(),
                "-D".to_string(),
                "BAR".to_string(),
                "src/main.cpp".to_string(),
            ]),
        };

        let info = CompilationDatabase::parse_compile_command(&cmd);
        assert_eq!(
            info.include_paths,
            vec![PathBuf::from("/home/user/project/include2")]
        );
        assert_eq!(info.defines.get("BAR"), Some(&None));
    }
}
