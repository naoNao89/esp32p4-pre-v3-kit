//! Real, networked, registry-only proof (no hardware and no upload).
//!
//! Run: cargo test --test distribution_registry test_registry_distribution_proof -- --ignored --exact --nocapture
//! Optional frozen inputs: ESP_P4_PROOF_PREPARED=/absolute/prepared
//! ESP_P4_PROOF_METADATA=/absolute/metadata.json (set both). Otherwise the CLI
//! prepares a fresh pinned checkout; no old baseline is silently reused.
//! Requires python3, tar, network access, and the installed RISC-V Rust target.
//! Evidence and command logs survive errors under target/distribution-validation/registry-proof-*.

use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const TARGET: &str = "riscv32imafc-unknown-none-elf";
const FIRMWARE: &str = r#"#![no_std]
#![no_main]
#[esp_hal::main]
fn main() -> ! {
    let peripherals = esp_hal::init(esp_hal::Config::default().with_cpu_clock(esp_hal::clock::CpuClock::_90MHz));
    core::hint::black_box(peripherals);
    loop { core::hint::spin_loop(); }
}
#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { loop { core::hint::spin_loop(); } }
"#;
const MACRO_COVERAGE: &str = r#"
#[esp_hal::handler]
fn proof_handler() {}
#[esp_hal::ram(unstable(rtc_fast, zeroed))]
static PROOF_RTC_ZEROED: u32 = 0;
"#;

