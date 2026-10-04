//! Deterministic, fail-closed packaging of the pinned prepared dependency graph.
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use toml::Value;

type Result<T> = std::result::Result<T, String>;
const TARGET: &str = "riscv32imafc-unknown-none-elf";
const PAC_REPOSITORY: &str = "https://github.com/esp-rs/esp-pacs";
const PAC_PINS: &[(&str, &str, &str)] = &[
    (
        "esp32",
        "0f4c316adbdbddb2479f50a5b931776b9655b23f",
        "0f4c316adbdbddb2479f50a5b931776b9655b23f",
    ),
    (
        "esp32s2",
        "0f4c316adbdbddb2479f50a5b931776b9655b23f",
        "0f4c316adbdbddb2479f50a5b931776b9655b23f",
    ),
    (
        "esp32s3",
        "0f4c316adbdbddb2479f50a5b931776b9655b23f",
        "0f4c316adbdbddb2479f50a5b931776b9655b23f",
    ),
    (
        "esp32p4",
        "5a07030bbe72d57f82314a26ec42b7f93e03bd1d",
        "5a07030bbe72d57f82314a26ec42b7f93e03bd1d",
    ),
    (
        "esp32p4",
        "fc3e6d4",
        "fc3e6d4b34cfc885f750641d27e6fe3e63bad5d2",
    ),
];

#[derive(Deserialize, Serialize, Debug, PartialEq)]
struct Metadata {
    packages: Vec<Package>,
    resolve: Resolve,
    workspace_members: Vec<String>,
}
#[derive(Deserialize, Serialize, Debug, PartialEq)]
struct Resolve {
    root: Option<String>,
    nodes: Vec<Node>,
}
#[derive(Deserialize, Serialize, Debug, PartialEq)]
struct Node {
    id: String,
    deps: Vec<NodeDep>,
    features: Vec<String>,
}
#[derive(Deserialize, Serialize, Debug, PartialEq)]
struct NodeDep {
    name: String,
    pkg: String,
    dep_kinds: Vec<DepKind>,
}
#[derive(Deserialize, Serialize, Debug, PartialEq)]
struct DepKind {
    kind: Option<String>,
    target: Option<String>,
}
#[derive(Deserialize, Serialize, Debug, PartialEq)]
struct Package {
    name: String,
    version: String,
    id: String,
    manifest_path: PathBuf,
    source: Option<String>,
    links: Option<String>,
    dependencies: Vec<Dependency>,
    features: BTreeMap<String, Vec<String>>,
    targets: Vec<serde_json::Value>,
}
#[derive(Deserialize, Serialize, Debug, PartialEq)]
struct Dependency {
    name: String,
    rename: Option<String>,
    req: String,
    kind: Option<String>,
    optional: bool,
    target: Option<String>,
    uses_default_features: bool,
    features: Vec<String>,
    source: Option<String>,
    path: Option<PathBuf>,
}
#[derive(Serialize)]
struct Distribution {
    version: u32,
    baseline: Baseline,
    packages: Vec<DistPackage>,
    registry_packages: Vec<RegistryPackage>,
    source_validation: SourceValidation,
}
#[derive(Serialize)]
struct Baseline {
    target: &'static str,
    graph_sha256: String,
    upstream_revision: String,
    upstream_repository: String,
    patch_sha256: String,
    consumer_id: String,
    consumer_features: Vec<String>,
    consumer_manifest_sha256: String,
    consumer_lock_sha256: String,
    minimum_chip_revision: u32,
}
#[derive(Serialize)]
struct SourceValidation {
    sdk_tree_hash: String,
    sdk_files: Vec<FileHash>,
    ignored_directories: Vec<&'static str>,
}
#[derive(Serialize)]
struct RegistryPackage {
    original_id: String,
    name: String,
    version: String,
    source: String,
    features: Vec<String>,
    roles: Vec<String>,
    links: Option<String>,
    declared_features: BTreeMap<String, Vec<String>>,
    dependencies: Vec<serde_json::Value>,
    targets: Vec<serde_json::Value>,
}
#[derive(Serialize)]
struct DistPackage {
    original_id: String,
    original_name: String,
    original_version: String,
    original_source: Option<String>,
    name: String,
    version: String,
    directory: String,
    source_repository: String,
    links: Option<String>,
    features: Vec<String>,
    declared_features: BTreeMap<String, Vec<String>>,
    rewrites: Vec<Rewrite>,
    source_tree_hash: String,
    generated_tree_hash: String,
    source_files: Vec<FileHash>,
    generated_files: Vec<FileHash>,
    reasons: Vec<String>,
    original_manifest_path: PathBuf,
    dependencies: Vec<serde_json::Value>,
    targets: Vec<serde_json::Value>,
    inclusion_reason: String,
    roles: Vec<String>,
    git_rev: Option<String>,
    git_tree: String,
}
#[derive(Serialize)]
struct Rewrite {
    file: String,
    from: String,
    to: String,
    reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    function: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    line: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    old_sha256: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    new_sha256: Option<String>,
}
impl Rewrite {
    fn manifest(from: String, to: String, reason: &str) -> Self {
        Self {
            file: "Cargo.toml".into(),
            from,
            to,
            reason: reason.into(),
            function: None,
            line: None,
            old_sha256: None,
            new_sha256: None,
        }
    }
}
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct FileHash {
    path: String,
    sha256: String,
}
struct Identity {
    name: String,
    version: String,
}

