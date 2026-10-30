//! Generate new, closed disposable fixtures for desktop/manual compatibility tests.
#[path = "../tests/common/local_fixtures.rs"]
mod fixtures;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    let root = std::path::PathBuf::from(
        args.next()
            .ok_or("usage: local_project_fixtures NEW_DIRECTORY")?,
    );
    if args.next().is_some() {
        return Err("expected one new output directory".into());
    }
    // Refuse an existing directory: never mix synthetic fixtures with user data.
    std::fs::create_dir(&root)?;
    for name in fixtures::NAMES {
        let path = root.join(format!("{name}.db"));
        fixtures::create(&path, name).await?;
        println!(
            "{} (closed, synthetic, {count} primary rows)",
            path.display(),
            count = fixtures::ROWS
        );
    }
    Ok(())
}