// Every child gets its own process group, including Cargo's rustc/build-script
// descendants. Unwinding and deadlines terminate the entire group.
struct Process(Child);
unsafe extern "C" {
    fn kill(pid: i32, signal: i32) -> i32;
}
impl Drop for Process {
    fn drop(&mut self) {
        unsafe {
            kill(-(self.0.id() as i32), 9);
        }
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

struct Proof {
    evidence: PathBuf,
    sequence: usize,
}
impl Proof {
    fn command(&mut self, label: &str, command: &mut Command) -> Output {
        self.sequence += 1;
        let prefix = self.evidence.join(format!("{:03}-{label}", self.sequence));
        fs::write(
            prefix.with_extension("command.txt"),
            format!("{command:?}\n"),
        )
        .unwrap();
        let stdout_path = prefix.with_extension("stdout.log");
        let stderr_path = prefix.with_extension("stderr.log");
        command
            .stdin(Stdio::null())
            .stdout(fs::File::create(&stdout_path).unwrap())
            .stderr(fs::File::create(&stderr_path).unwrap())
            .process_group(0);
        let mut process = Process(
            command
                .spawn()
                .unwrap_or_else(|error| panic!("{label}: {error}")),
        );
        let start = Instant::now();
        let status = loop {
            if let Some(status) = process.0.try_wait().unwrap() {
                break status;
            }
            if start.elapsed() > Duration::from_secs(1800) {
                fs::write(
                    prefix.with_extension("status.txt"),
                    "timeout after 1800 seconds; process group terminated",
                )
                .unwrap();
                panic!("{label} timed out; evidence: {}", prefix.display());
            }
            thread::sleep(Duration::from_millis(100));
        };
        fs::write(prefix.with_extension("status.txt"), status.to_string()).unwrap();
        Output {
            status,
            stdout: fs::read(stdout_path).unwrap(),
            stderr: fs::read(stderr_path).unwrap(),
        }
    }
    fn checked(&mut self, label: &str, command: &mut Command) -> Vec<u8> {
        let output = self.command(label, command);
        assert!(
            output.status.success(),
            "{label} failed: {}\n{}\nevidence: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr),
            self.evidence.display()
        );
        output.stdout
    }
    fn metadata(&mut self, label: &str, directory: &Path, home: &Path, no_deps: bool) -> Value {
        let mut command = cargo(directory, home);
        command.args([
            "metadata",
            "--format-version",
            "1",
            "--filter-platform",
            TARGET,
        ]);
        if no_deps {
            command.arg("--no-deps");
        }
        let bytes = self.checked(label, &mut command);
        fs::write(self.evidence.join(format!("{label}.json")), &bytes).unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }
}

fn cargo(directory: &Path, home: &Path) -> Command {
    let mut command = Command::new("cargo");
    command.current_dir(directory);
    // Remove Cargo source/config overrides and inherited build settings. RUSTUP_HOME
    // and PATH stay available for the installed toolchain; no user's Cargo home does.
    for (key, _) in env::vars_os() {
        let key_text = key.to_string_lossy();
        if key_text.starts_with("CARGO_")
            || key_text.starts_with("ESP_")
            || key_text == "RUSTFLAGS"
            || key_text == "RUSTDOCFLAGS"
            || key_text == "RUSTC"
            || key_text.starts_with("RUSTC_")
            || key_text == "RUSTDOC"
        {
            command.env_remove(&key);
        }
    }
    command
        .env("CARGO_HOME", home)
        .env("CARGO_TARGET_DIR", directory.join("target"))
        .env("ESP_HAL_CONFIG_MIN_CHIP_REVISION", "103");
    command
}
fn text(value: &Value, key: &str) -> String {
    value[key].as_str().unwrap().to_owned()
}
fn array(value: &Value) -> &[Value] {
    value.as_array().unwrap()
}
fn load(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}
fn save(path: &Path, value: &Value) {
    fs::write(path, serde_json::to_vec_pretty(value).unwrap()).unwrap();
}
fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn manifest(path: &Path) -> Value {
    let value: toml::Value = toml::from_str(&fs::read_to_string(path).unwrap()).unwrap();
    serde_json::to_value(value).unwrap()
}
fn files(root: &Path) -> BTreeMap<String, String> {
    fn visit(root: &Path, here: &Path, result: &mut BTreeMap<String, String>) {
        for entry in fs::read_dir(here).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            if entry.file_name() == ".git" || entry.file_name() == "target" {
                continue;
            }
            if path.is_dir() {
                visit(root, &path, result);
            } else {
                result.insert(
                    path.strip_prefix(root)
                        .unwrap()
                        .to_string_lossy()
                        .into_owned(),
                    hash(&fs::read(path).unwrap()),
                );
            }
        }
    }
    let mut result = BTreeMap::new();
    visit(root, root, &mut result);
    result
}
fn tree_hash(inventory: &BTreeMap<String, String>) -> String {
    let mut digest = Sha256::new();
    digest.update(b"esp32p4-distribution-tree-v1\0");
    for (path, content_hash) in inventory {
        digest.update((path.len() as u64).to_be_bytes());
        digest.update(path.as_bytes());
        digest.update((content_hash.len() as u64).to_be_bytes());
        digest.update(content_hash.as_bytes());
    }
    format!("{:x}", digest.finalize())
}
fn consumer(directory: &Path, dependency: &str) {
    consumer_features(directory, dependency, &["esp32p4"], false);
}
fn consumer_features(directory: &Path, dependency: &str, features: &[&str], macros: bool) {
    let features = serde_json::to_string(features).unwrap();
    fs::create_dir_all(directory.join("src")).unwrap();
    fs::write(
        directory.join("Cargo.toml"),
        format!(
            r#"[package]
name = "registry-proof-firmware"
version = "0.1.0"
edition = "2024"
publish = false
[workspace]
resolver = "3"
[dependencies]
esp-hal = {{ {dependency}, features = {features} }}
[profile.release]
lto = "off"
"#
        ),
    )
    .unwrap();
    let source = if macros {
        format!("{FIRMWARE}\n{MACRO_COVERAGE}")
    } else {
        FIRMWARE.to_owned()
    };
    fs::write(directory.join("src/main.rs"), source).unwrap();
}
fn home(directory: &Path, index: Option<&str>) {
    fs::create_dir_all(directory).unwrap();
    if let Some(index) = index {
        fs::write(
            directory.join("config.toml"),
            format!(
                r#"[source.crates-io]
registry = "{index}"
"#
            ),
        )
        .unwrap();
    }
}

fn seed_registry_lock(
    proof: &Proof,
    label: &str,
    seed: &Path,
    directory: &Path,
    mappings: &[Value],
    archives: &[Value],
) {
    let mut lock = manifest(seed);
    let original_packages = array(&lock["package"]).to_vec();
    let mapping_for = |package: &Value| {
        mappings.iter().find(|mapping| {
            mapping["original_name"] == package["name"]
                && mapping["original_version"] == package["version"]
                && mapping["original_source"] == package["source"]
        })
    };
    let inactive = |package: &Value| {
        package["source"].is_null()
            && matches!(
                package["name"].as_str(),
                Some("xtensa-lx" | "xtensa-lx-rt" | "xtensa-lx-rt-proc-macros")
            )
    };
    let dropped: Vec<_> = original_packages
        .iter()
        .filter(|package| inactive(package))
        .cloned()
        .collect();
    let packages = lock["package"].as_array_mut().unwrap();
    packages.retain(|package| !inactive(package));
    for package in packages {
        if let Some(mapping) = mapping_for(package) {
            package["name"] = mapping["name"].clone();
            package["version"] = mapping["version"].clone();
            package["source"] = json!("registry+https://github.com/rust-lang/crates.io-index");
            let archive = archives
                .iter()
                .find(|archive| archive["original_id"] == mapping["original_id"])
                .unwrap();
            package["checksum"] = archive["sha256"].clone();
        } else {
            assert!(
                package["source"]
                    .as_str()
                    .is_some_and(|source| source.starts_with("registry+"))
                    || package["name"] == "registry-proof-firmware",
                "unallowlisted non-registry package in seeded consumer lock: {package}"
            );
        }
        if let Some(dependencies) = package
            .get_mut("dependencies")
            .and_then(Value::as_array_mut)
        {
            let mut rewritten = Vec::new();
            for dependency in dependencies.iter() {
                let declaration = dependency.as_str().unwrap();
                let mut parts = declaration.splitn(3, ' ');
                let name = parts.next().unwrap();
                let version = parts.next();
                let source = parts
                    .next()
                    .map(|part| part.strip_prefix('(').unwrap().strip_suffix(')').unwrap());
                // Cargo.lock package rows retain the resolved Git SHA fragment;
                // dependency references omit it. Keep URL/query rev identity for
                // this lookup; mapping_for still compares the full pinned source.
                let candidates: Vec<_> = original_packages
                    .iter()
                    .filter(|candidate| {
                        candidate["name"] == name
                            && version.is_none_or(|version| candidate["version"] == version)
                            && source.is_none_or(|source| {
                                candidate["source"]
                                    .as_str()
                                    .is_some_and(|candidate_source| {
                                        candidate_source.split('#').next()
                                            == source.split('#').next()
                                    })
                            })
                    })
                    .collect();
                assert_eq!(
                    candidates.len(),
                    1,
                    "ambiguous locked source identity: {declaration}"
                );
                let candidate = candidates[0];
                if inactive(candidate) {
                    continue;
                }
                rewritten.push(if let Some(mapping) = mapping_for(candidate) {
                    json!(format!(
                        "{} {} (registry+https://github.com/rust-lang/crates.io-index)",
                        text(mapping, "name"),
                        text(mapping, "version")
                    ))
                } else {
                    dependency.clone()
                });
            }
            *dependencies = rewritten;
        }
    }
    let value: toml::Value = serde_json::from_value(lock.clone()).unwrap();
    fs::write(
        directory.join("Cargo.lock"),
        toml::to_string(&value).unwrap(),
    )
    .unwrap();
    save(
        &proof.evidence.join(format!("{label}-lock-seed.json")),
        &json!({
            "seed_path": seed, "seed_sha256": hash(&fs::read(seed).unwrap()),
            "generated_lock": lock, "generated_sha256": hash(&fs::read(directory.join("Cargo.lock")).unwrap()),
            "identity_mapping": mappings, "checksum_origin": "real Cargo package archive SHA256",
            "target_inactive_entries_omitted": dropped,
            "target_inactive_reason": "lock-level path entries outside the filtered P4 graph are re-resolved from registry declarations; no equivalence claimed",
            "foreign_registry_versions_and_checksums": "preserved from baseline lock"
        }),
    );
}
fn index_path(name: &str) -> PathBuf {
    let name = name.to_ascii_lowercase();
    match name.len() {
        1 => PathBuf::from("1").join(name),
        2 => PathBuf::from("2").join(name),
        3 => PathBuf::from("3").join(&name[..1]).join(name),
        _ => PathBuf::from(&name[..2]).join(&name[2..4]).join(name),
    }
}
fn dependency_tables(value: &Value) -> Vec<(&str, Option<&str>, &serde_json::Map<String, Value>)> {
    let mut result = Vec::new();
    for (table, kind) in [
        ("dependencies", "normal"),
        ("build-dependencies", "build"),
        ("dev-dependencies", "dev"),
    ] {
        if let Some(deps) = value[table].as_object() {
            result.push((kind, None, deps));
        }
        if let Some(targets) = value["target"].as_object() {
            for (target, settings) in targets {
                if let Some(deps) = settings[table].as_object() {
                    result.push((kind, Some(target.as_str()), deps));
                }
            }
        }
    }
    result
}
fn index_entry(normalized: &Value, checksum: &str) -> Value {
    let mut dependencies = Vec::new();
    for (kind, target, table) in dependency_tables(normalized) {
        for (alias, dependency) in table {
            assert!(
                dependency.get("path").is_none()
                    && dependency.get("git").is_none()
                    && dependency.get("registry").is_none()
                    && dependency.get("registry-index").is_none(),
                "archive contains a non-default registry/path/git dependency: {alias}: {dependency}"
            );
            dependencies.push(json!({
                "name": alias, "req": dependency.as_str().or_else(|| dependency["version"].as_str()).unwrap(),
                "features": dependency.get("features").cloned().unwrap_or(json!([])),
                "optional": dependency["optional"].as_bool().unwrap_or(false),
                "default_features": dependency["default-features"].as_bool().unwrap_or(true),
                "target": target, "kind": kind, "registry": null, "package": dependency.get("package")
            }));
        }
    }
    json!({"name": normalized["package"]["name"], "vers": normalized["package"]["version"],
        "deps": dependencies, "cksum": checksum, "features": {},
        "features2": normalized.get("features").cloned().unwrap_or(json!({})), "v": 2,
        "yanked": false, "links": normalized["package"].get("links"),
        "rust_version": normalized["package"].get("rust-version")})
}

fn start_server(registry: &Path, log: &Path) -> (Process, String) {
    let stdout_path = log.with_extension("startup.stdout.log");
    let stderr_path = log.with_extension("startup.stderr.log");
    let mut command = Command::new("python3");
    command
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/registry_overlay.py"))
        .arg(registry)
        .arg(log)
        .stdin(Stdio::null())
        .stdout(fs::File::create(&stdout_path).unwrap())
        .stderr(fs::File::create(&stderr_path).unwrap())
        .process_group(0);
    let mut guard = Process(command.spawn().unwrap());
    let start = Instant::now();
    loop {
        let output = fs::read_to_string(&stdout_path).unwrap();
        if let Some(port) = output
            .lines()
            .next()
            .and_then(|line| line.parse::<u16>().ok())
        {
            assert_ne!(port, 0);
            return (guard, format!("sparse+http://127.0.0.1:{port}/index/"));
        }
        if let Some(status) = guard.0.try_wait().unwrap() {
            panic!(
                "overlay exited before ready: {status}; {}",
                fs::read_to_string(&stderr_path).unwrap()
            );
        }
        assert!(
            start.elapsed() < Duration::from_secs(15),
            "overlay startup timed out; {}",
            log.display()
        );
        thread::sleep(Duration::from_millis(50));
    }
}

fn original_package<'a>(graph: &'a Value, distribution: &Value) -> &'a Value {
    array(&graph["packages"])
        .iter()
        .find(|package| package["id"] == distribution["original_id"])
        .unwrap()
}
fn matching_distribution<'a>(package: &Value, packages: &'a [Value]) -> Option<&'a Value> {
    packages.iter().find(|candidate| {
        candidate["original_name"] == package["name"]
            && candidate["original_version"] == package["version"]
            && candidate["original_source"] == package["source"]
    })
}
fn non_dependency_manifest(mut value: Value, source: &Path, mapping: Option<&Value>) -> Value {
    let mut workspace = json!({});
    for ancestor in source.ancestors() {
        let path = ancestor.join("Cargo.toml");
        if path.is_file() {
            let candidate = manifest(&path);
            if candidate.get("workspace").is_some() {
                workspace = candidate["workspace"].clone();
                break;
            }
        }
    }
    if let Some(package) = value["package"].as_object_mut() {
        // Explicit file allowlists replace upstream include/exclude patterns.
        // Their safety/content is checked separately, not treated as arbitrary
        // package-metadata changes.
        package.remove("include");
        package.remove("exclude");
        for (field, setting) in package.iter_mut() {
            if setting["workspace"].as_bool() == Some(true) {
                *setting = workspace["package"][field].clone();
                assert!(
                    !setting.is_null(),
                    "missing inherited package field {field}"
                );
            }
        }
        if let Some(mapping) = mapping {
            package.insert("name".into(), mapping["name"].clone());
            package.insert("version".into(), mapping["version"].clone());
        }
    }
    if value["lints"]["workspace"].as_bool() == Some(true) {
        value["lints"] = workspace["lints"].clone();
    }
    let root = value.as_object_mut().unwrap();
    for field in [
        "workspace",
        "dependencies",
        "build-dependencies",
        "dev-dependencies",
    ] {
        root.remove(field);
    }
    if let Some(targets) = root.get_mut("target").and_then(Value::as_object_mut) {
        targets.retain(|_, settings| {
            let settings = settings.as_object_mut().unwrap();
            for field in ["dependencies", "build-dependencies", "dev-dependencies"] {
                settings.remove(field);
            }
            !settings.is_empty()
        });
        if targets.is_empty() {
            root.remove("target");
        }
    }
    value
}

