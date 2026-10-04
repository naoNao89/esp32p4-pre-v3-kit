fn main() {
    let p = std::path::Path::new("C:\\foo\\bar");
    let json_p = serde_json::to_string(p.to_str().unwrap()).unwrap();
    println!("json_p = {}", json_p);
}
