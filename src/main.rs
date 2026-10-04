use same_file::Handle;

use std::env;
use std::fs;
use std::path::Path;
use std::process::{Command, ExitCode};

use serde::Deserialize;
use sha2::{Digest, Sha256};
use tempfile::Builder;

const MANIFEST_JSON: &str = include_str!("../patches/manifest.json");
const PATCH_BYTES: &[u8] = include_bytes!("../patches/esp32p4-pre-v3.patch");

#[derive(Deserialize, Debug)]
struct ManifestUpstream {
    repository: String,
    revision: String,
}

#[derive(Deserialize, Debug)]
struct ManifestPatch {
    #[serde(rename = "path")]
    _path: String,
    sha256: String,
}
#[derive(Deserialize, Debug)]
struct Manifest {
    upstream: ManifestUpstream,
    patch: ManifestPatch,
    cargo_patches: Vec<String>,
}

fn run_command(step: &str, cmd: &mut Command) -> Result<String, String> {
    let result = cmd
        .output()
        .map_err(|e| format!("{} failed to start: {}", step, e))?;
    if !result.status.success() {
        let stderr = String::from_utf8_lossy(&result.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&result.stdout).trim().to_string();
        let detail = if !stderr.is_empty() { stderr } else { stdout };
        let suffix = if !detail.is_empty() {
            format!(": {}", detail)
        } else {
            String::new()
        };
        return Err(format!(
            "{} failed (exit {}){}",
            step,
            result.status.code().unwrap_or(1),
            suffix
        ));
    }
    Ok(String::from_utf8_lossy(&result.stdout).trim().to_string())
}

