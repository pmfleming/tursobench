use std::path::Path;

pub fn run<T>(future: impl std::future::Future<Output = T>) -> T {
    tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap()
        .block_on(future)
}

pub fn create(path: &Path, statements: &[&str]) {
    run(async {
        let db = turso::Builder::new_local(path.to_str().unwrap())
            .experimental_index_method(true)
            .build()
            .await
            .unwrap();
        let connection = db.connect().unwrap();
        for sql in statements {
            connection.execute(*sql, ()).await.unwrap();
        }
    });
}

pub fn snapshot(root: &Path) -> std::collections::BTreeMap<std::path::PathBuf, Vec<u8>> {
    fn walk(
        root: &Path,
        path: &Path,
        files: &mut std::collections::BTreeMap<std::path::PathBuf, Vec<u8>>,
    ) {
        for entry in std::fs::read_dir(path).unwrap() {
            let path = entry.unwrap().path();
            // Native/legacy admission coordination is volatile, not database
            // content. Read-only sessions may create/reserve this sidecar.
            if path
                .file_name()
                .is_some_and(|n| n.to_string_lossy().ends_with("-tshm"))
            {
                continue;
            }
            if path.is_dir() {
                walk(root, &path, files);
            } else {
                files.insert(
                    path.strip_prefix(root).unwrap().into(),
                    std::fs::read(&path).unwrap(),
                );
            }
        }
    }
    let mut files = std::collections::BTreeMap::new();
    walk(root, root, &mut files);
    files
}
