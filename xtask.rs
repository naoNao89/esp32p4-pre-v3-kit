use std::env;
use std::process::{Command, ExitCode};

fn find_python() -> std::io::Result<Command> {
    if let Ok(py) = env::var("PYTHON") {
        return Ok(Command::new(py));
    }
    let candidates = ["python3", "python"];
    for cmd in candidates {
        let mut c = Command::new(cmd);
        c.arg("-c")
            .arg("import sys; sys.exit(0 if sys.version_info >= (3,) else 1)");
        match c.status() {
            Ok(status) if status.success() => return Ok(Command::new(cmd)),
            Ok(_) => continue, // Found but not suitable (e.g. Python 2)
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e), // Preserve real command failures
        }
    }
    let mut c = Command::new("py");
    c.arg("-3").arg("-c").arg("pass");
    match c.status() {
        Ok(status) if status.success() => {
            let mut cmd = Command::new("py");
            cmd.arg("-3");
            return Ok(cmd);
        }
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::NotFound,
        "No suitable Python 3 interpreter found. Set PYTHON or install python3.",
    ))
}

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let Some(command) = args.next() else {
        eprintln!("usage: cargo xtask <prepare DEST|host-tests|fmt-packages|lint-packages>");
        return ExitCode::from(2);
    };

    let root = env!("CARGO_MANIFEST_DIR");
    let result = match command.as_str() {
        "prepare" => find_python().and_then(|mut cmd| {
            cmd.arg(std::path::Path::new(root).join("prepare.py"))
                .args(args)
                .status()
        }),
        "host-tests" => find_python().and_then(|mut cmd| {
            cmd.current_dir(root)
                .args(["-m", "unittest", "discover", "-s", "tests"])
                .args(args)
                .status()
        }),
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
