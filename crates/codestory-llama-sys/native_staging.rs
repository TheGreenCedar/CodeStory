use std::fs;
use std::io;
use std::path::Path;

const UPSTREAM_BUILD_SUPPORT_LIBRARY_NAMES: &[&str] = &["libllama-common.so"];

/// Stage real Linux shared-library files into every Cargo runtime directory.
///
/// `llama-cpp-sys-2` hard-links only the unversioned `.so` symlinks into these
/// directories. The links are therefore dangling, so its next build observes
/// `Path::exists() == false` and then fails to create the already-present link.
/// It also omits dynamically loaded backend modules from `deps` and `examples`,
/// even though test and example executables resolve runtime modules beside the
/// executable. Replacing the already validated runtime files with hard links to
/// their resolved regular files fixes both boundaries without duplicating bytes
/// or staging libraries outside the runtime manifest. The pinned upstream build
/// also places a dangling `libllama-common.so` link in these directories. That
/// build-support library remains outside CodeStory's package manifest; this
/// helper refreshes its entry where upstream already created one so a reused
/// target directory cannot retain bytes from an older dependency build.
pub(crate) fn stage_linux_shared_libraries(
    runtime_sources: &[&Path],
    upstream_build_support_sources: &[&Path],
    profile_dir: &Path,
) -> io::Result<()> {
    let mut sources = runtime_sources.to_vec();
    for source in &sources {
        if !source.is_file() || !is_linux_shared_library_name(source) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "invalid Linux runtime shared-library source: {}",
                    source.display()
                ),
            ));
        }
    }
    for source in upstream_build_support_sources {
        let Some(name) = source.file_name().and_then(|name| name.to_str()) else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "invalid upstream build-support source: {}",
                    source.display()
                ),
            ));
        };
        if !source.is_file() || !UPSTREAM_BUILD_SUPPORT_LIBRARY_NAMES.contains(&name) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "unexpected upstream build-support source: {}",
                    source.display()
                ),
            ));
        }
    }
    sources.sort_by(|left, right| left.file_name().cmp(&right.file_name()));
    for pair in sources.windows(2) {
        if pair[0].file_name() == pair[1].file_name() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!(
                    "duplicate Linux shared-library source: {}",
                    pair[0]
                        .file_name()
                        .expect("filtered source has a file name")
                        .to_string_lossy()
                ),
            ));
        }
    }

    let destinations = [
        profile_dir.to_path_buf(),
        profile_dir.join("deps"),
        profile_dir.join("examples"),
    ];
    for destination_dir in destinations {
        fs::create_dir_all(&destination_dir)?;
        for source in &sources {
            let name = source
                .file_name()
                .expect("filtered shared-library source has a file name");
            replace_with_real_hard_link(source, &destination_dir.join(name))?;
        }
        for source in upstream_build_support_sources {
            refresh_preexisting_build_support_link(source, &destination_dir)?;
        }
    }
    Ok(())
}

fn is_linux_shared_library_name(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    name.starts_with("lib") && (name.ends_with(".so") || name.contains(".so."))
}

