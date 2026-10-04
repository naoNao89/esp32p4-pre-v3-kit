use std::path::Path;
fn main() {
    println!("{:?}", Path::new("foo/bar/").file_name());
    println!("{:?}", Path::new(".").file_name());
    println!("{:?}", Path::new("/").file_name());
    println!("{:?}", Path::new("..").file_name());
}
