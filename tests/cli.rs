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