fn verify_sources(graph: &Value, packages: &[Value], distribution: &Path) -> Value {
    let mut evidence = Vec::new();
    let mut inventories = Vec::new();
    for package in packages {
        let original = original_package(graph, package);
        let source = Path::new(original["manifest_path"].as_str().unwrap())
            .parent()
            .unwrap();
        let generated = distribution.join(package["directory"].as_str().unwrap());
        let source_hashes = files(source);
        let generated_hashes = files(&generated);
        assert_eq!(
            package["source_tree_hash"],
            tree_hash(&source_hashes),
            "source tree digest is not its real file inventory"
        );
        assert_eq!(
            package["generated_tree_hash"],
            tree_hash(&generated_hashes),
            "generated tree digest is not its real file inventory"
        );
        let source_rewrites: Vec<_> = array(&package["rewrites"])
            .iter()
            .filter(|rewrite| rewrite["file"] != "Cargo.toml")
            .collect();
        if package["original_name"] == "esp-hal-procmacros" {
            let expected: BTreeSet<_> = [
                "src/interrupt.rs",
                "src/lp_core.rs",
                "src/ram.rs",
                "src/unified_main.rs",
            ]
            .into_iter()
            .collect();
            assert_eq!(source_rewrites.len(), 4);
            assert_eq!(
                source_rewrites
                    .iter()
                    .map(|r| r["file"].as_str().unwrap())
                    .collect::<BTreeSet<_>>(),
                expected
            );
        } else {
            assert!(
                source_rewrites.is_empty(),
                "non-macro source rewrite declared"
            );
        }
        assert_eq!(
            source_hashes.keys().collect::<Vec<_>>(),
            generated_hashes.keys().collect::<Vec<_>>(),
            "generated file inventory changed for {}",
            package["name"]
        );
        for (file, digest) in &source_hashes {
            if file == "Cargo.toml" {
                continue;
            }
            let mut expected = fs::read(source.join(file)).unwrap();
            for rewrite in array(&package["rewrites"]) {
                if rewrite["file"].as_str() != Some(file.as_str()) {
                    continue;
                }
                // Only proc-macro package lookup strings are allowed to alter Rust source.
                assert_eq!(package["original_name"], "esp-hal-procmacros");
                let from = rewrite["from"].as_str().unwrap();
                let to = rewrite["to"].as_str().unwrap();
                let expected_from = if file == "src/unified_main.rs" {
                    "proc_macro_crate::crate_name(\"esp-hal\")"
                } else {
                    "crate_name(\"esp-hal\")"
                };
                assert_eq!(
                    from, expected_from,
                    "unallowlisted macro replacement {rewrite}"
                );
                assert_eq!(
                    to,
                    expected_from.replace("\"esp-hal\"", "\"esp-hal-p4-pre-v3\"")
                );
                let content = String::from_utf8(expected).unwrap();
                assert_eq!(
                    content.matches(from).count(),
                    1,
                    "expected exactly one package lookup in {file}"
                );
                expected = content.replace(from, to).into_bytes();
            }
            assert_eq!(
                hash(&expected),
                generated_hashes[file],
                "undeclared source mutation: {}:{file}",
                package["name"]
            );
            if digest != &generated_hashes[file] {
                evidence.push(json!({"package": package["name"], "file": file,
                    "original_sha256": digest, "generated_sha256": generated_hashes[file]}));
            }
        }
        let generated_manifest = manifest(&generated.join("Cargo.toml"));
        let source_manifest = manifest(&source.join("Cargo.toml"));
        let include = array(&generated_manifest["package"]["include"]);
        assert!(
            !include.is_empty(),
            "publishable package needs an explicit file allowlist"
        );
        assert!(generated_manifest["package"].get("exclude").is_none());
        let mut allowed = BTreeSet::new();
        for file in include {
            let file = file.as_str().unwrap().trim_start_matches('/');
            assert!(
                !file.is_empty()
                    && !file.chars().any(|c| matches!(c, '*' | '?' | '[' | ']'))
                    && Path::new(file)
                        .components()
                        .all(|c| matches!(c, std::path::Component::Normal(_))),
                "allowlist is not a literal package-relative file: {file}"
            );
            assert!(
                source_hashes.contains_key(file),
                "allowlist references a nonexistent source file: {file}"
            );
            assert!(allowed.insert(file), "duplicate allowlist entry: {file}");
        }
        for target in array(&original["targets"]) {
            if array(&target["kind"])
                .iter()
                .any(|kind| matches!(kind.as_str(), Some("example" | "test" | "bench")))
            {
                continue;
            }
            let file = Path::new(target["src_path"].as_str().unwrap())
                .strip_prefix(source)
                .unwrap();
            assert!(
                allowed.contains(file.to_str().unwrap()),
                "allowlist omitted a production target: {}",
                file.display()
            );
        }
        evidence.push(json!({"package": package["name"], "file": "Cargo.toml",
            "original_sha256": source_hashes["Cargo.toml"], "generated_sha256": generated_hashes["Cargo.toml"],
            "allowed_transformations": ["identity and exact internal dependency mapping", "workspace inheritance flattening",
                "dev-only dependency removal", "literal source-derived package include", "inactive Xtensa source normalization"],
            "source_package_include": source_manifest["package"].get("include"),
            "source_package_exclude": source_manifest["package"].get("exclude"),
            "generated_package_include": generated_manifest["package"]["include"]}));
        assert_eq!(generated_manifest["package"]["name"], package["name"]);
        assert_eq!(generated_manifest["package"]["version"], package["version"]);
        assert_eq!(
            generated_manifest["package"].get("links"),
            original.get("links").filter(|v| !v.is_null())
        );
        assert_eq!(
            generated_manifest
                .get("features")
                .cloned()
                .unwrap_or(json!({})),
            source_manifest
                .get("features")
                .cloned()
                .unwrap_or(json!({})),
            "raw TOML feature declarations changed"
        );
        assert_eq!(
            non_dependency_manifest(generated_manifest, &generated, None),
            non_dependency_manifest(source_manifest, source, Some(package)),
            "manifest changed outside workspace flattening/package identity/dependency rewrites: {}",
            package["name"]
        );
        inventories.push(
            json!({"original_id": package["original_id"], "name": package["name"],
            "source_file_sha256": source_hashes, "generated_file_sha256": generated_hashes}),
        );
    }
    json!({"allowed_source_differences": evidence, "file_inventories": inventories})
}

fn dependency_contract(package: &Value, mappings: &[Value], original: bool) -> Vec<Value> {
    let mut result = Vec::new();
    for dependency in array(&package["dependencies"]) {
        if dependency["kind"] == "dev" {
            continue;
        }
        let mapped = if original {
            mappings.iter().find(|candidate| {
                candidate["original_name"] == dependency["name"]
                    && if dependency["source"].is_null() {
                        candidate["original_source"].is_null()
                    } else {
                        candidate["original_source"]
                            .as_str()
                            .unwrap_or("")
                            .split('#')
                            .next()
                            == dependency["source"].as_str()
                    }
            })
        } else {
            None
        };
        let mut features = array(&dependency["features"]).to_vec();
        features.sort_by_key(Value::to_string);
        let mut contract = json!({
            "name": dependency["name"], "req": dependency["req"],
            "rename": dependency["rename"], "kind": dependency["kind"], "target": dependency["target"],
            "optional": dependency["optional"], "uses_default_features": dependency["uses_default_features"],
            "features": features, "source": dependency["source"]
        });
        if let Some(mapped) = mapped {
            contract["name"] = mapped["name"].clone();
            contract["req"] = json!(format!("={}", mapped["version"].as_str().unwrap()));
            contract["source"] = json!("registry+https://github.com/rust-lang/crates.io-index");
            // Cargo metadata represents an explicit package alias even when the
            // original path dependency had no rename field.
            contract["rename"] = dependency["rename"]
                .as_str()
                .map_or_else(|| dependency["name"].clone(), |s| json!(s));
        }
        if original
            && mapped.is_none()
            && dependency["source"].is_null()
            && matches!(
                dependency["name"].as_str(),
                Some("xtensa-lx" | "xtensa-lx-rt")
            )
        {
            assert_eq!(
                dependency["target"], "cfg(target_arch = \"xtensa\")",
                "out-of-closure path normalization is allowed only for inactive Xtensa targets"
            );
            contract["source"] = json!("registry+https://github.com/rust-lang/crates.io-index");
        }
        result.push(contract);
    }
    result.sort_by_key(Value::to_string);
    result
}
fn targets_contract(package: &Value, mapping: Option<&Value>) -> Vec<Value> {
    let root = Path::new(package["manifest_path"].as_str().unwrap())
        .parent()
        .unwrap();
    let mut result = Vec::new();
    for target in array(&package["targets"]) {
        if array(&target["kind"])
            .iter()
            .any(|kind| matches!(kind.as_str(), Some("example" | "test" | "bench")))
        {
            continue;
        }
        let mut name = target["name"].clone();
        if let Some(mapping) = mapping
            && target["name"].as_str() == Some(&text(mapping, "name").replace('-', "_"))
        {
            name = json!(text(mapping, "original_name").replace('-', "_"));
        }
        result.push(json!({"name": name, "kind": target["kind"], "crate_types": target["crate_types"],
            "required-features": target.get("required-features"), "edition": target["edition"],
            "src_path": Path::new(target["src_path"].as_str().unwrap()).strip_prefix(root).unwrap().to_string_lossy()}));
    }
    result.sort_by_key(Value::to_string);
    result
}