fn io<T>(value: std::io::Result<T>) -> Result<T> {
    value.map_err(|e| e.to_string())
}
fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn command(cmd: &mut Command) -> Result<String> {
    let output = io(cmd.output())?;
    if !output.status.success() {
        return Err(format!(
            "command {:?} failed: {}",
            cmd,
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    String::from_utf8(output.stdout).map_err(|e| e.to_string())
}
fn git(dir: &Path, args: &[&str]) -> Result<String> {
    command(Command::new("git").current_dir(dir).args(args)).map(|s| s.trim().into())
}
fn apply_patch(dir: &Path) -> Result<()> {
    let mut child = io(Command::new("git")
        .current_dir(dir)
        .args(["apply", "--whitespace=nowarn"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn())?;
    io(child
        .stdin
        .take()
        .ok_or("git apply has no stdin")?
        .write_all(crate::PATCH_BYTES))?;
    let output = io(child.wait_with_output())?;
    if !output.status.success() {
        return Err(format!(
            "embedded patch failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(())
}
fn snapshot(repository: &Path, revision: &str, destination: &Path) -> Result<()> {
    command(
        Command::new("git")
            .args(["clone", "--quiet", "--shared", "--no-checkout"])
            .arg(repository)
            .arg(destination),
    )?;
    git(destination, &["checkout", "--quiet", "--detach", revision])?;
    Ok(())
}

// A check followed by std::fs::rename can overwrite an empty directory created
// by another process in between. Commit with the operating system's no-replace
// rename so even that destination stays untouched.
fn commit_directory(source: &Path, destination: &Path) -> Result<()> {
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;
        let source = CString::new(source.as_os_str().as_bytes()).map_err(|e| e.to_string())?;
        let destination =
            CString::new(destination.as_os_str().as_bytes()).map_err(|e| e.to_string())?;
        #[cfg(target_os = "macos")]
        unsafe extern "C" {
            fn renamex_np(
                source: *const std::ffi::c_char,
                destination: *const std::ffi::c_char,
                flags: u32,
            ) -> i32;
        }
        #[cfg(target_os = "linux")]
        unsafe extern "C" {
            fn renameat2(
                source_fd: i32,
                source: *const std::ffi::c_char,
                destination_fd: i32,
                destination: *const std::ffi::c_char,
                flags: u32,
            ) -> i32;
        }
        // SAFETY: both C strings live for the call; the OS copies the paths.
        #[cfg(target_os = "macos")]
        let status = unsafe { renamex_np(source.as_ptr(), destination.as_ptr(), 4) }; // RENAME_EXCL
        #[cfg(target_os = "linux")]
        let status = unsafe { renameat2(-100, source.as_ptr(), -100, destination.as_ptr(), 1) }; // AT_FDCWD, RENAME_NOREPLACE
        if status == 0 {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error().to_string())
        }
    }
    #[cfg(target_os = "windows")]
    {
        io(fs::rename(source, destination))
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        let _ = (source, destination);
        Err("atomic no-replace distribution commit is unavailable on this platform".into())
    }
}

// Hash records encode path and content lengths, rather than concatenating ambiguous
// path/content bytes. Symlinks cannot redirect the declared source payload.
fn inventory(root: &Path) -> Result<Vec<FileHash>> {
    fn walk(root: &Path, dir: &Path, records: &mut Vec<FileHash>) -> Result<()> {
        for entry in io(fs::read_dir(dir))? {
            let entry = io(entry)?;
            let kind = io(entry.file_type())?;
            if kind.is_symlink() {
                return Err(format!(
                    "source symlink is not authorized: {}",
                    entry.path().display()
                ));
            }
            if kind.is_dir() {
                if entry.file_name() != ".git" && entry.file_name() != "target" {
                    walk(root, &entry.path(), records)?;
                }
            } else if kind.is_file() {
                let path = entry.path();
                let relative = path
                    .strip_prefix(root)
                    .map_err(|e| e.to_string())?
                    .to_str()
                    .ok_or("source path is not UTF-8")?
                    .replace('\\', "/");
                records.push(FileHash {
                    path: relative,
                    sha256: digest(&io(fs::read(path))?),
                });
            } else {
                return Err(format!(
                    "unsupported source file: {}",
                    entry.path().display()
                ));
            }
        }
        Ok(())
    }
    let mut files = Vec::new();
    walk(root, root, &mut files)?;
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(files)
}
fn tree_hash(files: &[FileHash]) -> String {
    let mut hash = Sha256::new();
    hash.update(b"esp32p4-distribution-tree-v1\0");
    for file in files {
        hash.update((file.path.len() as u64).to_be_bytes());
        hash.update(file.path.as_bytes());
        hash.update((file.sha256.len() as u64).to_be_bytes());
        hash.update(file.sha256.as_bytes());
    }
    format!("{:x}", hash.finalize())
}
fn same_tree(actual: &Path, expected: &Path) -> Result<Vec<FileHash>> {
    let actual_files = inventory(actual)?;
    let expected_files = inventory(expected)?;
    if actual_files != expected_files {
        let actual_map: BTreeMap<_, _> =
            actual_files.iter().map(|f| (&f.path, &f.sha256)).collect();
        let expected_map: BTreeMap<_, _> = expected_files
            .iter()
            .map(|f| (&f.path, &f.sha256))
            .collect();
        let changed: Vec<_> = actual_map
            .keys()
            .chain(expected_map.keys())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .filter(|p| actual_map.get(**p) != expected_map.get(**p))
            .take(8)
            .collect();
        return Err(format!(
            "source payload differs from pinned tree: {:?}",
            changed
        ));
    }
    Ok(actual_files)
}
fn copy_files(source: &Path, destination: &Path, files: &[FileHash]) -> Result<()> {
    io(fs::create_dir_all(destination))?;
    for file in files {
        let dest = destination.join(&file.path);
        io(fs::create_dir_all(
            dest.parent().ok_or("file has no parent")?,
        ))?;
        io(fs::copy(source.join(&file.path), dest))?;
    }
    Ok(())
}
fn parse_manifest(path: &Path) -> Result<Value> {
    io(fs::read_to_string(path))?
        .parse::<Value>()
        .map_err(|e| e.to_string())
}
fn workspace_manifest(path: &Path) -> Result<Option<(PathBuf, Value)>> {
    let manifest = parse_manifest(path)?;
    if manifest.get("workspace").is_some() {
        return Ok(Some((
            path.parent().ok_or("manifest parent missing")?.into(),
            manifest,
        )));
    }
    if let Some(root) = manifest
        .get("package")
        .and_then(|p| p.get("workspace"))
        .and_then(Value::as_str)
    {
        let root = path.parent().ok_or("manifest parent missing")?.join(root);
        return Ok(Some((
            root.clone(),
            parse_manifest(&root.join("Cargo.toml"))?,
        )));
    }
    for ancestor in path
        .parent()
        .ok_or("manifest parent missing")?
        .ancestors()
        .skip(1)
    {
        let candidate = ancestor.join("Cargo.toml");
        if candidate.is_file() {
            let manifest = parse_manifest(&candidate)?;
            if manifest.get("workspace").is_some() {
                return Ok(Some((ancestor.into(), manifest)));
            }
        }
    }
    Ok(None)
}
fn flatten_inheritance(manifest: &mut Value, workspace: Option<&Value>) -> Result<()> {
    let root = manifest.as_table_mut().ok_or("manifest is not a table")?;
    let package = root
        .get_mut("package")
        .and_then(Value::as_table_mut)
        .ok_or("package table missing")?;
    package.remove("workspace");
    for (key, field) in package.iter_mut() {
        if field.get("workspace").and_then(Value::as_bool) == Some(true) {
            *field = workspace
                .and_then(|w| w.get("workspace"))
                .and_then(|w| w.get("package"))
                .and_then(|p| p.get(key))
                .cloned()
                .ok_or_else(|| format!("missing inherited package field {key}"))?;
        }
    }
    if root
        .get("lints")
        .and_then(|v| v.get("workspace"))
        .and_then(Value::as_bool)
        == Some(true)
    {
        let lints = workspace
            .and_then(|w| w.get("workspace"))
            .and_then(|w| w.get("lints"))
            .cloned()
            .ok_or("missing workspace lints")?;
        root.insert("lints".into(), lints);
    }
    Ok(())
}
fn flatten_dependency(value: &mut Value, alias: &str, workspace: Option<&Value>) -> Result<()> {
    if value.get("workspace").and_then(Value::as_bool) != Some(true) {
        return Ok(());
    }
    let overrides = value
        .as_table()
        .ok_or("inherited dependency not a table")?
        .clone();
    let inherited = workspace
        .and_then(|w| w.get("workspace"))
        .and_then(|w| w.get("dependencies"))
        .and_then(|w| w.get(alias))
        .cloned()
        .ok_or_else(|| format!("missing inherited dependency {alias}"))?;
    let mut table = match inherited {
        Value::String(version) => {
            let mut table = toml::map::Map::new();
            table.insert("version".into(), Value::String(version));
            table
        }
        Value::Table(table) => table,
        _ => return Err("invalid workspace dependency".into()),
    };
    for (key, val) in overrides {
        if key == "workspace" {
            continue;
        }
        if key == "features" {
            let existing = table.entry(key).or_insert_with(|| Value::Array(Vec::new()));
            existing
                .as_array_mut()
                .ok_or("invalid dependency features")?
                .extend(
                    val.as_array()
                        .ok_or("invalid dependency features")?
                        .iter()
                        .cloned(),
                );
        } else {
            table.insert(key, val);
        }
    }
    *value = Value::Table(table);
    Ok(())
}
fn distribution_name(package: &Package, duplicate_name: bool) -> String {
    if duplicate_name {
        let revision = package
            .source
            .as_deref()
            .and_then(|s| s.rsplit_once('#'))
            .map(|(_, r)| r)
            .unwrap_or("");
        return format!(
            "{}-p4-pre-v3-{}",
            package.name,
            &revision[..revision.len().min(7)]
        );
    }
    format!("{}-p4-pre-v3", package.name)
}
fn registry(source: Option<&str>) -> bool {
    matches!(
        source,
        Some("registry+https://github.com/rust-lang/crates.io-index")
            | Some("registry+sparse+https://index.crates.io/")
    )
}
fn selected_closure(metadata: &Metadata) -> Result<BTreeMap<String, BTreeSet<String>>> {
    let root = metadata
        .resolve
        .root
        .as_ref()
        .ok_or("metadata must have one consumer root")?;
    let nodes: BTreeMap<_, _> = metadata
        .resolve
        .nodes
        .iter()
        .map(|n| (n.id.as_str(), n))
        .collect();
    let packages: BTreeMap<_, _> = metadata
        .packages
        .iter()
        .map(|p| (p.id.as_str(), p))
        .collect();
    if nodes.len() != metadata.resolve.nodes.len() {
        return Err("duplicate resolve package ID".into());
    }
    let mut roles = BTreeMap::<String, BTreeSet<String>>::new();
    roles
        .entry(root.clone())
        .or_default()
        .insert("consumer".into());
    let mut queue = VecDeque::from([(root.as_str(), false)]);
    let mut visited = BTreeSet::new();
    while let Some((id, host)) = queue.pop_front() {
        if !visited.insert((id, host)) {
            continue;
        }
        let node = nodes
            .get(id)
            .ok_or_else(|| format!("resolve node missing: {id}"))?;
        for dep in &node.deps {
            let package = packages
                .get(dep.pkg.as_str())
                .ok_or("resolved package missing")?;
            let proc_macro = package.targets.iter().any(|t| {
                t.get("kind")
                    .and_then(serde_json::Value::as_array)
                    .is_some_and(|k| k.iter().any(|k| k == "proc-macro"))
            });
            for kind in &dep.dep_kinds {
                let kind_name = kind.kind.as_deref().unwrap_or("normal");
                if kind_name == "normal" || kind_name == "build" {
                    let dependency_host = host || kind_name == "build" || proc_macro;
                    let role = roles.entry(dep.pkg.clone()).or_default();
                    role.insert(kind_name.into());
                    role.insert(if dependency_host { "host" } else { "target" }.into());
                    if proc_macro {
                        role.insert("proc-macro".into());
                    }
                    if kind_name == "build" {
                        role.insert("host-build".into());
                    }
                    queue.push_back((dep.pkg.as_str(), dependency_host));
                } else if kind_name != "dev" {
                    return Err(format!("unknown dependency kind {kind_name}"));
                }
            }
        }
    }
    Ok(roles)
}
fn validate_graph(metadata: &Metadata) -> Result<()> {
    let root = metadata
        .resolve
        .root
        .as_ref()
        .ok_or("metadata has no consumer root")?;
    if metadata.workspace_members != [root.clone()] {
        return Err("distribution requires a single consumer workspace root".into());
    }
    let package = metadata
        .packages
        .iter()
        .find(|p| &p.id == root)
        .ok_or("consumer root package missing")?;
    let node = metadata
        .resolve
        .nodes
        .iter()
        .find(|n| &n.id == root)
        .ok_or("consumer root node missing")?;
    let mut cmd = Command::new("cargo");
    cmd.current_dir(
        package
            .manifest_path
            .parent()
            .ok_or("consumer directory missing")?,
    )
    .args([
        "metadata",
        "--offline",
        "--locked",
        "--format-version=1",
        "--filter-platform",
        TARGET,
        "--no-default-features",
    ])
    .arg("--manifest-path")
    .arg(&package.manifest_path)
    .env("ESP_HAL_CONFIG_MIN_CHIP_REVISION", "103");
    if !node.features.is_empty() {
        cmd.arg("--features").arg(node.features.join(","));
    }
    let fresh: Metadata = serde_json::from_str(&command(&mut cmd)?).map_err(|e| e.to_string())?;
    if &fresh != metadata {
        return Err("frozen metadata differs from the locked prepared consumer graph".into());
    }
    Ok(())
}
fn dependency_package<'a>(
    dependency: &Dependency,
    packages: &'a [Package],
) -> Result<Option<&'a Package>> {
    if let Some(path) = &dependency.path {
        let manifest = io(path.join("Cargo.toml").canonicalize())?;
        let mut found = packages
            .iter()
            .filter(|p| p.name == dependency.name && p.manifest_path == manifest);
        if let Some(package) = found.next()
            && found.next().is_none()
        {
            return Ok(Some(package));
        }
        return Err(format!(
            "path dependency identity missing or ambiguous: {}",
            dependency.name
        ));
    }
    if let Some(source) = &dependency.source {
        if source.starts_with("git+") {
            let mut found = packages.iter().filter(|p| {
                p.name == dependency.name
                    && p.source
                        .as_deref()
                        .and_then(|s| s.rsplit_once('#'))
                        .map(|(s, _)| s)
                        == Some(source.as_str())
            });
            if let Some(package) = found.next()
                && found.next().is_none()
            {
                return Ok(Some(package));
            }
            return Err(format!(
                "Git dependency identity missing or ambiguous: {}",
                dependency.name
            ));
        }
        if !registry(Some(source)) {
            return Err(format!(
                "custom registry dependency is not authorized: {source}"
            ));
        }
    }
    Ok(None)
}
#[allow(clippy::too_many_arguments)]
fn rewrite_section(
    table: &mut toml::map::Map<String, Value>,
    kind: &str,
    target: Option<&str>,
    package: &Package,
    packages: &[Package],
    identities: &BTreeMap<String, Identity>,
    workspace: Option<&Value>,
    rewrites: &mut Vec<Rewrite>,
) -> Result<()> {
    for (alias, value) in table.iter_mut() {
        flatten_dependency(value, alias, workspace)?;
        let mut candidates = package.dependencies.iter().filter(|d| {
            d.rename.as_ref().unwrap_or(&d.name) == alias
                && d.kind.as_deref().unwrap_or("normal") == kind
                && d.target.as_deref() == target
        });
        let dependency = candidates.next().ok_or_else(|| {
            format!(
                "dependency declaration missing: {} {kind} {target:?} {alias}",
                package.name
            )
        })?;
        if candidates.next().is_some() {
            return Err(format!(
                "dependency declaration identity ambiguous: {} {kind} {target:?} {alias}",
                package.name
            ));
        }
        let before = value.to_string();
        let inactive_xtensa = target == Some("cfg(target_arch = \"xtensa\")")
            && matches!(dependency.name.as_str(), "xtensa-lx" | "xtensa-lx-rt")
            && dependency.path.is_some();
        if inactive_xtensa {
            if packages
                .iter()
                .any(|p| p.name == dependency.name && identities.contains_key(&p.id))
            {
                return Err("Xtensa publication exception is active in the P4 closure".into());
            }
            let table = value
                .as_table_mut()
                .ok_or("Xtensa dependency not a table")?;
            if table.remove("path").is_none() || !table.contains_key("version") {
                return Err("invalid Xtensa publication exception".into());
            }
            rewrites.push(Rewrite::manifest(
                before,
                value.to_string(),
                "target-inactive publication normalization; API equivalence outside P4 NOT claimed",
            ));
            continue;
        }
        let dependency_package = dependency_package(dependency, packages)?;
        if let Some(dependency_package) = dependency_package {
            let identity = identities.get(&dependency_package.id).ok_or_else(|| {
                format!(
                    "non-registry dependency outside exported closure: {}",
                    dependency_package.id
                )
            })?;
            let table = value
                .as_table_mut()
                .ok_or("source dependency not a table")?;
            for key in [
                "path",
                "git",
                "rev",
                "branch",
                "tag",
                "registry",
                "registry-index",
            ] {
                table.remove(key);
            }
            table.insert("package".into(), Value::String(identity.name.clone()));
            table.insert(
                "version".into(),
                Value::String(format!("={}", identity.version)),
            );
            table.insert(
                "path".into(),
                Value::String(format!("../{}", identity.name)),
            );
            rewrites.push(Rewrite::manifest(
                before,
                value.to_string(),
                &format!("exact package identity rewrite: {}", dependency_package.id),
            ));
        } else if let Some(table) = value.as_table_mut()
            && ["git", "path", "registry", "registry-index"]
                .iter()
                .any(|key| table.contains_key(*key))
        {
            return Err(format!(
                "unmapped dependency source: {} {alias}",
                package.name
            ));
        }
    }
    Ok(())
}
fn rewrite_manifest(
    package: &Package,
    source_manifest: &Path,
    source_files: &[FileHash],
    packages: &[Package],
    identities: &BTreeMap<String, Identity>,
) -> Result<(Value, Vec<Rewrite>)> {
    let mut manifest = parse_manifest(source_manifest)?;
    let workspace = workspace_manifest(source_manifest)?;
    let workspace_value = workspace.as_ref().map(|(_, v)| v);
    let original_manifest = manifest.clone();
    flatten_inheritance(&mut manifest, workspace_value)?;
    let identity = identities
        .get(&package.id)
        .ok_or("package identity not exported")?;
    let mut rewrites = Vec::new();
    let root = manifest.as_table_mut().ok_or("invalid Cargo manifest")?;
    let fields = root
        .get_mut("package")
        .and_then(Value::as_table_mut)
        .ok_or("package missing")?;
    fields.insert("name".into(), Value::String(identity.name.clone()));
    fields.insert("version".into(), Value::String(identity.version.clone()));
    if let Some(include) = fields.remove("include") {
        rewrites.push(Rewrite::manifest(
            include.to_string(),
            String::new(),
            "replace upstream inclusion patterns with exact source payload allowlist",
        ));
    }
    if let Some(exclude) = fields.remove("exclude") {
        rewrites.push(Rewrite::manifest(
            exclude.to_string(),
            String::new(),
            "replace upstream exclusion patterns with exact source payload allowlist",
        ));
    }
    let include = source_files
        .iter()
        .map(|file| Value::String(format!("/{}", file.path)))
        .collect();
    fields.insert("include".into(), Value::Array(include));
    rewrites.push(Rewrite::manifest(
        String::new(),
        fields["include"].to_string(),
        "explicit source-file allowlist; Cargo normalization records are inspected separately",
    ));
    rewrites.push(Rewrite::manifest(
        format!("{}@{}", package.name, package.version),
        format!("{}@{}", identity.name, identity.version),
        "standalone distribution package identity",
    ));
    // Every dropped dev declaration is enumerated. No dev edge enters the closure.
    fn drop_dev(
        table: &mut toml::map::Map<String, Value>,
        target: Option<&str>,
        rewrites: &mut Vec<Rewrite>,
    ) -> Result<()> {
        if let Some(dev) = table.remove("dev-dependencies") {
            for (alias, value) in dev.as_table().ok_or("invalid dev-dependencies")? {
                rewrites.push(Rewrite::manifest(format!("dev-dependencies {target:?} {alias} = {value}"), String::new(),
                    "dev-only dependency excluded from normal/build distribution closure; tests are not a consumer-equivalence claim"));
            }
        }
        Ok(())
    }
    drop_dev(root, None, &mut rewrites)?;
    for (section, kind) in [("dependencies", "normal"), ("build-dependencies", "build")] {
        if let Some(table) = root.get_mut(section) {
            rewrite_section(
                table.as_table_mut().ok_or("invalid dependency table")?,
                kind,
                None,
                package,
                packages,
                identities,
                workspace_value,
                &mut rewrites,
            )?;
        }
    }
    if let Some(targets) = root.get_mut("target") {
        for (target, sections) in targets.as_table_mut().ok_or("invalid target table")? {
            let sections = sections.as_table_mut().ok_or("invalid target section")?;
            drop_dev(sections, Some(target), &mut rewrites)?;
            for (section, kind) in [("dependencies", "normal"), ("build-dependencies", "build")] {
                if let Some(table) = sections.get_mut(section) {
                    rewrite_section(
                        table.as_table_mut().ok_or("invalid target dependencies")?,
                        kind,
                        Some(target),
                        package,
                        packages,
                        identities,
                        workspace_value,
                        &mut rewrites,
                    )?;
                }
            }
        }
    }
    if root.contains_key("patch") || root.contains_key("replace") {
        return Err("package-local overrides are not authorized".into());
    }
    let mut standalone = toml::map::Map::new();
    // Match the edition-2024 prepared consumer without leaking the SDK's resolver 2.
    standalone.insert("resolver".into(), Value::String("3".into()));
    root.insert("workspace".into(), Value::Table(standalone));
    rewrites.push(Rewrite::manifest(original_manifest.to_string(), manifest.to_string(),
        "complete manifest transformation: flattened workspace fields, package identity, exact internal dependencies, dev-only exclusion, explicit payload, isolated resolver 3"));
    Ok((manifest, rewrites))
}
fn rewrite_macros(directory: &Path, rewrites: &mut Vec<Rewrite>) -> Result<()> {
    for (file, line, function, from) in [
        ("interrupt.rs", 56, "handler", "crate_name(\"esp-hal\")"),
        ("lp_core.rs", 222, "load_lp_code", "crate_name(\"esp-hal\")"),
        ("ram.rs", 182, "ram", "crate_name(\"esp-hal\")"),
        (
            "unified_main.rs",
            137,
            "esp_hal_crate",
            "proc_macro_crate::crate_name(\"esp-hal\")",
        ),
    ] {
        let path = directory.join("src").join(file);
        let old = io(fs::read_to_string(&path))?;
        if old.matches(from).count() != 1
            || !old.lines().nth(line - 1).is_some_and(|l| l.contains(from))
        {
            return Err(format!(
                "pinned macro discovery call changed: {file}:{line}"
            ));
        }
        let to = from.replace("\"esp-hal\"", "\"esp-hal-p4-pre-v3\"");
        let new = old.replacen(from, &to, 1);
        io(fs::write(path, &new))?;
        rewrites.push(Rewrite {
            file: format!("src/{file}"),
            from: from.into(),
            to,
            reason: "precise proc_macro_crate discovery of the fork package; no HAL logic change"
                .into(),
            function: Some(function.into()),
            line: Some(line),
            old_sha256: Some(digest(old.as_bytes())),
            new_sha256: Some(digest(new.as_bytes())),
        });
    }
    Ok(())
}

pub fn distribute(prepared: &Path, metadata_json: &Path, destination: &Path) -> Result<()> {
    if fs::symlink_metadata(destination).is_ok() {
        return Err(format!(
            "destination already exists: {}",
            destination.display()
        ));
    }
    let parent = destination
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    if !parent.is_dir() {
        return Err(format!(
            "destination parent does not exist: {}",
            parent.display()
        ));
    }
    let upstream = io(prepared.join("upstream").canonicalize())?;
    let pinned: crate::Manifest =
        serde_json::from_str(crate::MANIFEST_JSON).map_err(|e| e.to_string())?;
    if digest(crate::PATCH_BYTES) != pinned.patch.sha256 {
        return Err("embedded patch digest mismatch".into());
    }
    if git(&upstream, &["rev-parse", "HEAD"])? != pinned.upstream.revision {
        return Err("prepared SDK HEAD does not match pinned revision".into());
    }
    let temporary = io(tempfile::Builder::new()
        .prefix(".distribution-")
        .tempdir_in(parent))?;
    let expected_sdk = temporary.path().join("expected-sdk");
    snapshot(&upstream, &pinned.upstream.revision, &expected_sdk)?;
    apply_patch(&expected_sdk)?;
    let sdk_files = same_tree(&upstream, &expected_sdk)?;
    let sdk_git_tree = git(&expected_sdk, &["rev-parse", "HEAD^{tree}"])?;
    let bytes = io(fs::read(metadata_json))?;
    let metadata: Metadata = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    let mut ids = BTreeSet::new();
    for package in &metadata.packages {
        if !ids.insert(&package.id) {
            return Err("duplicate metadata package identity".into());
        }
    }
    validate_graph(&metadata)?;
    let mut roles = selected_closure(&metadata)?;
    let root = metadata
        .resolve
        .root
        .as_ref()
        .ok_or("consumer root missing")?;
    let mut identities = BTreeMap::new();
    let mut registry_packages = Vec::new();
    let mut source_revisions = BTreeMap::new();
    let mut checked_pacs = BTreeMap::<PathBuf, (PathBuf, String)>::new();
    let mut source_git_trees = BTreeMap::new();
    let mut source_snapshots = BTreeMap::new();
    let mut names = BTreeSet::new();
    let mut original_names = BTreeMap::<&str, usize>::new();
    for package in &metadata.packages {
        if roles.contains_key(&package.id)
            && &package.id != root
            && !registry(package.source.as_deref())
        {
            *original_names.entry(&package.name).or_default() += 1;
        }
    }
    for package in &metadata.packages {
        if !roles.contains_key(&package.id) || &package.id == root {
            continue;
        }
        let node = metadata
            .resolve
            .nodes
            .iter()
            .find(|n| n.id == package.id)
            .ok_or("package resolve node missing")?;
        if registry(package.source.as_deref()) {
            registry_packages.push(RegistryPackage {
                original_id: package.id.clone(),
                name: package.name.clone(),
                version: package.version.clone(),
                source: package.source.clone().ok_or("registry source missing")?,
                features: node.features.clone(),
                roles: roles[&package.id].iter().cloned().collect(),
                links: package.links.clone(),
                declared_features: package.features.clone(),
                dependencies: package
                    .dependencies
                    .iter()
                    .map(|d| serde_json::to_value(d).map_err(|e| e.to_string()))
                    .collect::<Result<_>>()?,
                targets: package.targets.clone(),
            });
            continue;
        }
        let package_dir = io(package
            .manifest_path
            .parent()
            .ok_or("package directory missing")?
            .canonicalize())?;
        if package.source.is_none() && package_dir == upstream.join(&package.name) {
            source_revisions.insert(package.id.clone(), Some(pinned.upstream.revision.clone()));
            source_snapshots.insert(package.id.clone(), expected_sdk.join(&package.name));
            source_git_trees.insert(package.id.clone(), sdk_git_tree.clone());
        } else {
            let allowed_pin = PAC_PINS
                .iter()
                .find(|(name, rev, head)| {
                    package.name == *name
                        && package.source.as_deref()
                            == Some(format!("git+{PAC_REPOSITORY}?rev={rev}#{head}").as_str())
                })
                .ok_or_else(|| {
                    format!(
                        "source outside pinned SDK/PAC allowlist: {} {:?}",
                        package.id, package.source
                    )
                })?;
            let checkout = PathBuf::from(git(&package_dir, &["rev-parse", "--show-toplevel"])?);
            if package_dir != checkout.join(&package.name)
                || git(&checkout, &["rev-parse", "HEAD"])? != allowed_pin.2
            {
                return Err(format!("PAC checkout identity mismatch: {}", package.id));
            }
            if !checked_pacs.contains_key(&checkout) {
                let expected = temporary
                    .path()
                    .join(format!("expected-pac-{}", allowed_pin.2));
                snapshot(&checkout, allowed_pin.2, &expected)?;
                // Cargo's empty checkout-completion marker is cache bookkeeping,
                // not a repository payload file. All other extra files are rejected.
                let mut actual = inventory(&checkout)?;
                if let Some(marker) = actual
                    .iter()
                    .position(|f| f.path == ".cargo-ok" && f.sha256 == digest(b""))
                {
                    actual.remove(marker);
                }
                if actual != inventory(&expected)? {
                    return Err(format!(
                        "PAC payload differs from pinned Git tree: {}",
                        package.id
                    ));
                }
                let git_tree = git(&expected, &["rev-parse", "HEAD^{tree}"])?;
                checked_pacs.insert(checkout.clone(), (expected, git_tree));
            }
            source_snapshots.insert(
                package.id.clone(),
                checked_pacs[&checkout].0.join(&package.name),
            );
            source_git_trees.insert(package.id.clone(), checked_pacs[&checkout].1.clone());
            source_revisions.insert(package.id.clone(), Some(allowed_pin.2.to_string()));
        }
        let source_manifest = source_snapshots[&package.id].join("Cargo.toml");
        let manifest = parse_manifest(&source_manifest)?;
        let mut effective = manifest;
        let workspace = workspace_manifest(&source_manifest)?;
        flatten_inheritance(&mut effective, workspace.as_ref().map(|(_, v)| v))?;
        if effective
            .get("package")
            .and_then(|p| p.get("name"))
            .and_then(Value::as_str)
            != Some(&package.name)
            || effective
                .get("package")
                .and_then(|p| p.get("version"))
                .and_then(Value::as_str)
                != Some(&package.version)
        {
            return Err(format!(
                "metadata package identity disagrees with source manifest: {}",
                package.id
            ));
        }
        let name = distribution_name(package, original_names[package.name.as_str()] > 1);
        if !names.insert(name.clone()) {
            return Err(format!("generated package name collision: {name}"));
        }
        identities.insert(
            package.id.clone(),
            Identity {
                name,
                version: format!("{}-p4v13.1", package.version),
            },
        );
        let role = roles.get_mut(&package.id).ok_or("package roles missing")?;
        if package.targets.iter().any(|t| {
            t.get("kind")
                .and_then(serde_json::Value::as_array)
                .is_some_and(|k| k.iter().any(|k| k == "proc-macro"))
        }) {
            role.insert("proc-macro".into());
            role.insert("host".into());
        }
        if role.contains("build") {
            role.insert("host-build".into());
        }
    }
    if !identities.values().any(|i| i.name == "esp-hal-p4-pre-v3") {
        return Err("prepared closure does not contain esp-hal".into());
    }
    let generated = temporary.path().join("generated");
    io(fs::create_dir(&generated))?;
    let mut packages = Vec::new();
    for package in &metadata.packages {
        let Some(identity) = identities.get(&package.id) else {
            continue;
        };
        let directory = format!("crates/{}", identity.name);
        let dest = generated.join(&directory);
        let source = &source_snapshots[&package.id];
        let source_files = inventory(source)?;
        copy_files(source, &dest, &source_files)?;
        let (manifest, mut rewrites) = rewrite_manifest(
            package,
            &source.join("Cargo.toml"),
            &source_files,
            &metadata.packages,
            &identities,
        )?;
        io(fs::write(
            dest.join("Cargo.toml"),
            toml::to_string_pretty(&manifest).map_err(|e| e.to_string())?,
        ))?;
        if package.name == "esp-hal-procmacros" {
            rewrite_macros(&dest, &mut rewrites)?;
        }
        let generated_files = inventory(&dest)?;
        let original_manifest_hash = source_files
            .iter()
            .find(|f| f.path == "Cargo.toml")
            .ok_or("source manifest inventory missing")?
            .sha256
            .clone();
        let generated_manifest_hash = generated_files
            .iter()
            .find(|f| f.path == "Cargo.toml")
            .ok_or("generated manifest inventory missing")?
            .sha256
            .clone();
        for rewrite in rewrites.iter_mut().filter(|r| r.file == "Cargo.toml") {
            rewrite.old_sha256 = Some(original_manifest_hash.clone());
            rewrite.new_sha256 = Some(generated_manifest_hash.clone());
        }
        rewrites.sort_by(|a, b| (&a.file, &a.from).cmp(&(&b.file, &b.from)));
        let mut features = metadata
            .resolve
            .nodes
            .iter()
            .find(|n| n.id == package.id)
            .ok_or("node missing")?
            .features
            .clone();
        features.sort();
        packages.push(DistPackage {
            original_id: package.id.clone(),
            original_name: package.name.clone(),
            original_version: package.version.clone(),
            original_source: package.source.clone(),
            source_repository: if package.source.is_none() {
                pinned.upstream.repository.clone()
            } else {
                PAC_REPOSITORY.into()
            },
            name: identity.name.clone(),
            version: identity.version.clone(),
            directory,
            links: package.links.clone(),
            features,
            declared_features: package.features.clone(),
            rewrites,
            reasons: vec![format!(
                "{} source identity retained because normal/build resolution reaches {}",
                if package.source.is_none() {
                    "prepared SDK"
                } else {
                    "pinned Git PAC"
                },
                package.id
            )],
            original_manifest_path: package.manifest_path.clone(),
            dependencies: package
                .dependencies
                .iter()
                .map(|d| serde_json::to_value(d).map_err(|e| e.to_string()))
                .collect::<Result<_>>()?,
            targets: package.targets.clone(),
            source_tree_hash: tree_hash(&source_files),
            generated_tree_hash: tree_hash(&generated_files),
            source_files,
            generated_files,
            inclusion_reason: if package.source.is_none() {
                "pinned prepared SDK normal/build closure"
            } else {
                "declared pinned PAC normal/build closure"
            }
            .into(),
            roles: roles[&package.id].iter().cloned().collect(),
            git_rev: source_revisions[&package.id].clone(),
            git_tree: source_git_trees[&package.id].clone(),
        });
    }
    packages.sort_by(|a, b| a.name.cmp(&b.name));
    registry_packages.sort_by(|a, b| a.original_id.cmp(&b.original_id));
    let consumer = metadata
        .packages
        .iter()
        .find(|package| &package.id == root)
        .ok_or("consumer package missing")?;
    let consumer_features = metadata
        .resolve
        .nodes
        .iter()
        .find(|node| &node.id == root)
        .ok_or("consumer node missing")?
        .features
        .clone();
    let distribution = Distribution {
        version: 1,
        baseline: Baseline {
            target: TARGET,
            graph_sha256: digest(&bytes),
            upstream_revision: pinned.upstream.revision,
            upstream_repository: pinned.upstream.repository,
            patch_sha256: pinned.patch.sha256,
            consumer_id: root.clone(),
            consumer_features,
            consumer_manifest_sha256: digest(&io(fs::read(&consumer.manifest_path))?),
            consumer_lock_sha256: digest(&io(fs::read(
                consumer
                    .manifest_path
                    .parent()
                    .ok_or("consumer directory missing")?
                    .join("Cargo.lock"),
            ))?),
            minimum_chip_revision: 103,
        },
        packages,
        registry_packages,
        source_validation: SourceValidation {
            sdk_tree_hash: tree_hash(&sdk_files),
            sdk_files,
            ignored_directories: vec![".git", "target"],
        },
    };
    let mut json = serde_json::to_vec_pretty(&distribution).map_err(|e| e.to_string())?;
    json.push(b'\n');
    io(fs::write(generated.join("distribution.json"), json))?;
    // Publish only a complete sibling directory. Validation and generation failures
    // are removed by TempDir; an existing destination is never overwritten.
    if fs::symlink_metadata(destination).is_ok() {
        return Err("destination appeared during generation".into());
    }
    commit_directory(&generated, destination)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_package(path: &Path) -> Package {
        Package {
            name: "esp-hal".into(),
            version: "1.1.0".into(),
            id: "sdk-hal".into(),
            manifest_path: path.into(),
            source: None,
            links: None,
            dependencies: Vec::new(),
            features: BTreeMap::new(),
            targets: Vec::new(),
        }
    }

    #[test]
    fn registry_dependency_is_not_reclassified_as_a_source_fork() {
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().join("Cargo.toml");
        fs::write(&path, "[package]\nname='esp-hal'\nversion='1.1.0'\nedition='2024'\n[build-dependencies]\nautocfg='1'\n").unwrap();
        let mut package = fixture_package(&path);
        package.dependencies.push(Dependency {
            name: "autocfg".into(),
            rename: None,
            req: "^1".into(),
            kind: Some("build".into()),
            optional: false,
            target: None,
            uses_default_features: true,
            features: Vec::new(),
            source: Some("registry+https://github.com/rust-lang/crates.io-index".into()),
            path: None,
        });
        let identities = BTreeMap::from([(
            "sdk-hal".into(),
            Identity {
                name: "esp-hal-p4-pre-v3".into(),
                version: "1.1.0-p4v13.1".into(),
            },
        )]);
        let source_files = inventory(temporary.path()).unwrap();
        let (manifest, _) = rewrite_manifest(
            &package,
            &path,
            &source_files,
            &[fixture_package(&path)],
            &identities,
        )
        .unwrap();
        assert_eq!(
            manifest["build-dependencies"]["autocfg"].as_str(),
            Some("1")
        );
        assert!(
            dependency_package(&package.dependencies[0], &[])
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn closure_uses_resolved_normal_and_build_edges_not_dev_roots() {
        let mut package = fixture_package(Path::new("/not-opened/Cargo.toml"));
        package.id = "consumer".into();
        let mut normal = fixture_package(Path::new("/not-opened/Cargo.toml"));
        normal.id = "normal".into();
        let mut build = fixture_package(Path::new("/not-opened/Cargo.toml"));
        build.id = "build".into();
        let mut dev = fixture_package(Path::new("/not-opened/Cargo.toml"));
        dev.id = "dev".into();
        let metadata = Metadata {
            packages: vec![package, normal, build, dev],
            workspace_members: vec!["consumer".into()],
            resolve: Resolve {
                root: Some("consumer".into()),
                nodes: vec![
                    Node {
                        id: "consumer".into(),
                        features: Vec::new(),
                        deps: vec![
                            NodeDep {
                                name: "normal".into(),
                                pkg: "normal".into(),
                                dep_kinds: vec![DepKind {
                                    kind: None,
                                    target: None,
                                }],
                            },
                            NodeDep {
                                name: "build".into(),
                                pkg: "build".into(),
                                dep_kinds: vec![DepKind {
                                    kind: Some("build".into()),
                                    target: None,
                                }],
                            },
                            NodeDep {
                                name: "dev".into(),
                                pkg: "dev".into(),
                                dep_kinds: vec![DepKind {
                                    kind: Some("dev".into()),
                                    target: None,
                                }],
                            },
                        ],
                    },
                    Node {
                        id: "normal".into(),
                        deps: Vec::new(),
                        features: Vec::new(),
                    },
                    Node {
                        id: "build".into(),
                        deps: Vec::new(),
                        features: Vec::new(),
                    },
                    Node {
                        id: "dev".into(),
                        deps: Vec::new(),
                        features: Vec::new(),
                    },
                ],
            },
        };
        let roles = selected_closure(&metadata).unwrap();
        assert!(roles["normal"].contains("target"));
        assert!(roles["build"].contains("host"));
        assert!(!roles.contains_key("dev"));
    }

    #[test]
    fn inherited_dependency_features_and_default_flag_are_preserved() {
        let workspace: Value = "[workspace.dependencies]\napi={version='1',default-features=false,features=['shared']}\n".parse().unwrap();
        let manifest: Value =
            "[dependencies]\napi={workspace=true,features=['local'],optional=true}\n"
                .parse()
                .unwrap();
        let mut dependency = manifest["dependencies"]["api"].clone();
        flatten_dependency(&mut dependency, "api", Some(&workspace)).unwrap();
        assert_eq!(dependency["version"].as_str(), Some("1"));
        assert_eq!(dependency["default-features"].as_bool(), Some(false));
        assert_eq!(dependency["optional"].as_bool(), Some(true));
        assert_eq!(
            dependency["features"].as_array().unwrap(),
            &[
                Value::String("shared".into()),
                Value::String("local".into())
            ]
        );
        assert!(dependency.get("workspace").is_none());
    }

    #[test]
    fn atomic_commit_does_not_replace_an_existing_empty_directory() {
        let temporary = tempfile::tempdir().unwrap();
        let source = temporary.path().join("source");
        let destination = temporary.path().join("destination");
        fs::create_dir(&source).unwrap();
        fs::write(source.join("payload"), b"source").unwrap();
        fs::create_dir(&destination).unwrap();
        assert!(commit_directory(&source, &destination).is_err());
        assert!(source.join("payload").exists());
        assert_eq!(fs::read_dir(destination).unwrap().count(), 0);
    }
}
