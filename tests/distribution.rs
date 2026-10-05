use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

fn distribute(prepared: &Path, graph: &Path, destination: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_cargo-esp32p4-pre-v3"))
        .arg("distribute")
        .arg(prepared)
        .arg(graph)
        .arg(destination)
        .output()
        .unwrap()
}

#[test]
fn invalid_input_cannot_create_or_modify_destination() {
    let temporary = tempfile::tempdir().unwrap();
    let prepared = temporary.path().join("prepared");
    fs::create_dir(&prepared).unwrap();
    let graph = temporary.path().join("metadata.json");
    fs::write(&graph, b"{}").unwrap();
    let destination = temporary.path().join("distribution");
    assert!(!distribute(&prepared, &graph, &destination).status.success());
    assert!(!destination.exists());
    fs::create_dir(&destination).unwrap();
    fs::write(destination.join("sentinel"), b"untouched").unwrap();
    assert!(!distribute(&prepared, &graph, &destination).status.success());
    assert_eq!(
        fs::read(destination.join("sentinel")).unwrap(),
        b"untouched"
    );
    assert_eq!(fs::read_dir(&destination).unwrap().count(), 1);
}

#[test]
fn malformed_graph_and_unverified_sdk_are_rejected_without_output() {
    let temporary = tempfile::tempdir().unwrap();
    let prepared = temporary.path().join("prepared");
    fs::create_dir_all(prepared.join("upstream")).unwrap();
    fs::write(
        prepared.join("upstream/Cargo.toml"),
        "[workspace]\nmembers=[]\n",
    )
    .unwrap();
    let graph = temporary.path().join("metadata.json");
    let destination = temporary.path().join("distribution");
    for content in ["not json", "{\"packages\":[],\"resolve\":{\"nodes\":[]}}"] {
        fs::write(&graph, content).unwrap();
        assert!(!distribute(&prepared, &graph, &destination).status.success());
        assert!(!destination.exists());
    }
}

fn fixture_paths() -> (PathBuf, PathBuf) {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    (
        std::env::var_os("DISTRIBUTION_PREPARED")
            .map(PathBuf::from)
            .unwrap_or_else(|| root.join("target/distribution-validation/prepared-baseline")),
        std::env::var_os("DISTRIBUTION_METADATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| root.join("target/distribution-validation/prepared-graph.json")),
    )
}

#[test]
#[ignore = "requires a validated real prepared SDK and frozen consumer Cargo metadata"]
fn real_distribution_preserves_identity_aliases_and_is_deterministic() {
    let (prepared, graph) = fixture_paths();
    let temporary = tempfile::tempdir().unwrap();
    let generate = |destination: &Path| {
        let result = distribute(&prepared, &graph, destination);
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        serde_json::from_slice::<serde_json::Value>(
            &fs::read(destination.join("distribution.json")).unwrap(),
        )
        .unwrap()
    };
    let first = temporary.path().join("first");
    let first_manifest = generate(&first);
    assert_eq!(first_manifest, generate(&temporary.path().join("second")));
    let packages = first_manifest["packages"].as_array().unwrap();
    assert!(!packages.is_empty());
    assert!(packages.iter().all(|p| {
        p["original_source"]
            .as_str()
            .is_none_or(|s| !s.starts_with("registry+"))
    }));
    let pacs: Vec<_> = packages
        .iter()
        .filter(|p| p["original_name"] == "esp32p4")
        .collect();
    assert_eq!(
        pacs.len(),
        2,
        "the HAL and ROM pins are different source identities"
    );
    assert_ne!(pacs[0]["name"], pacs[1]["name"]);
    assert_ne!(pacs[0]["original_id"], pacs[1]["original_id"]);
    let hal = packages
        .iter()
        .find(|p| p["original_name"] == "esp-hal")
        .unwrap();
    let manifest: toml::Value = fs::read_to_string(
        first
            .join(hal["directory"].as_str().unwrap())
            .join("Cargo.toml"),
    )
    .unwrap()
    .parse()
    .unwrap();
    let proc = packages
        .iter()
        .find(|p| p["original_name"] == "esp-hal-procmacros")
        .unwrap();
    assert_eq!(
        manifest["dependencies"]["procmacros"]["package"].as_str(),
        proc["name"].as_str()
    );
    assert_eq!(
        manifest["dependencies"]["procmacros"]["version"].as_str(),
        Some(format!("={}", proc["version"].as_str().unwrap()).as_str())
    );
    assert!(manifest["workspace"].is_table());
    assert_eq!(
        fs::read(
            first
                .join(hal["directory"].as_str().unwrap())
                .join("src/lib.rs")
        )
        .unwrap(),
        fs::read(prepared.join("upstream/esp-hal/src/lib.rs")).unwrap()
    );
    for package in packages {
        assert_eq!(package["source_tree_hash"].as_str().unwrap().len(), 64);
        assert_eq!(package["generated_tree_hash"].as_str().unwrap().len(), 64);
        assert!(!package["reasons"].as_array().unwrap().is_empty());
        assert!(!package["source_files"].as_array().unwrap().is_empty());
        let manifest: toml::Value = fs::read_to_string(
            first
                .join(package["directory"].as_str().unwrap())
                .join("Cargo.toml"),
        )
        .unwrap()
        .parse()
        .unwrap();
        assert!(manifest["package"]["include"].as_array().unwrap().len() > 1);
        assert!(manifest.get("patch").is_none());
    }
    let sentinel = first.join("sentinel");
    fs::write(&sentinel, b"keep").unwrap();
    assert!(!distribute(&prepared, &graph, &first).status.success());
    assert_eq!(fs::read(&sentinel).unwrap(), b"keep");
}

#[test]
#[ignore = "requires a validated real prepared SDK and frozen consumer Cargo metadata"]
fn real_distribution_rejects_a_modified_sdk_and_stale_graph() {
    let (prepared, graph) = fixture_paths();
    let temporary = tempfile::tempdir().unwrap();
    let metadata: serde_json::Value = serde_json::from_slice(&fs::read(&graph).unwrap()).unwrap();
    let mut stale = metadata;
    stale["resolve"]["nodes"][0]["features"] = serde_json::json!(["invented-feature"]);
    let stale_path = temporary.path().join("stale.json");
    fs::write(&stale_path, serde_json::to_vec(&stale).unwrap()).unwrap();
    let destination = temporary.path().join("destination");
    assert!(
        !distribute(&prepared, &stale_path, &destination)
            .status
            .success()
    );
    assert!(!destination.exists());
    // Reconstruct a real pinned source tree offline; do not mutate the caller's
    // validated fixture. Then prove an unauthorized source edit fails closed.
    let tampered = temporary.path().join("tampered-prepared");
    fs::create_dir(&tampered).unwrap();
    let result = Command::new("git")
        .args(["clone", "--quiet", "--shared"])
        .arg(prepared.join("upstream"))
        .arg(tampered.join("upstream"))
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let mut child = Command::new("git")
        .current_dir(tampered.join("upstream"))
        .args(["apply", "--whitespace=nowarn"])
        .stdin(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    use std::io::Write;
    child
        .stdin
        .take()
        .unwrap()
        .write_all(include_bytes!("../patches/esp32p4-pre-v3.patch"))
        .unwrap();
    assert!(child.wait().unwrap().success());
    let hal_source = tampered.join("upstream/esp-hal/src/lib.rs");
    fs::OpenOptions::new()
        .append(true)
        .open(hal_source)
        .unwrap()
        .write_all(b"\n// unauthorized source modification\n")
        .unwrap();
    assert!(!distribute(&tampered, &graph, &destination).status.success());
    assert!(!destination.exists());
}