fn graph_contract(graph: &Value, mappings: &[Value], registry: bool) -> Value {
    let root = graph["resolve"]["root"].as_str().unwrap();
    let packages: BTreeMap<_, _> = array(&graph["packages"])
        .iter()
        .map(|p| (p["id"].as_str().unwrap(), p))
        .collect();
    let nodes: BTreeMap<_, _> = array(&graph["resolve"]["nodes"])
        .iter()
        .map(|n| (n["id"].as_str().unwrap(), n))
        .collect();
    let identity = |id: &str| -> String {
        if id == root {
            return "consumer".into();
        }
        let package = packages[id];
        let mapping = if registry {
            mappings
                .iter()
                .find(|m| m["name"] == package["name"] && m["version"] == package["version"])
        } else {
            matching_distribution(package, mappings)
        };
        if let Some(mapping) = mapping {
            text(mapping, "original_id")
        } else {
            assert!(
                package["source"]
                    .as_str()
                    .is_some_and(|s| s.starts_with("registry+")),
                "unexpected non-registry foreign package {package}"
            );
            format!(
                "{}@{}:{}",
                package["name"].as_str().unwrap(),
                package["version"].as_str().unwrap(),
                package["source"].as_str().unwrap()
            )
        }
    };
    let mut reached = BTreeSet::new();
    let mut pending = vec![root];
    while let Some(id) = pending.pop() {
        if !reached.insert(id) {
            continue;
        }
        for dependency in array(&nodes[id]["deps"]) {
            if array(&dependency["dep_kinds"])
                .iter()
                .any(|kind| kind["kind"] != "dev")
            {
                pending.push(dependency["pkg"].as_str().unwrap());
            }
        }
    }
    let mut result = BTreeMap::new();
    for id in reached {
        let package = packages[id];
        assert!(
            !matches!(package["name"].as_str(), Some("xtensa-lx" | "xtensa-lx-rt")),
            "target-inactive publication normalization reached the P4 normal/build graph"
        );
        if registry && id != root {
            assert!(
                package["source"]
                    .as_str()
                    .is_some_and(|s| s.starts_with("registry+")),
                "registry consumer leaked checkout: {package}"
            );
        }
        let mut edges = Vec::new();
        for dep in array(&nodes[id]["deps"]) {
            for kind in array(&dep["dep_kinds"]) {
                if kind["kind"] != "dev" {
                    let destination = packages[dep["pkg"].as_str().unwrap()];
                    edges.push(
                        json!({"alias": dep["name"], "to": identity(dep["pkg"].as_str().unwrap()),
                        "kind": kind["kind"], "target": kind["target"],
                        "proc_macro": array(&destination["targets"]).iter().any(|target|
                            array(&target["kind"]).iter().any(|role| role == "proc-macro"))}),
                    );
                }
            }
        }
        edges.sort_by_key(Value::to_string);
        let mapping = if registry {
            mappings
                .iter()
                .find(|m| m["name"] == package["name"] && m["version"] == package["version"])
        } else {
            None
        };
        let mut roles = targets_contract(package, mapping);
        if id == root {
            for role in &mut roles {
                role["name"] = json!("consumer");
            }
        }
        result.insert(identity(id), json!({"features": nodes[id]["features"], "links": package["links"],
            "feature_definitions": package["features"], "roles": roles,
            "dependencies": edges, "declarations": dependency_contract(package, mappings, !registry)}));
    }
    serde_json::to_value(result).unwrap()
}

#[allow(clippy::too_many_arguments)]
fn record_graph_differences(
    proof: &Proof,
    label: &str,
    baseline: &Value,
    registry: &Value,
    mappings: &[Value],
    expected: &Value,
    actual: &Value,
    physical_index: &str,
) {
    let mut differences = Vec::new();
    for mapping in mappings {
        let before = array(&baseline["packages"])
            .iter()
            .find(|p| matching_distribution(p, std::slice::from_ref(mapping)).is_some());
        let after = array(&registry["packages"]).iter().find(|p| {
            (p["name"] == mapping["name"] && p["version"] == mapping["version"])
                || matching_distribution(p, std::slice::from_ref(mapping)).is_some()
        });
        assert_eq!(
            before.is_some(),
            after.is_some(),
            "mapped member disappeared from the registry consumer"
        );
        if let (Some(before), Some(after)) = (before, after) {
            if before["id"] == after["id"]
                && before["name"] == after["name"]
                && before["version"] == after["version"]
                && before["source"] == after["source"]
            {
                continue;
            }
            differences.push(json!({
                "original_id": mapping["original_id"],
                "before": {"id": before["id"], "name": before["name"], "version": before["version"], "source": before["source"]},
                "after": {"id": after["id"], "name": after["name"], "version": after["version"], "source": after["source"]},
                "reason": "preserve pinned source identity as a separately named registry fork; do not substitute same-version upstream",
                "dependency_requirement_rule": format!("={}", text(mapping, "version")),
                "dependency_alias_rule": "preserve original alias (original package name when previously implicit)",
                "target_name_rule": "only Cargo's default name follows the mapped package identity"
            }));
        }
    }
    let keys: BTreeSet<_> = expected
        .as_object()
        .unwrap()
        .keys()
        .chain(actual.as_object().unwrap().keys())
        .collect();
    let unexpected: Vec<_> = keys
        .into_iter()
        .filter(|key| expected[*key] != actual[*key])
        .map(|key| json!({"logical_id": key, "expected": expected[key], "actual": actual[key]}))
        .collect();
    let root_identity = |graph: &Value| {
        let package = array(&graph["packages"])
            .iter()
            .find(|package| package["id"] == graph["resolve"]["root"])
            .unwrap();
        json!({"id": package["id"], "name": package["name"], "version": package["version"],
            "manifest_path": package["manifest_path"], "source": package["source"]})
    };
    save(
        &proof.evidence.join(format!("{label}.json")),
        &json!({
            "schema": 1, "intentional_identity_differences": differences, "allowlisted_identity_mapping": mappings,
            "unallowlisted_differences": unexpected,
            "consumer_root_identity_allowlist": {"before": root_identity(baseline), "after": root_identity(registry),
                "rule": "only consumer package identity, checkout path, and default target name are logicalized; same declared dependencies/features/target roles"},
            "comparison_fields": ["normal/build edges", "aliases", "dependency requirements and flags", "target predicates",
                "feature definitions", "activated features", "native links", "production target roles", "proc-macro relationships"],
            "dev_only_equivalence_claimed": false,
            "target_inactive_normalization": {"names": ["xtensa-lx", "xtensa-lx-rt"],
                "only_target": "cfg(target_arch = \"xtensa\")", "must_not_resolve_or_compile_on_p4": true},
            "physical_test_index": physical_index,
            "source_identity_note": "Cargo metadata canonical registry identity and loopback transport are separate; see registry-source.json"
        }),
    );
}

fn build_units(
    proof: &mut Proof,
    label: &str,
    directory: &Path,
    home: &Path,
    graph: &Value,
    mappings: &[Value],
    registry: bool,
) -> Value {
    let output = proof.checked(
        label,
        cargo(directory, home).args([
            "rustc",
            "--locked",
            "--release",
            "--target",
            TARGET,
            "--message-format=json",
            "--",
            "-C",
            "link-arg=-Tlinkall.x",
            "-C",
            "force-frame-pointers=yes",
        ]),
    );
    let mut units = BTreeSet::new();
    for line in String::from_utf8(output).unwrap().lines() {
        let record: Value = serde_json::from_str(line).unwrap();
        if record["reason"] != "compiler-artifact" {
            continue;
        }
        let package = array(&graph["packages"])
            .iter()
            .find(|p| p["id"] == record["package_id"])
            .unwrap();
        assert!(
            !matches!(package["name"].as_str(), Some("xtensa-lx" | "xtensa-lx-rt")),
            "target-inactive publication normalization reached a compiled P4 unit"
        );
        let root = package["id"] == graph["resolve"]["root"];
        let mapping = if registry {
            mappings
                .iter()
                .find(|m| m["name"] == package["name"] && m["version"] == package["version"])
        } else {
            matching_distribution(package, mappings)
        };
        let logical = if root {
            "consumer".into()
        } else if let Some(mapping) = mapping {
            text(mapping, "original_id")
        } else {
            format!(
                "{}@{}:{}",
                text(package, "name"),
                text(package, "version"),
                text(package, "source")
            )
        };
        let mut target_name = text(&record["target"], "name");
        if let Some(mapping) = mapping
            && target_name == text(mapping, "name").replace('-', "_")
        {
            target_name = text(mapping, "original_name").replace('-', "_");
        }
        let filenames = array(&record["filenames"]);
        assert!(
            !filenames.is_empty(),
            "compiler artifact has no output paths"
        );
        let target_directory = directory.join("target").join(TARGET);
        let contexts: BTreeSet<_> = filenames
            .iter()
            .map(|file| {
                if Path::new(file.as_str().unwrap()).starts_with(&target_directory) {
                    "target"
                } else {
                    "host"
                }
            })
            .collect();
        let mut features = array(&record["features"]).to_vec();
        features.sort_by_key(Value::to_string);
        units.insert(
            json!({"logical_package_id": logical, "target_name": target_name,
            "kind": record["target"]["kind"], "crate_types": record["target"]["crate_types"],
            "features": features, "compilation_context": contexts})
            .to_string(),
        );
    }
    assert!(
        !units.is_empty(),
        "release build emitted no compiler artifacts"
    );
    json!(
        units
            .into_iter()
            .map(|unit| serde_json::from_str::<Value>(&unit).unwrap())
            .collect::<Vec<_>>()
    )
}

