use std::process::Command;

#[test]
fn test_cli_help() {
    let bin_path = env!("CARGO_BIN_EXE_cargo-esp32p4-pre-v3");

    // Test with cargo-injected command
    let mut cmd = Command::new(bin_path);
    cmd.arg("esp32p4-pre-v3").arg("--help");

    let output = cmd.output().expect("Failed to execute CLI");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "Command failed with: {}\nstdout: {}",
        stderr,
        stdout
    );

    let help_text = if !stdout.is_empty() { stdout } else { stderr };
    assert!(
        help_text.contains("prepare"),
        "Help output missing 'prepare' command: {}",
        help_text
    );
    assert!(
        help_text.contains("--help"),
        "Help output missing '--help' flag: {}",
        help_text
    );
}

#[test]
fn test_cli_prepare_existing_dot_and_dotdot_preserves_content() {
    let bin_path = env!("CARGO_BIN_EXE_cargo-esp32p4-pre-v3");

    fn snapshot(
        dir: &std::path::Path,
    ) -> std::collections::BTreeMap<std::path::PathBuf, Option<Vec<u8>>> {
        let mut state = std::collections::BTreeMap::new();
        let mut stack = vec![dir.to_path_buf()];
        while let Some(current) = stack.pop() {
            for entry in std::fs::read_dir(&current).unwrap() {
                let entry = entry.unwrap();
                let path = entry.path();
                let file_type = entry.file_type().unwrap();
                let rel_path = path.strip_prefix(dir).unwrap().to_path_buf();
                if file_type.is_dir() {
                    state.insert(rel_path, None);
                    stack.push(path);
                } else {
                    state.insert(rel_path, Some(std::fs::read(&path).unwrap()));
                }
            }
        }
        state
    }

    for arg in [".", ".."] {
        let temp = tempfile::TempDir::new().unwrap();
        let parent = temp.path().join("parent");
        std::fs::create_dir(&parent).unwrap();
        let workspace_child = parent.join("child");
        std::fs::create_dir(&workspace_child).unwrap();

        std::fs::write(parent.join("sentinel.txt"), b"content_parent").unwrap();
        let nested_parent = parent.join("nested_parent");
        std::fs::create_dir(&nested_parent).unwrap();
        std::fs::write(nested_parent.join("inner.txt"), b"inner_parent").unwrap();

        std::fs::write(workspace_child.join("sentinel.txt"), b"content_child").unwrap();
        let nested_child = workspace_child.join("nested_child");
        std::fs::create_dir(&nested_child).unwrap();
        std::fs::write(nested_child.join("inner.txt"), b"inner_child").unwrap();

        let before = snapshot(temp.path());

        let mut cmd = Command::new(bin_path);
        cmd.current_dir(&workspace_child)
            .arg("esp32p4-pre-v3")
            .arg("prepare")
            .arg(arg);

        let output = cmd.output().expect("Failed to execute CLI");

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        assert_eq!(
            output.status.code(),
            Some(1),
            "Expected exit code 1 for '{}' with existing content.\nstdout:\n{}\nstderr:\n{}",
            arg,
            stdout,
            stderr
        );

        let after = snapshot(temp.path());
        assert_eq!(
            before, after,
            "Filesystem modified during prepare '{}'!\nstdout:\n{}\nstderr:\n{}",
            arg, stdout, stderr
        );
    }
}

#[test]
fn test_cli_direct_help() {
    let bin_path = env!("CARGO_BIN_EXE_cargo-esp32p4-pre-v3");

    // Test direct execution
    let mut cmd = Command::new(bin_path);
    cmd.arg("--help");

    let output = cmd.output().expect("Failed to execute CLI");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "Command failed with: {}\nstdout: {}",
        stderr,
        stdout
    );

    let help_text = if !stdout.is_empty() { stdout } else { stderr };
    assert!(
        help_text.contains("prepare"),
        "Help output missing 'prepare' command: {}",
        help_text
    );
    assert!(
        help_text.contains("--help"),
        "Help output missing '--help' flag: {}",
        help_text
    );
}
