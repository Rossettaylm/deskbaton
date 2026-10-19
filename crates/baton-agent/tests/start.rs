//! `Agent::start` 生成并复用本机身份，`shutdown` 等协调器结束后才返回。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use baton_agent::{Agent, Options};
use baton_core::config::Config;
use baton_core::ports::Notifier;

struct Quiet;

impl Notifier for Quiet {
    fn notify(&self, message: &str) {
        eprintln!("[notify] {message}");
    }
}

fn options(dir: &std::path::Path, stopped: &Arc<AtomicBool>) -> Options {
    let mut config = Config {
        name: Some("start-test".into()),
        ..Config::default()
    };
    config.network.port = 0;
    let stopped = Arc::clone(stopped);
    Options {
        config,
        data_dir: dir.to_owned(),
        notifier: Box::new(Quiet),
        observer: None,
        file_observer: None,
        on_stopped: Box::new(move || stopped.store(true, Ordering::SeqCst)),
    }
}

#[test]
fn starts_with_persistent_identity_and_stops_cleanly() {
    let dir = tempfile::tempdir().unwrap();
    let stopped = Arc::new(AtomicBool::new(false));

    let mut agent = Agent::start(options(dir.path(), &stopped)).unwrap();
    assert_eq!(agent.me().0, "start-test");
    let fingerprint = agent.link().snapshot().fingerprint;
    agent.shutdown();
    assert!(
        stopped.load(Ordering::SeqCst),
        "shutdown 返回前协调器已结束"
    );
    drop(agent);

    let again = Agent::start(options(dir.path(), &Arc::new(AtomicBool::new(false)))).unwrap();
    assert_eq!(again.link().snapshot().fingerprint, fingerprint);
    assert!(dir.path().join("identity").is_dir());
}