pub fn prepare(destination: &Path, manifest_json: &str, patch_bytes: &[u8]) -> Result<(), String> {
    let dest_abs = if destination.is_absolute() {
        destination.to_path_buf()
    } else {
        env::current_dir()
            .map_err(|e| e.to_string())?
            .join(destination)
    };

    if fs::symlink_metadata(&dest_abs).is_ok() {
        return Err(format!(
            "destination already exists: {}",
            dest_abs.display()
        ));
    }

    let dest_parent = dest_abs.parent().ok_or_else(|| {
        format!(
            "destination parent does not exist or is not a directory: {}",
            dest_abs.display()
        )
    })?;

    if !dest_parent.is_dir() {
        return Err(format!(
            "destination parent does not exist or is not a directory: {}",
            dest_parent.display()
        ));
    }

    let manifest: Manifest = serde_json::from_str(manifest_json)
        .map_err(|e| format!("read patch plan or patch failed: {}", e))?;

    let expected_sha256 = manifest.patch.sha256;
    let mut hasher = Sha256::new();
    hasher.update(patch_bytes);
    let actual_sha256 = format!("{:x}", hasher.finalize());

    if actual_sha256 != expected_sha256 {
        return Err(format!(
            "patch SHA256 mismatch: expected {}, got {}",
            expected_sha256, actual_sha256
        ));
    }

    for pkg in &manifest.cargo_patches {
        if pkg.is_empty()
            || pkg == "."
            || pkg == ".."
            || Path::new(pkg).file_name().and_then(|n| n.to_str()) != Some(pkg.as_str())
        {
            return Err(
                "read patch plan failed: cargo_patches must be package directory names".to_string(),
            );
        }
    }

    let prefix = format!(
        ".{}.prepare-",
        dest_abs.file_name().unwrap().to_string_lossy()
    );
    let temp_dir = Builder::new()
        .prefix(&prefix)
        .tempdir_in(dest_parent)
        .map_err(|e| format!("create staging area failed: {}", e))?;
    let staging = temp_dir.path();

    let staged_patch = staging.join("verified.patch");
    fs::write(&staged_patch, patch_bytes)
        .map_err(|e| format!("stage verified patch failed: {}", e))?;

    let upstream = staging.join("upstream");
    run_command(
        "initialize temporary checkout",
        Command::new("git")
            .arg("init")
            .arg("--quiet")
            .arg(&upstream),
    )?;
    run_command(
        "configure upstream remote",
        Command::new("git")
            .current_dir(&upstream)
            .arg("remote")
            .arg("add")
            .arg("origin")
            .arg(&manifest.upstream.repository),
    )?;
    run_command(
        "fetch pinned revision",
        Command::new("git")
            .current_dir(&upstream)
            .arg("fetch")
            .arg("--no-tags")
            .arg("--depth=1")
            .arg("origin")
            .arg(&manifest.upstream.revision),
    )?;
    run_command(
        "checkout pinned revision",
        Command::new("git")
            .current_dir(&upstream)
            .arg("checkout")
            .arg("--quiet")
            .arg("--detach")
            .arg("FETCH_HEAD"),
    )?;

    let head = run_command(
        "verify pinned revision",
        Command::new("git")
            .current_dir(&upstream)
            .arg("rev-parse")
            .arg("HEAD"),
    )?;
    if head != manifest.upstream.revision {
        return Err(format!(
            "verify pinned revision failed: expected {}, got {}",
            manifest.upstream.revision, head
        ));
    }

    run_command(
        "check patch application",
        Command::new("git")
            .current_dir(&upstream)
            .arg("apply")
            .arg("--check")
            .arg(&staged_patch),
    )?;
    run_command(
        "apply source patch",
        Command::new("git")
            .current_dir(&upstream)
            .arg("apply")
            .arg(&staged_patch),
    )?;

    for pkg in &manifest.cargo_patches {
        if !upstream.join(pkg).is_dir() {
            return Err(format!(
                "verify patched checkout failed: missing package directory {}",
                pkg
            ));
        }
    }

    let candidate = staging.join("prepared");
    fs::create_dir(&candidate).map_err(|e| format!("stage prepared checkout failed: {}", e))?;
    fs::rename(&upstream, candidate.join("upstream"))
        .map_err(|e| format!("stage prepared checkout failed: {}", e))?;

    let mut patch_lines = vec!["[patch.crates-io]".to_string()];
    for pkg in &manifest.cargo_patches {
        let pkg_path = dest_abs.join("upstream").join(pkg);
        let pkg_path_str = pkg_path
            .to_str()
            .ok_or_else(|| format!("unrepresentable TOML path for package {}", pkg))?;
        patch_lines.push(format!(
            "{} = {{ path = {} }}",
            serde_json::to_string(pkg).unwrap(),
            serde_json::to_string(pkg_path_str).unwrap()
        ));
    }

    fs::write(
        candidate.join("Cargo.patch.toml"),
        patch_lines.join("\n") + "\n",
    )
    .map_err(|e| format!("write Cargo.patch.toml failed: {}", e))?;

    if let Err(e) = fs::create_dir(&dest_abs) {
        if e.kind() == std::io::ErrorKind::AlreadyExists {
            return Err(format!(
                "destination already exists: {}",
                dest_abs.display()
            ));
        } else {
            return Err(format!("create destination failed: {}", e));
        }
    }

    let dest_handle = Handle::from_path(&dest_abs).map_err(|e| {
        format!(
            "publish prepared checkout failed: cannot get handle for created dir: {}",
            e
        )
    })?;

    if let Err(e) = fs::rename(&candidate, &dest_abs) {
        if Handle::from_path(&dest_abs).is_ok_and(|h| h == dest_handle) {
            let _ = fs::remove_dir(&dest_abs);
        }
        return Err(format!("publish prepared checkout failed: {}", e));
    }

    println!(
        "Prepared patched upstream source at {}",
        dest_abs.join("upstream").display()
    );
    println!(
        "Add the path replacements in {} to your workspace Cargo.toml [patch.crates-io] section.",
        dest_abs.join("Cargo.patch.toml").display()
    );
    Ok(())
}
fn print_help() {
    println!("usage: cargo esp32p4-pre-v3 <prepare DEST|--help|-h|--version|-V>");
    println!();
    println!("Prepare the pinned, patched upstream HAL checkout for a consumer workspace.");
}

