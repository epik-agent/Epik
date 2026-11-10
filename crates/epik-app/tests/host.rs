//! `epik_core::remote::Ssh` with the real `ssh`, to a real remote host:
//! the SSH destination in `EPIK_TEST_HOST`, set up as the README's "Remote
//! hosts" says, such as the container `host/host.sh up` makes. Without
//! `EPIK_TEST_HOST`, each test says it is skipped and passes.

use epik_core::ipc::{Request, Response};
use epik_core::remote::Ssh;
use epik_core::settings::{Role, Settings};

const HOST_VAR: &str = "EPIK_TEST_HOST";

/// An `Ssh` to the host in `EPIK_TEST_HOST`, or `None` if it is unset.
fn host(test: &str) -> Option<Ssh> {
    match std::env::var(HOST_VAR) {
        Ok(destination) if !destination.is_empty() => Some(Ssh::new(&destination)),
        _ => {
            eprintln!("{test}: skipped, because {HOST_VAR} names no host");
            None
        }
    }
}

#[test]
fn the_host_answers_with_this_version() {
    let Some(ssh) = host("the_host_answers_with_this_version") else {
        return;
    };

    assert_eq!(
        ssh.request(&Request::Ping).unwrap(),
        Response::Pong {
            version: epik_core::version().to_owned(),
        }
    );
}

#[test]
fn the_hosts_setup_lacks_only_the_sign_ins() {
    let Some(ssh) = host("the_hosts_setup_lacks_only_the_sign_ins") else {
        return;
    };
    let claude_only = Settings {
        coding: Role {
            agent: "claude".to_owned(),
            model: None,
        },
        review: Some(Role {
            agent: "claude".to_owned(),
            model: None,
        }),
    };

    let reply = ssh
        .request(&Request::Setup {
            roles: Some(claude_only),
        })
        .unwrap();

    let Response::Setup { items } = reply else {
        panic!("{reply:?}");
    };
    let blocking: Vec<&str> = items
        .iter()
        .filter(|check| check.blocks_build())
        .map(|check| check.item.id())
        .collect();
    assert_eq!(blocking, ["gh-auth", "claude-auth"], "{items:#?}");
}

#[test]
fn the_host_has_no_runs() {
    let Some(ssh) = host("the_host_has_no_runs") else {
        return;
    };

    assert_eq!(
        ssh.request(&Request::Runs).unwrap(),
        Response::RunKeys { runs: vec![] }
    );
}