#[allow(clippy::too_many_arguments)]
fn supplemental_macro_proof(
    proof: &mut Proof,
    prepared: &Path,
    hal: &Value,
    mappings: &[Value],
    archives: &[Value],
    work: &Path,
    index: &str,
    primary: &Value,
) {
    let baseline = work.join("supplemental-baseline-consumer");
    let registry = work.join("supplemental-registry-consumer");
    let baseline_home = work.join("supplemental-baseline-home");
    let registry_home = work.join("supplemental-registry-home");
    home(&baseline_home, None);
    home(&registry_home, Some(index));
    let features = ["esp32p4", "critical-section", "unstable"];
    consumer_features(
        &baseline,
        &format!(
            "path = {}",
            serde_json::to_string(&prepared.join("upstream/esp-hal")).unwrap()
        ),
        &features,
        true,
    );
    consumer_features(
        &registry,
        &format!(
            "package = {}, version = {}",
            serde_json::to_string(&hal["name"]).unwrap(),
            serde_json::to_string(&format!("={}", text(hal, "version"))).unwrap()
        ),
        &features,
        true,
    );
    fs::copy(
        work.join("baseline-consumer/Cargo.lock"),
        baseline.join("Cargo.lock"),
    )
    .unwrap();
    let baseline_graph = proof.metadata(
        "supplemental-baseline-graph",
        &baseline,
        &baseline_home,
        false,
    );
    seed_registry_lock(
        proof,
        "supplemental-registry",
        &baseline.join("Cargo.lock"),
        &registry,
        mappings,
        archives,
    );
    let registry_graph = proof.metadata(
        "supplemental-registry-graph",
        &registry,
        &registry_home,
        false,
    );
    for package in array(&registry_graph["packages"]) {
        if package["id"] == registry_graph["resolve"]["root"] {
            continue;
        }
        assert_eq!(
            package["source"],
            "registry+https://github.com/rust-lang/crates.io-index"
        );
    }
    let expected = graph_contract(&baseline_graph, mappings, false);
    let actual = graph_contract(&registry_graph, mappings, true);
    save(
        &proof
            .evidence
            .join("supplemental-baseline-logical-graph.json"),
        &expected,
    );
    save(
        &proof
            .evidence
            .join("supplemental-registry-logical-graph.json"),
        &actual,
    );
    record_graph_differences(
        proof,
        "supplemental-graph-differences",
        &baseline_graph,
        &registry_graph,
        mappings,
        &expected,
        &actual,
        index,
    );
    assert_eq!(
        actual, expected,
        "supplemental macro consumers differ beyond identity mapping"
    );
    let keys: BTreeSet<_> = primary
        .as_object()
        .unwrap()
        .keys()
        .chain(expected.as_object().unwrap().keys())
        .collect();
    let activation_changes: Vec<_> = keys.into_iter().filter(|key| primary[*key] != expected[*key])
        .map(|key| json!({"logical_id": key, "primary": primary[key], "supplemental": expected[key]})).collect();
    save(
        &proof.evidence.join("supplemental-feature-allowlist.json"),
        &json!({
            "primary_requested_hal_features": ["esp32p4"],
            "supplemental_requested_hal_features": features,
            "reason": "separate paired consumer covers proc-macro identity lookup paths hidden behind unstable APIs",
            "observed_feature_and_resolution_changes": activation_changes,
            "not_used_to_expand_primary_distribution_closure": true
        }),
    );
    let baseline_units = build_units(
        proof,
        "supplemental-baseline-release-rev103",
        &baseline,
        &baseline_home,
        &baseline_graph,
        mappings,
        false,
    );
    save(
        &proof
            .evidence
            .join("supplemental-baseline-compiler-units.json"),
        &baseline_units,
    );
    let registry_units = build_units(
        proof,
        "supplemental-registry-release-rev103",
        &registry,
        &registry_home,
        &registry_graph,
        mappings,
        true,
    );
    save(
        &proof
            .evidence
            .join("supplemental-registry-compiler-units.json"),
        &registry_units,
    );
    assert_eq!(
        registry_units, baseline_units,
        "supplemental compiled host/target feature units differ"
    );
    assert!(!registry_home.join("git").exists());
    let baseline_elf = baseline
        .join("target")
        .join(TARGET)
        .join("release/registry-proof-firmware");
    let registry_elf = registry
        .join("target")
        .join(TARGET)
        .join("release/registry-proof-firmware");
    fs::copy(
        &baseline_elf,
        proof.evidence.join("supplemental-baseline-firmware.elf"),
    )
    .unwrap();
    fs::copy(
        &registry_elf,
        proof.evidence.join("supplemental-registry-firmware.elf"),
    )
    .unwrap();
    fs::copy(
        registry.join("Cargo.toml"),
        proof
            .evidence
            .join("supplemental-registry-consumer.Cargo.toml"),
    )
    .unwrap();
    fs::copy(
        registry.join("Cargo.lock"),
        proof
            .evidence
            .join("supplemental-registry-consumer.Cargo.lock"),
    )
    .unwrap();
    save(
        &proof.evidence.join("supplemental-proof.json"),
        &json!({
            "target": TARGET, "minimum_chip_revision": 103, "requested_hal_features": features,
            "normal_build_graph_equal_after_identity_allowlist": true,
            "compiler_feature_units_equal_after_identity_allowlist": true,
            "macro_coverage": ["main", "handler", "ram rtc_fast zeroed trait assertion"],
            "baseline_elf_sha256": hash(&fs::read(baseline_elf).unwrap()),
            "registry_elf_sha256": hash(&fs::read(registry_elf).unwrap())
        }),
    );
}