pub fn run_cli(mut args: impl Iterator<Item = std::ffi::OsString>) -> ExitCode {
    args.next(); // Skip argv[0]
    let mut first_arg = args.next();
    if first_arg.as_ref().is_some_and(|a| a == "esp32p4-pre-v3") {
        first_arg = args.next();
    }

    let Some(command) = first_arg else {
        eprintln!("usage: cargo esp32p4-pre-v3 <prepare DEST|--help|-h|--version|-V>");
        return ExitCode::from(2);
    };

    if command == "--help" || command == "-h" {
        print_help();
        ExitCode::SUCCESS
    } else if command == "--version" || command == "-V" {
        println!("cargo-esp32p4-pre-v3 {}", env!("CARGO_PKG_VERSION"));
        ExitCode::SUCCESS
    } else if command == "prepare" {
        let mut dest: Option<std::ffi::OsString> = None;
        let mut positional_only = false;
        for arg in args {
            if positional_only {
                if dest.is_some() {
                    eprintln!("usage: cargo esp32p4-pre-v3 prepare DEST");
                    return ExitCode::from(2);
                }
                dest = Some(arg);
                continue;
            }

            if arg == "--help" || arg == "-h" {
                print_help();
                return ExitCode::SUCCESS;
            }

            if arg == "--" {
                positional_only = true;
                continue;
            }

            if arg.as_encoded_bytes().starts_with(b"-") {
                eprintln!("unknown flag: {}", arg.to_string_lossy());
                eprintln!("usage: cargo esp32p4-pre-v3 prepare DEST");
                return ExitCode::from(2);
            }

            if dest.is_some() {
                eprintln!("usage: cargo esp32p4-pre-v3 prepare DEST");
                return ExitCode::from(2);
            }
            dest = Some(arg);
        }

        let Some(dest_os) = dest else {
            eprintln!("usage: cargo esp32p4-pre-v3 prepare DEST");
            return ExitCode::from(2);
        };

        let dest_path = Path::new(&dest_os);
        if let Err(e) = prepare(dest_path, MANIFEST_JSON, PATCH_BYTES) {
            eprintln!("{}", e);
            ExitCode::FAILURE
        } else {
            ExitCode::SUCCESS
        }
    } else {
        eprintln!("unknown command: {}", command.to_string_lossy());
        eprintln!("usage: cargo esp32p4-pre-v3 <prepare DEST|--help|-h|--version|-V>");
        ExitCode::from(2)
    }
}

