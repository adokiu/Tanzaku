use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

fn main() {
    check_migration_versions();
    // rust-embed 读的是 crate 根下的相对路径；显式声明依赖，避免只改前端时 cargo 跳过重嵌。
    watch_admin_dist();
}

fn check_migration_versions() {
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
            panic!("duplicate sqlx migration version {version}: {prev} and {name}");
        }
    }
}

fn watch_admin_dist() {
    let stamp = Path::new(".web-admin-embed");
    println!("cargo:rerun-if-changed={}", stamp.display());

    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let admin_dist = manifest.join("../../web-admin/dist");
    let admin_dist = match admin_dist.canonicalize() {
        Ok(path) => path,
        Err(_) => {
            println!("cargo:warning=web-admin/dist 不存在，编译后的 board 将缺少管理端静态资源");
            return;
        }
    };
    println!("cargo:rerun-if-changed={}", admin_dist.display());
    walk_rerun(&admin_dist);
}

fn walk_rerun(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        println!("cargo:rerun-if-changed={}", path.display());
        if path.is_dir() {
            walk_rerun(&path);
        }
    }
}
