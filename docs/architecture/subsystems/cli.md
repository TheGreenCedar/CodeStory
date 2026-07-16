# CLI Subsystem

`codestory-cli` is the adapter for command-line, loopback HTTP, and multi-project
stdio/MCP surfaces. It captures process defaults, validates requests, selects a
project, calls runtime or retrieval services, and renders stable DTOs.

## Ownership

- argument, tool, resource, and prompt schemas;
- process-start configuration capture and trusted config precedence;
- explicit per-request project selection and retained `RuntimeContext` values;
- bounded local activation/readiness integration;
- text, JSON, HTTP, and stdio rendering;
- public status redaction and maintainer diagnostics.

Project files cannot silently choose cache roots, credentials, or network
egress. Ambient defaults are captured once; switching A/B/A in stdio reuses each
project's immutable config and never rereads or mutates process environment.

## Entry points

- `src/args.rs` and `src/main.rs`: CLI schema and dispatch
- `src/config.rs` and `src/runtime.rs`: startup config and project contexts
- `src/stdio_catalog.rs`: MCP schema and safety metadata
- `src/stdio_transport.rs`: project routing, activation, resources, and tools

Multi-project stdio retains at most four hot contexts. A context key combines
native workspace identity with a non-secret fingerprint of the immutable cache,
retrieval, embedding, and summary configuration captured at process start.
Equivalent path spellings converge on one context; configuration changes do
not silently reuse another context. Project selection requires an absolute,
existing repository root; projectless status reports `no_project`, while
relative or unavailable roots fail closed.

Status and resources use the runtime's observational summary path. They may
read existing complete publications and operation snapshots, but they do not
create storage or start activation. Product tool calls join the runtime-owned
activation service and the runtime owns the single bounded retrieval-publication
retry for a complete public response. The same whole-response service wraps
ordinary CLI packet, search, context, drill, and graph-assisted reads, so stdio
is not a stronger consistency boundary than the CLI.
- `src/output.rs`: rendering

Generated `--help` owns option syntax. User guides own workflows. This page owns
the adapter boundary.

## Serving contract

Status and diagnostics are observational. Activating product calls may perform
their bounded local refresh and managed retrieval preparation. Request
validation happens first, and every stdio request supplies an absolute project.
Hook state never routes the runtime.

HTTP remains read-only and loopback-bound by default. `packet` remains the broad
evidence workflow; exact graph primitives and `context` do not create a second
packet/search implementation.

## Extension rules

- add command/tool schema and rendering here;
- add reusable behavior to runtime first;
- do not open store/indexer internals or set environment variables after
  process startup.

## Failure signatures

- an invalid resource activates or mutates a project;
- adapter code assembles graph/retrieval product semantics;
- project switching changes frozen defaults or the shared engine policy;
- CLI output hides stale, partial, or unavailable evidence.
