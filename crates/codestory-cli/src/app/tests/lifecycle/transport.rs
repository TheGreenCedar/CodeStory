use crate::app::embedding_client_transport_mode;
use crate::args::{Cli, Command};
use crate::{embedding_server_transport, sidecar_runtime};
use anyhow::Result;
use clap::Parser;

fn parsed_command(args: &[&str]) -> Command {
    Cli::try_parse_from(std::iter::once("codestory-cli").chain(args.iter().copied()))
        .expect("command should parse")
        .command
}

#[test]
fn ground_and_retrieval_status_install_observe_only_live_transport() {
    for args in [&["ground"][..], &["retrieval", "status"][..]] {
        assert_eq!(
            embedding_client_transport_mode(&parsed_command(args)),
            Some(embedding_server_transport::ClientTransportMode::ObserveOnly),
            "{args:?} should retain a live observe transport without spawn authority"
        );
    }
}

#[test]
fn ground_and_retrieval_status_retain_the_native_live_probe() -> Result<()> {
    for args in [&["ground"][..], &["retrieval", "status"][..]] {
        assert_eq!(
            embedding_client_transport_mode(&parsed_command(args)),
            Some(embedding_server_transport::ClientTransportMode::ObserveOnly)
        );
    }
    embedding_server_transport::install_client_transport(
        embedding_server_transport::ClientTransportMode::ObserveOnly,
    )?;
    let runtime = sidecar_runtime::local();
    let client = codestory_retrieval::PerUserEmbeddingClient::for_runtime(&runtime)?;
    if let Err(error) = client.observe() {
        let message = format!("{error:#}");
        assert!(
            !message.contains("embedding_server_transport_unavailable")
                && !message.contains("embedding_server_spawn_forbidden"),
            "an observational command must execute the native live probe: {message}"
        );
    }
    Ok(())
}

#[test]
fn embedding_client_transport_startup_keeps_embedding_capable_commands() {
    for args in [
        &["index"][..],
        &["packet", "--question", "explain the runtime"][..],
        &["search", "--query", "RuntimeContext"][..],
        &["retrieval", "index"][..],
        &["retrieval", "query", "RuntimeContext"][..],
        &["serve"][..],
        &[
            "internal-embedding-qualification-worker",
            "--request",
            "/private/worker-request.json",
            "--output",
            "/private/worker-output.json",
        ][..],
    ] {
        assert_eq!(
            embedding_client_transport_mode(&parsed_command(args)),
            Some(embedding_server_transport::ClientTransportMode::SpawnCapable),
            "{args:?} should retain exact executable identity capture"
        );
    }
}

#[test]
fn production_cli_rejects_the_removed_qualification_suite() {
    let result = Cli::try_parse_from([
        "codestory-cli",
        "internal-embedding-qualification",
        "--request",
        "/private/request.json",
        "--output",
        "/private/output.json",
    ]);
    assert!(result.is_err());
}

#[test]
fn embedding_client_transport_startup_keeps_non_status_and_server_boundaries() {
    for args in [
        &["retrieval", "inventory"][..],
        &["retrieval", "republish-projections"][..],
    ] {
        assert_eq!(
            embedding_client_transport_mode(&parsed_command(args)),
            Some(embedding_server_transport::ClientTransportMode::SpawnCapable),
            "{args:?} should not widen the observational exemption"
        );
    }
    assert_eq!(
        embedding_client_transport_mode(&parsed_command(&["internal-embedding-server"])),
        None
    );
}

pub(super) struct EnvVarSnapshot<'a> {
    values: Vec<(&'a str, Option<std::ffi::OsString>)>,
}

impl<'a> EnvVarSnapshot<'a> {
    pub(super) fn clear(names: &'a [&'a str]) -> Self {
        let values = names
            .iter()
            .map(|name| (*name, std::env::var_os(name)))
            .collect();
        for name in names {
            unsafe {
                std::env::remove_var(name);
            }
        }
        Self { values }
    }
}

impl Drop for EnvVarSnapshot<'_> {
    fn drop(&mut self) {
        for (name, value) in &self.values {
            unsafe {
                match value {
                    Some(value) => std::env::set_var(name, value),
                    None => std::env::remove_var(name),
                }
            }
        }
    }
}
