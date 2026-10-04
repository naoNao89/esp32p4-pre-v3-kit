use std::path::Path;
fn main() {
    println!("{:?}", Path::new("/path/to/cwd").join(".").file_name());
    println!("{:?}", std::env::current_dir().unwrap().join(".").file_name());
}
