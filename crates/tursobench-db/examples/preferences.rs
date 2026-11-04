//! Native fixture helper for the startup-discovery smoke, not legacy migration.
use std::path::Path;
use tursobench_db::preferences::Store;
fn main() -> Result<(), String> {
    let args: Vec<_> = std::env::args().collect();
    let path = Path::new(
        args.get(1)
            .ok_or("Expected a disposable preferences path")?,
    );
    if let Some(payload) = args.get(2) {
        let mut store = Store::open(path)?;
        store.save(&std::fs::read_to_string(payload).map_err(|e| e.to_string())?)?;
    } else {
        println!("{}", Store::read_file(path)?.as_deref().unwrap_or("{}"));
    }
    Ok(())
}