#[test]
#[ignore = "networked full real packaging and isolated rev103 registry-only release build"]
fn test_registry_distribution_proof() {
    let repository = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let evidence_parent = repository.join("target/distribution-validation");
    fs::create_dir_all(&evidence_parent).unwrap();
    let evidence = tempfile::Builder::new()
        .prefix("registry-proof-")
        .tempdir_in(&evidence_parent)
        .unwrap()
        .keep();
    let mut proof = Proof {
        evidence: evidence.clone(),
        sequence: 0,
    };
    let workspace = tempfile::Builder::new()
        .prefix("esp-p4-registry-proof-")
        .tempdir()
        .unwrap()
        .keep();
    let work = workspace.as_path();
    save(
        &evidence.join("work-directory.json"),
        &json!({"directory": work,
        "retained_on_success_and_failure": true, "outside_checkout": true}),
    );
    let baseline_home = work.join("baseline-home");
    home(&baseline_home, None);
    proof.checked(
        "rustc-version",
        Command::new("rustc").arg("-vV").current_dir(work),
    );
    proof.checked("cargo-version", cargo(work, &baseline_home).arg("-V"));
    let binary = env!("CARGO_BIN_EXE_cargo-esp32p4-pre-v3");
    let explicit_prepared = env::var_os("ESP_P4_PROOF_PREPARED");
    let explicit_metadata = env::var_os("ESP_P4_PROOF_METADATA");
    assert_eq!(
        explicit_prepared.is_some(),
        explicit_metadata.is_some(),
        "set both frozen input variables, or neither"
    );
    let frozen_input = explicit_prepared.is_some();
    let (prepared, inventory_file) =
        if let (Some(prepared), Some(metadata)) = (explicit_prepared, explicit_metadata) {
            (
                fs::canonicalize(prepared).unwrap(),
                fs::canonicalize(metadata).unwrap(),
            )
        } else {
            let prepared = work.join("prepared");
            proof.checked(
                "prepare",
                Command::new(binary)
                    .arg("prepare")
                    .arg(&prepared)
                    .current_dir(work),
            );
            let inventory = work.join("inventory");
            consumer(
                &inventory,
                &format!(
                    "path = {}",
                    serde_json::to_string(&prepared.join("upstream/esp-hal")).unwrap()
                ),
            );
            let graph = proof.metadata("inventory", &inventory, &baseline_home, false);
            let metadata = evidence.join("prepared-inventory.json");
            save(&metadata, &graph);
            (prepared, metadata)
        };
    let inventory = load(&inventory_file);
    fs::copy(&inventory_file, evidence.join("input-graph.json")).unwrap();
    let before: BTreeMap<_, _> = array(&inventory["packages"])
        .iter()
        .filter(|p| {
            p["source"].is_null() || p["source"].as_str().is_some_and(|s| s.starts_with("git+"))
        })
        .filter(|p| Path::new(p["manifest_path"].as_str().unwrap()).is_file())
        .map(|p| {
            (
                text(p, "id"),
                files(
                    Path::new(p["manifest_path"].as_str().unwrap())
                        .parent()
                        .unwrap(),
                ),
            )
        })
        .collect();
    save(
        &evidence.join("source-before.json"),
        &serde_json::to_value(&before).unwrap(),
    );
    let distribution = work.join("distribution");
    let mut distribute = Command::new(binary);
    distribute
        .arg("distribute")
        .arg(&prepared)
        .arg(&inventory_file)
        .arg(&distribution)
        .current_dir(work);
    if let Some(home) = env::var_os("ESP_P4_PROOF_BASELINE_CARGO_HOME") {
        distribute.env("CARGO_HOME", home);
    } else if !frozen_input {
        distribute.env("CARGO_HOME", &baseline_home);
    }
    proof.checked("distribute", &mut distribute);
    let dist = load(&distribution.join("distribution.json"));
    fs::copy(
        distribution.join("distribution.json"),
        evidence.join("distribution.json"),
    )
    .unwrap();
    assert_eq!(dist["version"], 1);
    assert_eq!(dist["baseline"]["target"], TARGET);
    assert_eq!(
        dist["baseline"]["graph_sha256"],
        hash(&fs::read(&inventory_file).unwrap())
    );
    let pin: Value = serde_json::from_str(include_str!("../patches/manifest.json")).unwrap();
    assert_eq!(
        dist["baseline"]["upstream_revision"],
        pin["upstream"]["revision"]
    );
    assert_eq!(dist["baseline"]["patch_sha256"], pin["patch"]["sha256"]);
    let packages = array(&dist["packages"]);
    assert!(!packages.is_empty());
    let root = inventory["resolve"]["root"].as_str().unwrap();
    let nodes: BTreeMap<_, _> = array(&inventory["resolve"]["nodes"])
        .iter()
        .map(|node| (node["id"].as_str().unwrap(), node))
        .collect();
    let mut reachable = BTreeSet::new();
    let mut pending = vec![root];
    while let Some(id) = pending.pop() {
        if !reachable.insert(id) {
            continue;
        }
        for dependency in array(&nodes[id]["deps"]) {
            if array(&dependency["dep_kinds"])
                .iter()
                .any(|kind| kind["kind"] != "dev")
            {
                pending.push(dependency["pkg"].as_str().unwrap());
            }
        }
    }
    let expected_ids: BTreeSet<_> = array(&inventory["packages"])
        .iter()
        .filter(|package| package["id"].as_str() != Some(root))
        .filter(|package| reachable.contains(package["id"].as_str().unwrap()))
        .filter(|package| {
            package["source"].is_null()
                || package["source"]
                    .as_str()
                    .is_some_and(|source| source.starts_with("git+"))
        })
        .map(|package| package["id"].as_str().unwrap())
        .collect();
    assert_eq!(
        packages
            .iter()
            .map(|package| package["original_id"].as_str().unwrap())
            .collect::<BTreeSet<_>>(),
        expected_ids,
        "distribution omitted or added a non-registry normal/build closure package"
    );
    assert_eq!(
        packages
            .iter()
            .map(|package| text(package, "name"))
            .collect::<BTreeSet<_>>()
            .len(),
        packages.len(),
        "distinct original source identities collided in the distribution namespace"
    );
    for package in packages {
        let original = original_package(&inventory, package);
        assert_eq!(package["original_name"], original["name"]);
        assert_eq!(package["original_version"], original["version"]);
        assert_eq!(package["original_source"], original["source"]);
        assert_eq!(
            package["version"],
            format!("{}-p4v13.1", text(original, "version"))
        );
        assert_eq!(
            package["directory"],
            format!("crates/{}", text(package, "name"))
        );
        if original["name"] == "esp-hal" {
            assert_eq!(package["name"], "esp-hal-p4-pre-v3");
        }
    }
    save(
        &evidence.join("source-rewrites.json"),
        &verify_sources(&inventory, packages, &distribution),
    );

    let minimal = work.join("baseline-consumer");
    consumer(
        &minimal,
        &format!(
            "path = {}",
            serde_json::to_string(&prepared.join("upstream/esp-hal")).unwrap()
        ),
    );
    let inventory_root = array(&inventory["packages"])
        .iter()
        .find(|package| package["id"] == inventory["resolve"]["root"])
        .unwrap();
    let inventory_lock = Path::new(inventory_root["manifest_path"].as_str().unwrap())
        .parent()
        .unwrap()
        .join("Cargo.lock");
    assert!(
        inventory_lock.is_file(),
        "frozen goal graph needs its actual Cargo.lock for dependency version parity"
    );
    fs::copy(&inventory_lock, minimal.join("Cargo.lock")).unwrap();
    fs::copy(&inventory_lock, evidence.join("frozen-goal.Cargo.lock")).unwrap();
    let baseline_graph = proof.metadata("baseline-minimal-graph", &minimal, &baseline_home, false);
    fs::copy(
        minimal.join("Cargo.lock"),
        evidence.join("baseline-consumer.Cargo.lock"),
    )
    .unwrap();
    let frozen_contract = graph_contract(&inventory, packages, false);
    let recreated_contract = graph_contract(&baseline_graph, packages, false);
    save(
        &evidence.join("frozen-goal-logical-graph.json"),
        &frozen_contract,
    );
    save(
        &evidence.join("recreated-goal-logical-graph.json"),
        &recreated_contract,
    );
    record_graph_differences(
        &proof,
        "frozen-to-recreated-goal-differences",
        &inventory,
        &baseline_graph,
        packages,
        &frozen_contract,
        &recreated_contract,
        "official crates.io; no test index override",
    );
    assert_eq!(
        recreated_contract, frozen_contract,
        "fresh prepared consumer no longer recreates frozen goal normal/build graph"
    );
    let registry = evidence.join("registry");
    fs::create_dir_all(registry.join("index")).unwrap();
    fs::create_dir_all(registry.join("archives")).unwrap();
    save(
        &registry.join("names.json"),
        &json!(packages.iter().map(|p| text(p, "name")).collect::<Vec<_>>()),
    );
    let (_server, index) = start_server(&registry, &evidence.join("http-requests.jsonl"));
    let packaging_home = work.join("packaging-home");
    home(&packaging_home, Some(&index));
    save(
        &evidence.join("registry-source.json"),
        &json!({
            "configuration": "direct [source.crates-io].registry index override; NOT source replacement",
            "physical_index": index, "canonical_source_identity": "registry+https://github.com/rust-lang/crates.io-index",
            "generated_names": packages.iter().map(|package| text(package, "name")).collect::<Vec<_>>(),
            "foreign_index_origin": "https://index.crates.io/",
            "foreign_archive_origin": "https://static.crates.io/crates/",
            "public_payload_registry_contract": "normalized manifest dependencies retain default crates.io identity",
            "scope": "test-only fork namespace overlay; no public registration or alternate-registry claim"
        }),
    );
    let mut remaining: BTreeMap<String, &Value> =
        packages.iter().map(|p| (text(p, "name"), p)).collect();
    let generated_names: BTreeSet<_> = remaining.keys().cloned().collect();
    let mut published = BTreeSet::new();
    let mut archives = Vec::new();
    while !remaining.is_empty() {
        let ready = remaining.iter().find_map(|(name, package)| {
            let generated = distribution.join(package["directory"].as_str().unwrap());
            let value = manifest(&generated.join("Cargo.toml"));
            let dependencies = dependency_tables(&value)
                .into_iter()
                .flat_map(|(_, _, deps)| deps.iter())
                .map(|(alias, dep)| dep["package"].as_str().unwrap_or(alias));
            if dependencies
                .filter(|name| generated_names.contains(*name))
                .all(|name| published.contains(name))
            {
                Some(name.clone())
            } else {
                None
            }
        });
        let name = ready.unwrap_or_else(|| {
            panic!(
                "publication cycle or unresolved optional/dev dependency among: {:?}",
                remaining.keys().collect::<Vec<_>>()
            )
        });
        let package = remaining.remove(&name).unwrap();
        let generated = distribution.join(package["directory"].as_str().unwrap());
        let version = text(package, "version");
        proof.checked(
            &format!("package-{name}"),
            cargo(&generated, &packaging_home).args([
                "package",
                "--allow-dirty",
                "--no-verify",
                "--target",
                TARGET,
            ]),
        );
        let archive = generated
            .join("target/package")
            .join(format!("{name}-{version}.crate"));
        assert!(
            archive.is_file(),
            "cargo package did not create real archive"
        );
        let bytes = fs::read(&archive).unwrap();
        let checksum = hash(&bytes);
        let archive_dir = registry.join("archives").join(&name).join(&version);
        fs::create_dir_all(&archive_dir).unwrap();
        fs::write(archive_dir.join("download"), bytes).unwrap();
        let extracted = work.join("extracted").join(&name);
        fs::create_dir_all(&extracted).unwrap();
        proof.checked(
            &format!("extract-{name}"),
            Command::new("tar")
                .arg("-xzf")
                .arg(&archive)
                .arg("-C")
                .arg(&extracted),
        );
        let payload = extracted.join(format!("{name}-{version}"));
        let normalized = manifest(&payload.join("Cargo.toml"));
        assert!(
            normalized.get("patch").is_none() && normalized.get("replace").is_none(),
            "archive must not carry dependency overrides"
        );
        if payload.join("Cargo.lock").is_file() {
            let lock = manifest(&payload.join("Cargo.lock"));
            for locked in array(&lock["package"]) {
                if locked["name"] == package["name"] && locked["version"] == package["version"] {
                    continue;
                }
                assert!(
                    locked["source"]
                        .as_str()
                        .is_some_and(|source| source.starts_with("registry+")),
                    "archive lockfile contains a checkout dependency: {locked}"
                );
            }
        }
        let manifest_evidence = evidence.join("manifests").join(&name);
        fs::create_dir_all(&manifest_evidence).unwrap();
        let original_manifest = Path::new(
            original_package(&inventory, package)["manifest_path"]
                .as_str()
                .unwrap(),
        );
        fs::copy(
            original_manifest,
            manifest_evidence.join("prepared.Cargo.toml"),
        )
        .unwrap();
        fs::copy(
            generated.join("Cargo.toml"),
            manifest_evidence.join("generated.Cargo.toml"),
        )
        .unwrap();
        fs::copy(
            payload.join("Cargo.toml"),
            manifest_evidence.join("archive.Cargo.toml"),
        )
        .unwrap();
        fs::copy(
            payload.join("Cargo.toml.orig"),
            manifest_evidence.join("archive.Cargo.toml.orig"),
        )
        .unwrap();
        assert_eq!(
            fs::read(payload.join("Cargo.toml.orig")).unwrap(),
            fs::read(generated.join("Cargo.toml")).unwrap()
        );
        let payload_files = files(&payload);
        let listing = proof.checked(
            &format!("list-{name}"),
            cargo(&generated, &packaging_home).args(["package", "--allow-dirty", "--list"]),
        );
        let expected_files: BTreeSet<_> = String::from_utf8(listing)
            .unwrap()
            .lines()
            .map(str::to_owned)
            .collect();
        assert_eq!(
            expected_files,
            payload_files.keys().cloned().collect::<BTreeSet<_>>(),
            "archive inventory differs for {name}"
        );
        let input = manifest(&generated.join("Cargo.toml"));
        let allowed_files: BTreeSet<_> = array(&input["package"]["include"])
            .iter()
            .map(|file| file.as_str().unwrap().trim_start_matches('/').to_owned())
            .collect();
        let mut cargo_generated = Vec::new();
        for (file, digest) in &payload_files {
            match file.as_str() {
                "Cargo.toml" | "Cargo.lock" | ".cargo_vcs_info.json" => {
                    cargo_generated.push(json!({"file": file, "sha256": digest,
                        "reason": match file.as_str() {
                            "Cargo.toml" => "Cargo normalized manifest; expanded metadata/declarations checked below",
                            "Cargo.lock" => "Cargo-generated package resolution; every non-root source checked registry-only",
                            _ => "Cargo-generated VCS provenance; not a source payload file"
                        }}));
                }
                "Cargo.toml.orig" => {
                    assert_eq!(
                        *digest,
                        hash(&fs::read(generated.join("Cargo.toml")).unwrap())
                    );
                    cargo_generated.push(json!({"file": file, "sha256": digest,
                        "reason": "exact post-transform Cargo package input, NOT original prepared manifest"}));
                }
                _ => {
                    assert!(
                        allowed_files.contains(file),
                        "archive escaped explicit allowlist: {name}:{file}"
                    );
                    assert_eq!(
                        *digest,
                        hash(&fs::read(generated.join(file)).unwrap()),
                        "archive payload changed: {name}:{file}"
                    );
                }
            }
        }
        let normalized_graph = proof.metadata(
            &format!("archive-metadata-{name}"),
            &payload,
            &packaging_home,
            true,
        );
        let actual = &array(&normalized_graph["packages"])[0];
        let original = original_package(&inventory, package);
        assert_eq!(actual["name"], package["name"]);
        assert_eq!(actual["version"], package["version"]);
        assert_eq!(actual["features"], original["features"]);
        assert_eq!(actual["links"], original["links"]);
        save(
            &manifest_evidence.join("declaration-comparison.json"),
            &json!({
                "original_dependencies": original["dependencies"], "actual_dependencies": actual["dependencies"],
                "allowlisted_expected_normal_build": dependency_contract(original, packages, true),
                "actual_normal_build": dependency_contract(actual, packages, false),
                "original_targets": original["targets"], "actual_targets": actual["targets"],
                "dev_only_targets_and_dependencies": "excluded from production contract; no equivalence claimed",
                "original_expanded_features": original["features"], "actual_expanded_features": actual["features"],
                "links": actual["links"]
            }),
        );
        assert_eq!(
            dependency_contract(actual, packages, false),
            dependency_contract(original, packages, true),
            "normal/build declaration changed for {name}"
        );
        assert_eq!(
            targets_contract(actual, Some(package)),
            targets_contract(original, None),
            "target/proc-macro declaration changed for {name}"
        );
        let entry = index_entry(&normalized, &checksum);
        let index_file = registry.join("index").join(index_path(&name));
        fs::create_dir_all(index_file.parent().unwrap()).unwrap();
        fs::write(
            &index_file,
            format!("{}\n", serde_json::to_string(&entry).unwrap()),
        )
        .unwrap();
        archives.push(json!({"original_id": package["original_id"], "name": name, "version": version, "sha256": checksum,
            "payload_hashes": payload_files, "cargo_generated_files": cargo_generated, "index": entry,
            "verification": "cargo package --no-verify; release consumer compiled separately"}));
        save(&evidence.join("archives.json"), &json!(archives));
        published.insert(name);
    }
    assert_eq!(archives.len(), packages.len());
    save(&evidence.join("archives.json"), &json!(archives));

    let hal = packages
        .iter()
        .find(|p| p["original_name"] == "esp-hal")
        .unwrap();
    let registry_consumer = work.join("registry-consumer");
    consumer(
        &registry_consumer,
        &format!(
            "package = {}, version = {}",
            serde_json::to_string(&hal["name"]).unwrap(),
            serde_json::to_string(&format!("={}", text(hal, "version"))).unwrap()
        ),
    );
    seed_registry_lock(
        &proof,
        "primary-registry",
        &minimal.join("Cargo.lock"),
        &registry_consumer,
        packages,
        &archives,
    );
    let consumer_home = work.join("consumer-home");
    home(&consumer_home, Some(&index));
    assert!(!consumer_home.join("registry").exists() && !consumer_home.join("git").exists());
    let registry_graph = proof.metadata(
        "registry-minimal-graph",
        &registry_consumer,
        &consumer_home,
        false,
    );
    let consumer_input = manifest(&registry_consumer.join("Cargo.toml"));
    assert!(consumer_input.get("patch").is_none() && consumer_input.get("replace").is_none());
    for (_, _, dependencies) in dependency_tables(&consumer_input) {
        for dependency in dependencies.values() {
            assert!(dependency.get("path").is_none() && dependency.get("git").is_none());
        }
    }
    for package in array(&registry_graph["packages"]) {
        if package["id"] == registry_graph["resolve"]["root"] {
            continue;
        }
        assert_eq!(
            package["source"], "registry+https://github.com/rust-lang/crates.io-index",
            "registry-only consumer has non-default registry/path/Git source"
        );
    }
    let expected = graph_contract(&baseline_graph, packages, false);
    let actual = graph_contract(&registry_graph, packages, true);
    save(&evidence.join("baseline-logical-graph.json"), &expected);
    save(&evidence.join("registry-logical-graph.json"), &actual);
    record_graph_differences(
        &proof,
        "graph-differences",
        &baseline_graph,
        &registry_graph,
        packages,
        &expected,
        &actual,
        &index,
    );
    assert_eq!(
        actual,
        expected,
        "registry normal/build graph changed outside allowed identity rewrites; inspect logical graphs in {}",
        evidence.display()
    );
    assert!(
        !consumer_home.join("git").exists(),
        "registry-only consumer fetched Git sources"
    );
    let baseline_units = build_units(
        &mut proof,
        "baseline-release-rev103",
        &minimal,
        &baseline_home,
        &baseline_graph,
        packages,
        false,
    );
    save(
        &evidence.join("baseline-compiler-units.json"),
        &baseline_units,
    );
    let registry_units = build_units(
        &mut proof,
        "registry-release-rev103",
        &registry_consumer,
        &consumer_home,
        &registry_graph,
        packages,
        true,
    );
    save(
        &evidence.join("compiler-unit-comparison.json"),
        &json!({
        "baseline": baseline_units, "registry": registry_units, "unallowlisted_differences": registry_units != baseline_units,
        "allowed_identity_mapping": "same original_id mapping as graph-differences.json",
        "host_target_contexts_and_features_must_be_equal": true}),
    );
    save(
        &evidence.join("registry-compiler-units.json"),
        &registry_units,
    );
    assert_eq!(
        registry_units, baseline_units,
        "host/build/proc-macro/target feature units differ"
    );
    assert!(
        !consumer_home.join("git").exists(),
        "registry-only release build fetched Git sources"
    );
    let baseline_executable = minimal
        .join("target")
        .join(TARGET)
        .join("release/registry-proof-firmware");
    fs::copy(
        &baseline_executable,
        evidence.join("baseline-proof-firmware.elf"),
    )
    .unwrap();
    let executable = registry_consumer
        .join("target")
        .join(TARGET)
        .join("release/registry-proof-firmware");
    assert!(executable.is_file());
    fs::copy(&executable, evidence.join("registry-proof-firmware.elf")).unwrap();
    fs::copy(
        registry_consumer.join("Cargo.lock"),
        evidence.join("registry-consumer.Cargo.lock"),
    )
    .unwrap();
    fs::copy(
        registry_consumer.join("Cargo.toml"),
        evidence.join("registry-consumer.Cargo.toml"),
    )
    .unwrap();
    supplemental_macro_proof(
        &mut proof, &prepared, hal, packages, &archives, work, &index, &expected,
    );
    let after: BTreeMap<_, _> = before
        .keys()
        .map(|id| {
            let package = array(&inventory["packages"])
                .iter()
                .find(|p| p["id"].as_str() == Some(id))
                .unwrap();
            (
                id.clone(),
                files(
                    Path::new(package["manifest_path"].as_str().unwrap())
                        .parent()
                        .unwrap(),
                ),
            )
        })
        .collect();
    save(
        &evidence.join("source-after.json"),
        &serde_json::to_value(&after).unwrap(),
    );
    assert_eq!(before, after, "proof modified prepared/user source files");
    let requests = fs::read_to_string(evidence.join("http-requests.jsonl")).unwrap();
    for line in requests.lines() {
        let request: Value = serde_json::from_str(line).unwrap();
        assert_eq!(
            request["method"], "GET",
            "proof attempted a registry upload/API mutation"
        );
    }
    let download_names: BTreeSet<_> = requests
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .filter(|request| {
            request["status"] == 200
                && request["path"]
                    .as_str()
                    .is_some_and(|p| p.starts_with("/archives/"))
        })
        .map(|request| {
            request["path"]
                .as_str()
                .unwrap()
                .split('/')
                .nth(2)
                .unwrap()
                .to_owned()
        })
        .collect();
    for package in array(&registry_graph["packages"]) {
        if packages.iter().any(|mapping| {
            mapping["name"] == package["name"] && mapping["version"] == package["version"]
        }) {
            assert!(
                download_names.contains(package["name"].as_str().unwrap()),
                "fresh consumer did not fetch generated archive from loopback: {}",
                package["name"]
            );
        }
    }
    save(
        &evidence.join("proof.json"),
        &json!({"schema": 1, "target": TARGET, "minimum_chip_revision": 103,
        "distribution_packages": packages.len(), "all_packages_packaged": true,
        "registry_only_release": true, "source_unchanged": true, "archive_payload_comparison": true,
        "normal_build_graph_comparison": true, "compiler_feature_unit_comparison": true, "resolver": "3",
        "target_inactive_publication_normalization": {
            "dependencies": ["xtensa-lx", "xtensa-lx-rt"], "target": "cfg(target_arch = \"xtensa\")",
            "rewrite": "path to default crates.io, all other dependency semantics unchanged",
            "reason": "target-inactive publication normalization, equivalence outside tested P4 target NOT claimed"
        },
        "elf_sha256": hash(&fs::read(executable).unwrap()),
        "baseline_elf_sha256": hash(&fs::read(baseline_executable).unwrap()),
        "crates_io_readiness": "not attempted; generated closure versions must be registered before public resolution",
        "publication_attempted": false, "package_verification": "no-verify; release consumer build is the active-target verification",
        "primary_requested_hal_features": ["esp32p4"], "primary_consumer_macro_coverage": ["main"],
        "supplemental_macro_features": ["esp32p4", "critical-section", "unstable"],
        "supplemental_consumer_macro_coverage": ["main", "handler", "ram rtc_fast zeroed trait assertion"],
        "hardware_exercised": false, "uploaded": false}),
    );
    eprintln!(
        "Registry distribution proof evidence: {}",
        evidence.display()
    );
}