fn main() -> ExitCode {
    run_cli(std::env::args_os())
}
#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};
    use std::ffi::OsString;
    use std::fs;
    use tempfile::Builder;
    use tempfile::TempDir;

    fn run_git(args: &[&str], dir: &Path) {
        let mut cmd = Command::new("git");
        cmd.current_dir(dir).args(args);
        let output = cmd.output().expect("git failed to run");
        assert!(
            output.status.success(),
            "git {:?} failed\nstdout: {}\nstderr: {}",
            args,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn setup_mock_source(root: &Path) -> (String, Vec<u8>, String) {
        let src_dir = root.join("source");
        fs::create_dir(&src_dir).unwrap();
        run_git(&["init", "--quiet"], &src_dir);

        let pkgs = [
            ("esp-hal", "1.1.0"),
            ("esp-backtrace", "0.19.0"),
            ("esp-bootloader-esp-idf", "0.5.0"),
            ("esp-println", "0.17.0"),
        ];
        for (pkg, ver) in &pkgs {
            let pkg_dir = src_dir.join(pkg);
            fs::create_dir(&pkg_dir).unwrap();
            let manifest = format!("[package]\nname = \"{}\"\nversion = \"{}\"\n", pkg, ver);
            fs::write(pkg_dir.join("Cargo.toml"), manifest).unwrap();
            fs::create_dir(pkg_dir.join("src")).unwrap();
            fs::write(pkg_dir.join("src/lib.rs"), "pub fn old() {}\n").unwrap();
        }

        run_git(&["add", "."], &src_dir);
        run_git(&["config", "user.name", "Test"], &src_dir);
        run_git(&["config", "user.email", "test@example.com"], &src_dir);
        run_git(&["commit", "-m", "init"], &src_dir);

        let rev = String::from_utf8(
            Command::new("git")
                .current_dir(&src_dir)
                .arg("rev-parse")
                .arg("HEAD")
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap()
        .trim()
        .to_string();

        fs::write(src_dir.join("esp-hal/src/lib.rs"), "pub fn new() {}\n").unwrap();
        fs::write(src_dir.join("esp-hal/binary.bin"), b"\x00\x01\x02\x03").unwrap();
        run_git(&["add", "."], &src_dir);
        let diff = Command::new("git")
            .current_dir(&src_dir)
            .arg("diff")
            .arg("--cached")
            .arg("--binary")
            .output()
            .unwrap()
            .stdout;

        let expected_tree = String::from_utf8(
            Command::new("git")
                .current_dir(&src_dir)
                .arg("write-tree")
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap()
        .trim()
        .to_string();

        run_git(&["reset", "--hard", "HEAD"], &src_dir);

        (rev, diff, expected_tree)
    }

    fn make_manifest(repo: &str, rev: &str, patch_sha: &str, pkgs: &[&str]) -> String {
        let m = serde_json::json!({
            "upstream": {
                "repository": repo,
                "revision": rev,
            },
            "patch": {
                "path": "test.patch",
                "sha256": patch_sha
            },
            "cargo_patches": pkgs
        });
        serde_json::to_string(&m).unwrap()
    }

    fn assert_cargo_metadata_resolves_paths(dest: &Path, pkgs: &[(&str, &str)]) {
        let mut deps = String::new();
        for (pkg, ver) in pkgs {
            deps.push_str(&format!("{} = \"={}\"\n", pkg, ver));
        }
        let patch_toml = fs::read_to_string(dest.join("Cargo.patch.toml")).unwrap();
        let consumer_toml = format!(
            "[package]\nname = \"consumer\"\nversion = \"0.1.0\"\n[workspace]\n[dependencies]\n{}\n{}\n",
            deps, patch_toml
        );

        fs::write(dest.join("Cargo.toml"), consumer_toml).unwrap();
        fs::create_dir(dest.join("src")).unwrap();
        fs::write(dest.join("src/main.rs"), "fn main() {}\n").unwrap();

        let metadata_out = Command::new("cargo")
            .current_dir(dest)
            .args(["metadata", "--offline", "--format-version=1"])
            .output()
            .unwrap();
        assert!(
            metadata_out.status.success(),
            "cargo metadata failed with generated Cargo.patch.toml\nstderr: {}",
            String::from_utf8_lossy(&metadata_out.stderr)
        );

        let meta_str = String::from_utf8(metadata_out.stdout).unwrap();
        let meta_json: serde_json::Value = serde_json::from_str(&meta_str).unwrap();
        let pkgs_array = meta_json["packages"].as_array().unwrap();

        for (pkg, _) in pkgs {
            let pkg_obj = pkgs_array.iter().find(|p| p["name"] == *pkg).unwrap();
            let manifest_path = pkg_obj["manifest_path"].as_str().unwrap();

            let expected_path =
                fs::canonicalize(dest.join("upstream").join(pkg).join("Cargo.toml")).unwrap();
            let actual_path = fs::canonicalize(Path::new(manifest_path)).unwrap();
            assert_eq!(
                actual_path, expected_path,
                "manifest_path for {} did not resolve to prepared override",
                pkg
            );
        }
    }

    #[test]
    fn test_successful_prepare() {
        let temp = TempDir::new().unwrap();
        let (rev, patch_bytes, expected_tree) = setup_mock_source(temp.path());
        let src_url = format!("file://{}", temp.path().join("source").display());

        let mut hasher = Sha256::new();
        hasher.update(&patch_bytes);
        let hash = format!("{:x}", hasher.finalize());

        let pkgs = [
            ("esp-hal", "1.1.0"),
            ("esp-backtrace", "0.19.0"),
            ("esp-bootloader-esp-idf", "0.5.0"),
            ("esp-println", "0.17.0"),
        ];
        let pkg_names: Vec<&str> = pkgs.iter().map(|(n, _)| *n).collect();
        let manifest_json = make_manifest(&src_url, &rev, &hash, &pkg_names);
        let dest = temp.path().join("dest");
        let result = prepare(&dest, &manifest_json, &patch_bytes);

        assert!(result.is_ok(), "prepare failed: {:?}", result.err());
        assert!(dest.join("upstream/esp-hal/src/lib.rs").exists());
        let content = fs::read_to_string(dest.join("upstream/esp-hal/src/lib.rs")).unwrap();
        assert!(content.contains("fn new"));

        let bin_content = fs::read(dest.join("upstream/esp-hal/binary.bin")).unwrap();
        assert_eq!(bin_content, b"\x00\x01\x02\x03");

        assert!(dest.join("Cargo.patch.toml").exists());

        let upstream = dest.join("upstream");
        let prep_head = String::from_utf8(
            Command::new("git")
                .current_dir(&upstream)
                .arg("rev-parse")
                .arg("HEAD")
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap()
        .trim()
        .to_string();
        assert_eq!(prep_head, rev, "prepared HEAD does not match pinned rev");

        run_git(&["add", "--all"], &upstream);
        let prep_tree = String::from_utf8(
            Command::new("git")
                .current_dir(&upstream)
                .arg("write-tree")
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap()
        .trim()
        .to_string();
        assert_eq!(
            prep_tree, expected_tree,
            "patched write-tree does not match expected"
        );

        assert_cargo_metadata_resolves_paths(&dest, &pkgs);
    }

    #[test]
    #[cfg(unix)]
    fn test_patch_path_escaping() {
        let temp = TempDir::new().unwrap();
        let (rev, patch_bytes, _) = setup_mock_source(temp.path());
        let src_url = format!("file://{}", temp.path().join("source").display());

        let mut hasher = Sha256::new();
        hasher.update(&patch_bytes);
        let hash = format!("{:x}", hasher.finalize());

        let pkgs = [
            ("esp-hal", "1.1.0"),
            ("esp-backtrace", "0.19.0"),
            ("esp-bootloader-esp-idf", "0.5.0"),
            ("esp-println", "0.17.0"),
        ];
        let pkg_names: Vec<&str> = pkgs.iter().map(|(n, _)| *n).collect();
        let manifest_json = make_manifest(&src_url, &rev, &hash, &pkg_names);

        let dest = temp.path().join("dest space \u{1F980} \" \\ \n \t");
        let result = prepare(&dest, &manifest_json, &patch_bytes);
        assert!(
            result.is_ok(),
            "prepare failed with escaped dest path: {:?}",
            result.err()
        );

        assert_cargo_metadata_resolves_paths(&dest, &pkgs);
    }

    #[test]
    fn test_invalid_package_directory_names() {
        let temp = TempDir::new().unwrap();
        let (rev, patch_bytes, _) = setup_mock_source(temp.path());
        let src_url = format!("file://{}", temp.path().join("source").display());

        let mut hasher = Sha256::new();
        hasher.update(&patch_bytes);
        let hash = format!("{:x}", hasher.finalize());

        let bad_names = ["", ".", "..", std::path::MAIN_SEPARATOR_STR, "esp-hal/"];

        for (dest_id, bad_name) in bad_names.into_iter().enumerate() {
            let dest = temp.path().join(format!("dest_{}", dest_id));
            let manifest_json = make_manifest(&src_url, &rev, &hash, &[bad_name]);

            let mut before_entries: Vec<_> = fs::read_dir(temp.path())
                .unwrap()
                .map(|e| e.unwrap().file_name())
                .collect();
            before_entries.sort();

            let result = prepare(&dest, &manifest_json, &patch_bytes);
            assert!(result.is_err());
            assert!(!dest.exists());

            let mut after_entries: Vec<_> = fs::read_dir(temp.path())
                .unwrap()
                .map(|e| e.unwrap().file_name())
                .collect();
            after_entries.sort();

            assert_eq!(before_entries, after_entries);
        }
    }

    #[test]
    fn test_bad_patch_digest() {
        let temp = TempDir::new().unwrap();
        let dest = temp.path().join("dest");
        let manifest_json = make_manifest("repo", "rev", "badhash", &["esp-hal"]);

        let result = prepare(&dest, &manifest_json, b"patch");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("patch SHA256 mismatch"));
        assert!(!dest.exists());
    }

    #[test]
    fn test_fetch_failure() {
        let temp = TempDir::new().unwrap();
        let (_, patch_bytes, _) = setup_mock_source(temp.path());
        let src_url = format!("file://{}", temp.path().join("source").display());

        let mut hasher = Sha256::new();
        hasher.update(&patch_bytes);
        let hash = format!("{:x}", hasher.finalize());

        let manifest_json = make_manifest(
            &src_url,
            "0000000000000000000000000000000000000000",
            &hash,
            &["esp-hal"],
        );

        let dest = temp.path().join("dest");
        let result = prepare(&dest, &manifest_json, &patch_bytes);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("fetch pinned revision failed"));
        assert!(!dest.exists());
    }

    #[test]
    fn test_wrong_pinned_revision() {
        let temp = TempDir::new().unwrap();
        let (_, patch_bytes, _) = setup_mock_source(temp.path());

        let src_url = format!("file://{}", temp.path().join("source").display());
        let mut hasher = Sha256::new();
        hasher.update(&patch_bytes);
        let hash = format!("{:x}", hasher.finalize());

        // Using symbolic "HEAD" fetches correctly, but verify-pin checks literal SHA vs "HEAD" and fails.
        let manifest_json = make_manifest(&src_url, "HEAD", &hash, &["esp-hal"]);
        let dest = temp.path().join("dest");
        let result = prepare(&dest, &manifest_json, &patch_bytes);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .contains("verify pinned revision failed: expected HEAD, got")
        );
    }

    #[test]
    fn test_failed_patch_application() {
        let temp = TempDir::new().unwrap();
        let (rev, _, _) = setup_mock_source(temp.path());
        let src_url = format!("file://{}", temp.path().join("source").display());

        let bad_patch_bytes = b"garbage patch";
        let mut hasher = Sha256::new();
        hasher.update(bad_patch_bytes);
        let hash = format!("{:x}", hasher.finalize());

        let manifest_json = make_manifest(&src_url, &rev, &hash, &["esp-hal"]);

        let dest = temp.path().join("dest");
        let result = prepare(&dest, &manifest_json, bad_patch_bytes);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .contains("check patch application failed")
        );
        assert!(!dest.exists());
    }

    #[test]
    fn test_missing_package_dir() {
        let temp = TempDir::new().unwrap();
        let (rev, patch_bytes, _) = setup_mock_source(temp.path());

        let src_url = format!("file://{}", temp.path().join("source").display());
        let mut hasher = Sha256::new();
        hasher.update(&patch_bytes);
        let hash = format!("{:x}", hasher.finalize());

        let manifest_json = make_manifest(&src_url, &rev, &hash, &["esp-hal", "non-existent-pkg"]);

        let dest = temp.path().join("dest");
        let result = prepare(&dest, &manifest_json, &patch_bytes);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains(
            "verify patched checkout failed: missing package directory non-existent-pkg"
        ));
        assert!(!dest.exists());
    }

    #[test]
    fn test_existing_directory_and_dangling_symlink() {
        let temp = TempDir::new().unwrap();
        let (rev, patch_bytes, _) = setup_mock_source(temp.path());
        let src_url = format!("file://{}", temp.path().join("source").display());

        let mut hasher = Sha256::new();
        hasher.update(&patch_bytes);
        let hash = format!("{:x}", hasher.finalize());

        let manifest_json = make_manifest(&src_url, &rev, &hash, &["esp-hal"]);

        let dest = temp.path().join("dest");
        fs::create_dir(&dest).unwrap();
        let sentinel_path = dest.join("sentinel.txt");
        fs::write(&sentinel_path, "do not overwrite").unwrap();

        let old_patch_toml = dest.join("Cargo.patch.toml");
        fs::write(&old_patch_toml, "old toml").unwrap();

        let upstream_dir = dest.join("upstream");
        fs::create_dir(&upstream_dir).unwrap();
        let keep_bin = upstream_dir.join("keep.bin");
        fs::write(&keep_bin, b"bin").unwrap();

        let result = prepare(&dest, &manifest_json, &patch_bytes);
        assert!(result.is_err());
        assert_eq!(
            fs::read_to_string(&sentinel_path).unwrap(),
            "do not overwrite"
        );
        assert_eq!(fs::read_to_string(&old_patch_toml).unwrap(), "old toml");
        assert_eq!(fs::read(&keep_bin).unwrap(), b"bin");

        let symlink_dest = temp.path().join("symlink");
        #[cfg(unix)]
        std::os::unix::fs::symlink("nowhere", &symlink_dest).unwrap();
        #[cfg(windows)]
        std::os::windows::fs::symlink_dir("nowhere", &symlink_dest).unwrap();

        let result = prepare(&symlink_dest, &manifest_json, &patch_bytes);
        assert!(result.is_err());

        let meta = fs::symlink_metadata(&symlink_dest).unwrap();
        assert!(meta.is_symlink());
        assert_eq!(fs::read_link(&symlink_dest).unwrap(), Path::new("nowhere"));
    }

    #[test]
    fn test_parent_absence() {
        let temp = TempDir::new().unwrap();
        let dest = temp.path().join("missing/dest");
        let manifest_json = make_manifest("repo", "rev", "hash", &[]);

        let result = prepare(&dest, &manifest_json, b"");
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .contains("destination parent does not exist or is not a directory")
        );
    }

    #[test]
    fn test_relative_cwd() {
        let cwd = env::current_dir().unwrap();
        let temp = Builder::new().tempdir_in(&cwd).unwrap();
        let (rev, patch_bytes, _) = setup_mock_source(temp.path());

        let src_url = format!("file://{}", temp.path().join("source").display());
        let mut hasher = Sha256::new();
        hasher.update(&patch_bytes);
        let hash = format!("{:x}", hasher.finalize());

        let manifest_json = make_manifest(&src_url, &rev, &hash, &["esp-hal"]);

        let rel_dest = Path::new(temp.path().file_name().unwrap()).join("dest");
        let result = prepare(&rel_dest, &manifest_json, &patch_bytes);

        assert!(result.is_ok(), "prepare failed: {:?}", result.err());
        assert!(
            temp.path()
                .join("dest/upstream/esp-hal/src/lib.rs")
                .exists()
        );
    }

    #[test]
    fn test_cli_parsing() {
        let args = vec![
            OsString::from("cargo"),
            OsString::from("esp32p4-pre-v3"),
            OsString::from("prepare"),
            OsString::from("--help"),
        ];
        assert_eq!(run_cli(args.into_iter()), ExitCode::SUCCESS);

        let args = vec![
            OsString::from("cargo"),
            OsString::from("prepare"),
            OsString::from("--help"),
        ];
        assert_eq!(run_cli(args.into_iter()), ExitCode::SUCCESS);

        let args = vec![
            OsString::from("cargo"),
            OsString::from("esp32p4-pre-v3"),
            OsString::from("prepare"),
            OsString::from("-invalid"),
        ];
        assert_eq!(run_cli(args.into_iter()), ExitCode::from(2));
    }

    #[cfg(unix)]
    #[test]
    fn test_cli_parsing_non_utf8() {
        use std::os::unix::ffi::OsStringExt;
        let temp = TempDir::new().unwrap();
        let destination = temp
            .path()
            .join("missing-parent")
            .join(OsString::from_vec(vec![0xff, 0xfe, 0xfd]));
        let args = vec![
            OsString::from("cargo"),
            OsString::from("prepare"),
            destination.into_os_string(),
        ];
        // Reject the missing parent without fetching, while accepting non-UTF8 argv.
        assert_eq!(run_cli(args.into_iter()), ExitCode::FAILURE);
    }
}
