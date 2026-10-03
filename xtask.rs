use std::process::{Command, ExitCode};

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let Some(command) = args.next() else {
        eprintln!("usage: cargo xtask <prepare DEST|host-tests|fmt-packages|lint-packages>");
        return ExitCode::from(2);
    };

    let root = env!("CARGO_MANIFEST_DIR");
    let result = match command.as_str() {
        "prepare" => Command::new("python3")
            .arg(std::path::Path::new(root).join("prepare.py"))
            .args(args)
            .status(),
        "host-tests" => Command::new("python3")
            .current_dir(root)
            .args(["-m", "unittest", "discover", "-s", "tests"])
            .args(args)
            .status(),
        "fmt-packages" => Command::new("cargo")
            .current_dir(root)
            .arg("fmt")
            .args(args)
            .status(),
        "lint-packages" => Command::new("cargo")
            .current_dir(root)
            .args(["clippy", "--all-targets"])
            .args(args)
            .args(["--", "-D", "warnings"])
            .status(),
        _ => {
            eprintln!("unknown xtask command: {command}");
            eprintln!("usage: cargo xtask <prepare DEST|host-tests|fmt-packages|lint-packages>");
            return ExitCode::from(2);
        }
    };

    match result {
        Ok(status) if status.success() => ExitCode::SUCCESS,
        Ok(status) => ExitCode::from(status.code().unwrap_or(1) as u8),
        Err(error) => {
            eprintln!("xtask: failed to start {command}: {error}");
            ExitCode::FAILURE
        }
    }
}