#[test]
fn seed_registry_lock_maps_fragmentless_git_refs_without_merging_revisions() {
    let temporary = tempfile::tempdir().unwrap();
    let evidence = temporary.path().join("evidence");
    let destination = temporary.path().join("consumer");
    fs::create_dir_all(&evidence).unwrap();
    fs::create_dir_all(&destination).unwrap();
    let proof = Proof {
        evidence,
        sequence: 0,
    };
    let hal_source = "git+https://github.com/esp-rs/esp-pacs?rev=5a07030bbe72d57f82314a26ec42b7f93e03bd1d#5a07030bbe72d57f82314a26ec42b7f93e03bd1d";
    let rom_source = "git+https://github.com/esp-rs/esp-pacs?rev=fc3e6d4#fc3e6d4b34cfc885f750641d27e6fe3e63bad5d2";
    let mappings = vec![
        json!({"original_id": "hal-pac", "original_name": "esp32p4", "original_version": "0.2.0",
            "original_source": hal_source, "name": "esp32p4-p4-pre-v3-5a07030b", "version": "0.2.0-p4v13.1"}),
        json!({"original_id": "rom-pac", "original_name": "esp32p4", "original_version": "0.2.0",
            "original_source": rom_source, "name": "esp32p4-p4-pre-v3-fc3e6d4b", "version": "0.2.0-p4v13.1"}),
    ];
    let archives = vec![
        json!({"original_id": "hal-pac", "sha256": hash(b"hal-pac-archive-fixture")}),
        json!({"original_id": "rom-pac", "sha256": hash(b"rom-pac-archive-fixture")}),
    ];
    let mut lock = json!({"version": 4, "package": [
        {"name": "registry-proof-firmware", "version": "0.1.0", "dependencies": [
            "esp32p4 0.2.0 (git+https://github.com/esp-rs/esp-pacs?rev=5a07030bbe72d57f82314a26ec42b7f93e03bd1d)",
            "esp32p4 0.2.0 (git+https://github.com/esp-rs/esp-pacs?rev=fc3e6d4)"
        ]},
        {"name": "esp32p4", "version": "0.2.0", "source": hal_source},
        {"name": "esp32p4", "version": "0.2.0", "source": rom_source}
    ]});
    let seed = temporary.path().join("Cargo.lock");
    let value: toml::Value = serde_json::from_value(lock.clone()).unwrap();
    fs::write(&seed, toml::to_string(&value).unwrap()).unwrap();
    seed_registry_lock(
        &proof,
        "fragmentless",
        &seed,
        &destination,
        &mappings,
        &archives,
    );
    let rewritten = manifest(&destination.join("Cargo.lock"));
    let root = array(&rewritten["package"])
        .iter()
        .find(|p| p["name"] == "registry-proof-firmware")
        .unwrap();
    assert_eq!(
        root["dependencies"],
        json!([
            "esp32p4-p4-pre-v3-5a07030b 0.2.0-p4v13.1 (registry+https://github.com/rust-lang/crates.io-index)",
            "esp32p4-p4-pre-v3-fc3e6d4b 0.2.0-p4v13.1 (registry+https://github.com/rust-lang/crates.io-index)"
        ])
    );
    assert_eq!(
        array(&rewritten["package"])
            .iter()
            .filter(|p| p["name"] != "registry-proof-firmware")
            .count(),
        2
    );
    for (name, checksum) in [
        (
            "esp32p4-p4-pre-v3-5a07030b",
            hash(b"hal-pac-archive-fixture"),
        ),
        (
            "esp32p4-p4-pre-v3-fc3e6d4b",
            hash(b"rom-pac-archive-fixture"),
        ),
    ] {
        let package = array(&rewritten["package"])
            .iter()
            .find(|p| p["name"] == name)
            .unwrap();
        assert_eq!(package["checksum"], checksum);
        assert_eq!(
            package["source"],
            "registry+https://github.com/rust-lang/crates.io-index"
        );
    }
    // Without the query-revision discriminator the lock reference is ambiguous,
    // so selecting either same-name/same-version source would be unsound.
    lock["package"][0]["dependencies"] = json!(["esp32p4 0.2.0"]);
    let value: toml::Value = serde_json::from_value(lock).unwrap();
    fs::write(&seed, toml::to_string(&value).unwrap()).unwrap();
    let failure = std::panic::catch_unwind(|| {
        seed_registry_lock(
            &proof,
            "ambiguous",
            &seed,
            &destination,
            &mappings,
            &archives,
        )
    });
    let diagnostic = failure.expect_err("ambiguous same-version sources must be rejected");
    let message = diagnostic
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| diagnostic.downcast_ref::<&str>().copied())
        .unwrap();
    assert!(
        message.contains("ambiguous locked source identity"),
        "{message}"
    );
}
