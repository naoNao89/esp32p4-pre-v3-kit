use std::fs;
use std::path::Path;

fn main() {
    let d = tempfile::tempdir().unwrap();
    let src = d.path().join("src");
    let dst = d.path().join("dst");
    fs::create_dir(&src).unwrap();
    fs::create_dir(&dst).unwrap();
    fs::rename(&src, &dst).unwrap();
    println!("Success");
}
