use anyhow::{Context, Result, bail};
use clap::CommandFactory;
use clap_complete::{Shell, generate};
use std::net::{TcpListener, ToSocketAddrs};

use crate::runtime::ensure_index_ready;
use crate::{
    args::{Cli, CompletionShell, GenerateCompletionsCommand, ServeCommand},
    http_transport, stdio_transport,
};

use super::lifecycle::new_agent_surface_runtime;

pub(super) async fn run_serve(cmd: ServeCommand) -> Result<()> {
    validate_serve_cache_routing(&cmd)?;
    if !cmd.stdio {
        ensure_http_serve_bind_allowed(&cmd.addr, cmd.allow_non_loopback)?;
    }
    if cmd.multi_project {
        return stdio_transport::run_stdio_server(None, cmd.refresh).await;
    }
    let runtime = new_agent_surface_runtime(&cmd.project, None, None)?;
    if cmd.stdio {
        return stdio_transport::run_stdio_server(Some(runtime), cmd.refresh).await;
    }
    let opened = runtime.ensure_open(cmd.refresh)?;
    ensure_index_ready(&opened, "serve")?;
    let listener = TcpListener::bind(&cmd.addr)
        .with_context(|| format!("Failed to bind server to {}", cmd.addr))?;
    eprintln!("codestory serve listening on http://{}", cmd.addr);
    let policy = http_transport::HttpServePolicy::new(cmd.allow_non_loopback);
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                if let Err(error) = http_transport::handle_http_request(&runtime, stream, policy) {
                    eprintln!("serve request failed: {error:#}");
                }
            }
            Err(error) => eprintln!("serve accept failed: {error}"),
        }
    }
    Ok(())
}

fn validate_serve_cache_routing(cmd: &ServeCommand) -> Result<()> {
    if cmd.multi_project && cmd.project.cache_dir.is_some() {
        bail!(
            "serve --multi-project cannot use --cache-dir; use single-project serve with \
--project and --cache-dir, or omit --cache-dir for per-project cache routing"
        );
    }
    Ok(())
}

pub(super) fn ensure_http_serve_bind_allowed(addr: &str, allow_non_loopback: bool) -> Result<()> {
    if allow_non_loopback {
        return Ok(());
    }

    let resolved = addr
        .to_socket_addrs()
        .with_context(|| format!("Failed to resolve serve address {addr}"))?
        .collect::<Vec<_>>();
    if resolved.is_empty() {
        bail!("Serve address {addr} did not resolve to a socket address");
    }
    if resolved
        .iter()
        .all(|socket_addr| socket_addr.ip().is_loopback())
    {
        return Ok(());
    }

    bail!(
        "Refusing to bind HTTP serve to non-loopback address `{addr}` without --allow-non-loopback. \
serve exposes local graph/search endpoints without request authentication; bind to 127.0.0.1/localhost \
or rerun with --allow-non-loopback only behind an intentional network boundary."
    )
}

pub(super) fn run_generate_completions(cmd: GenerateCompletionsCommand) -> Result<()> {
    let shell = match cmd.shell {
        CompletionShell::Bash => Shell::Bash,
        CompletionShell::Zsh => Shell::Zsh,
        CompletionShell::Fish => Shell::Fish,
        CompletionShell::Powershell => Shell::PowerShell,
    };
    let mut command = Cli::command();
    generate(shell, &mut command, "codestory-cli", &mut std::io::stdout());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::args::{Command, ProjectArgs};
    use clap::Parser;

    #[test]
    fn serve_cache_routing_rejects_programmatic_multi_project_override() {
        let parsed = Cli::try_parse_from(["codestory-cli", "serve", "--stdio", "--multi-project"])
            .expect("valid multi-project command");
        let Command::Serve(mut command) = parsed.command else {
            panic!("expected serve command");
        };
        validate_serve_cache_routing(&command).expect("default multi-project routing");
        command.project = ProjectArgs {
            project: "unopened-project".into(),
            cache_dir: Some("unopened-cache".into()),
        };
        let error = validate_serve_cache_routing(&command)
            .expect_err("programmatic callers must not bypass the parse-time conflict");
        assert!(
            error
                .to_string()
                .contains("--multi-project cannot use --cache-dir")
        );
        command.multi_project = false;
        validate_serve_cache_routing(&command)
            .expect("single-project exact override remains valid");
    }
}