fn refresh_preexisting_build_support_link(source: &Path, destination_dir: &Path) -> io::Result<()> {
    let destination = destination_dir.join(
        source
            .file_name()
            .expect("validated build-support source has a file name"),
    );
    match fs::symlink_metadata(&destination) {
        Ok(_) => replace_with_real_hard_link(source, &destination),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn replace_with_real_hard_link(source: &Path, destination: &Path) -> io::Result<()> {
    let source = fs::canonicalize(source)?;
    match fs::symlink_metadata(destination) {
        Ok(metadata) if metadata.file_type().is_dir() => {
            return Err(io::Error::new(
                io::ErrorKind::IsADirectory,
                format!(
                    "shared-library destination is a directory: {}",
                    destination.display()
                ),
            ));
        }
        Ok(_) => fs::remove_file(destination)?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    fs::hard_link(source, destination)?;
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    #[test]
    fn replaces_dangling_links_in_every_cargo_runtime_directory() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let core_dir = temp.path().join("native/lib");
        let backend_dir = temp.path().join("native/backends");
        let profile_dir = temp.path().join("target/debug");
        fs::create_dir_all(&core_dir).expect("core directory");
        fs::create_dir_all(&backend_dir).expect("backend directory");
        fs::write(core_dir.join("libggml.so.0.13.1"), b"ggml").expect("versioned library");
        symlink("libggml.so.0.13.1", core_dir.join("libggml.so.0")).expect("soname link");
        symlink("libggml.so.0", core_dir.join("libggml.so")).expect("linker-name link");
        fs::write(backend_dir.join("libggml-cpu-x64.so"), b"cpu").expect("CPU module");
        fs::write(backend_dir.join("libggml-vulkan.so"), b"vulkan").expect("Vulkan module");
        fs::write(core_dir.join("libllama-common.so.0.0.0"), b"common")
            .expect("build-support library");
        symlink(
            "libllama-common.so.0.0.0",
            core_dir.join("libllama-common.so"),
        )
        .expect("build-support linker-name link");
        fs::write(core_dir.join("libextra.so"), b"extra").expect("unowned shared library");

        let runtime_sources = [
            core_dir.join("libggml.so"),
            core_dir.join("libggml.so.0"),
            core_dir.join("libggml.so.0.13.1"),
            backend_dir.join("libggml-cpu-x64.so"),
            backend_dir.join("libggml-vulkan.so"),
        ];
        let runtime_sources = runtime_sources
            .iter()
            .map(|path| path.as_path())
            .collect::<Vec<_>>();
        let build_support_source = core_dir.join("libllama-common.so");

        for destination_dir in [
            profile_dir.clone(),
            profile_dir.join("deps"),
            profile_dir.join("examples"),
        ] {
            fs::create_dir_all(&destination_dir).expect("runtime directory");
            symlink("libggml.so.0", destination_dir.join("libggml.so"))
                .expect("upstream dangling link");
            if destination_dir != profile_dir {
                symlink(
                    "libllama-common.so.0",
                    destination_dir.join("libllama-common.so"),
                )
                .expect("upstream non-runtime dangling link");
            }
            symlink("libextra.so.0", destination_dir.join("libextra.so"))
                .expect("unowned dangling link");
            assert!(!destination_dir.join("libggml.so").exists());
            assert!(fs::symlink_metadata(destination_dir.join("libggml.so")).is_ok());
        }

        stage_linux_shared_libraries(
            &runtime_sources,
            &[build_support_source.as_path()],
            &profile_dir,
        )
        .expect("stage shared libraries");
        assert_runtime_sources_unchanged(&runtime_sources);

        for destination_dir in [
            profile_dir.clone(),
            profile_dir.join("deps"),
            profile_dir.join("examples"),
        ] {
            for name in ["libggml.so", "libggml.so.0", "libggml.so.0.13.1"] {
                let destination = destination_dir.join(name);
                assert!(destination.is_file(), "{}", destination.display());
                assert!(
                    !fs::symlink_metadata(&destination)
                        .expect("staged metadata")
                        .file_type()
                        .is_symlink(),
                    "{}",
                    destination.display()
                );
                assert_eq!(fs::read(destination).expect("staged bytes"), b"ggml");
            }
            assert_eq!(
                fs::read(destination_dir.join("libggml-cpu-x64.so")).expect("staged CPU module"),
                b"cpu"
            );
            assert_eq!(
                fs::read(destination_dir.join("libggml-vulkan.so")).expect("staged Vulkan module"),
                b"vulkan"
            );
            assert!(!destination_dir.join("libextra.so").exists());
            assert!(
                fs::symlink_metadata(destination_dir.join("libextra.so"))
                    .expect("unowned link remains")
                    .file_type()
                    .is_symlink(),
                "unowned shared library must remain untouched"
            );
            if destination_dir == profile_dir {
                assert!(
                    fs::symlink_metadata(destination_dir.join("libllama-common.so")).is_err(),
                    "absent build-support entry must remain absent"
                );
            } else {
                assert_eq!(
                    fs::read(destination_dir.join("libllama-common.so"))
                        .expect("repaired upstream build-support link"),
                    b"common"
                );
                assert!(
                    !fs::symlink_metadata(destination_dir.join("libllama-common.so"))
                        .expect("build-support metadata")
                        .file_type()
                        .is_symlink(),
                    "upstream build-support link must resolve to a regular file"
                );
            }
        }

        fs::remove_file(core_dir.join("libllama-common.so.0.0.0"))
            .expect("replace build-support source inode");
        fs::write(core_dir.join("libllama-common.so.0.0.0"), b"common-v2")
            .expect("updated build-support library");
        for destination_dir in [profile_dir.join("deps"), profile_dir.join("examples")] {
            assert_eq!(
                fs::read(destination_dir.join("libllama-common.so"))
                    .expect("stale build-support entry"),
                b"common"
            );
        }

        stage_linux_shared_libraries(
            &runtime_sources,
            &[build_support_source.as_path()],
            &profile_dir,
        )
        .expect("repeated staging remains idempotent");
        assert_runtime_sources_unchanged(&runtime_sources);
        assert!(
            fs::symlink_metadata(profile_dir.join("libllama-common.so")).is_err(),
            "repeated staging must not introduce absent build support"
        );
        for destination_dir in [profile_dir.join("deps"), profile_dir.join("examples")] {
            assert_eq!(
                fs::read(destination_dir.join("libllama-common.so"))
                    .expect("refreshed build-support entry"),
                b"common-v2"
            );
        }
    }

    fn assert_runtime_sources_unchanged(runtime_sources: &[&Path]) {
        let expected: [&[u8]; 5] = [b"ggml", b"ggml", b"ggml", b"cpu", b"vulkan"];
        for (source, expected) in runtime_sources.iter().zip(expected) {
            assert_eq!(
                fs::read(source).expect("upstream runtime source remains readable"),
                expected,
                "staging mutated upstream runtime source {}",
                source.display()
            );
        }
    }

    #[test]
    fn rejects_a_directory_at_a_library_destination() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let core_dir = temp.path().join("native/lib");
        let profile_dir = temp.path().join("target/debug");
        fs::create_dir_all(&core_dir).expect("core directory");
        fs::write(core_dir.join("libllama.so"), b"llama").expect("shared library");
        fs::create_dir_all(profile_dir.join("libllama.so")).expect("conflicting directory");

        let source = core_dir.join("libllama.so");
        let error = stage_linux_shared_libraries(&[source.as_path()], &[], &profile_dir)
            .expect_err("directory collision must fail closed");
        assert_eq!(error.kind(), io::ErrorKind::IsADirectory);
    }
}
