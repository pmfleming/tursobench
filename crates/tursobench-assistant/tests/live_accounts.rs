//! Opt-in: probes existing sign-ins, with no prompt, login/logout or database.
use std::{sync::Arc, time::Duration};
use tursobench_assistant::{
    account::{Event, Job, Operation, SignIn, Target},
    vendor::{Provider, resolve, search_dirs},
};
#[test]
#[ignore = "requires the user's installed and signed-in CLIs; ACP may save an empty session"]
fn installed_agents_report_status_and_models() {
    let cwd = tempfile::tempdir().unwrap();
    for provider in [Provider::Claude, Provider::Codex, Provider::Antigravity] {
        let vendor = provider.vendor();
        let program =
            resolve(std::path::Path::new(vendor.command), &search_dirs()).expect("installed CLI");
        let mut args: Vec<_> = vendor.args.iter().map(|a| (*a).to_owned()).collect();
        if provider == Provider::Antigravity {
            args.push("--disable-telemetry".into());
        }
        let job = Job::start(
            Target {
                provider,
                program,
                args,
                env: vec![],
                cwd: cwd.path().into(),
            },
            Operation::Check,
            Arc::new(|| {}),
        );
        match job.events.recv_timeout(Duration::from_secs(40)).unwrap() {
            Event::Finished(Ok(status)) => {
                assert_eq!(status.state, SignIn::SignedIn, "{}", vendor.name);
                assert!(!status.models.is_empty(), "{} models", vendor.name);
                assert!(status.version.is_some(), "{} version", vendor.name);
                assert_eq!(status.issue, None, "{} status session", vendor.name);
                eprintln!("{}: signed in; {} models", vendor.name, status.models.len());
            }
            Event::Finished(Err(error)) => panic!("{}: {error}", vendor.name),
            Event::Browser(_) => panic!("a status check must never start sign-in"),
        }
    }
}
