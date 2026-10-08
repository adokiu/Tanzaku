use std::{
    collections::HashMap,
    path::Path,
};

fn main() {
    let dir = Path::new("migrations");
    let mut seen = HashMap::new();
    for entry in std::fs::read_dir(dir).expect("read migrations directory") {
        let entry = entry.expect("migration dir entry");
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let Some((version, _)) = name.split_once('_') else {
            continue;
        };
        if let Some(prev) = seen.insert(version.to_string(), name.to_string()) {
            panic!(
                "duplicate sqlx migration version {version}: {prev} and {name}"
            );
        }
    }
}
