use crate::config::{
    SIDECAR_STATE_FILE_V3, SidecarLayout, SidecarPorts, fnv1a_hex, sidecar_ports_from_value,
};
use anyhow::{Context, Result};
use fs4::fs_std::FileExt;
use rusqlite::{Connection, Transaction, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::OpenOptions;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub(crate) const AGENT_PORT_LEASE_TTL: Duration = Duration::from_secs(10 * 60);
const LEGACY_REGISTRY_SCHEMA_VERSION: u32 = 2;
const SQLITE_REGISTRY_SCHEMA_VERSION: i64 = 1;
const SQLITE_REGISTRY_FILE: &str = "port-allocations.sqlite3";
const LEGACY_REGISTRY_FILE: &str = "port-allocations.json";
const LEGACY_LOCK_FILE: &str = "port-allocations.lock";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct AgentPortOwner {
    id: String,
    process_id: u32,
    created_at_epoch_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct AgentPortLease {
    namespace: String,
    owner: AgentPortOwner,
    acquired_at_epoch_ms: i64,
    renewed_at_epoch_ms: i64,
    expires_at_epoch_ms: i64,
    ports: SidecarPorts,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct LegacyPortRegistry {
    schema_version: u32,
    leases: BTreeMap<String, AgentPortLease>,
}

impl Default for LegacyPortRegistry {
    fn default() -> Self {
        Self {
            schema_version: LEGACY_REGISTRY_SCHEMA_VERSION,
            leases: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct AgentPortAllocation {
    pub(crate) ports: SidecarPorts,
    pub(crate) owner_id: String,
}

impl AgentPortAllocation {
    pub(crate) fn failed(&self) -> bool {
        self.owner_id.is_empty() || self.ports.embed_http == 0
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
struct RegistryCleanup {
    pruned: usize,
    retained: usize,
    failures: usize,
    failure_details: Vec<String>,
}

impl RegistryCleanup {
    fn record_failure(&mut self, detail: String) {
        if self.failure_details.len() < 5 {
            self.failure_details.push(detail);
        }
    }
}

pub(crate) fn allocate_agent_embedding_port(
    base: &Path,
    namespace: &str,
    configured: Option<u16>,
) -> Result<AgentPortAllocation> {
    allocate_agent_port_at(base, namespace, configured, now_epoch_ms())
}

fn allocate_agent_port_at(
    base: &Path,
    namespace: &str,
    configured: Option<u16>,
    now: i64,
) -> Result<AgentPortAllocation> {
    if !is_agent_namespace_path_component(namespace) {
        anyhow::bail!("agent namespace is not a safe path component");
    }
    let (allocation, cleanup) = with_registry_transaction(base, |root, transaction| {
        let cleanup = prune_registry(root, transaction, namespace, now)?;
        let registry = load_registry(transaction)?;
        let existing = registry.get(namespace).cloned();
        let state_ports = if existing.is_none() {
            owned_sidecar_state_ports(root, namespace)?
        } else {
            None
        };
        let mut reserved = reserved_ports_excluding(&registry, namespace);
        let embed_http = select_port(
            configured,
            existing.as_ref().map(|lease| lease.ports.embed_http),
            namespace,
            "embed",
            &mut reserved,
            state_ports
                .as_ref()
                .is_some_and(|ports| Some(ports.embed_http) == configured),
        )?;
        let ports = SidecarPorts {
            embed_http,
            embed_url: SidecarLayout::embed_base_url(embed_http),
        };
        let owner = select_owner(root, namespace, existing.as_ref(), &ports, now)?;
        let continued = existing
            .as_ref()
            .filter(|lease| lease.owner.id == owner.id && lease.ports == ports);
        let renewed_at_epoch_ms = existing.as_ref().map_or(Ok(now), |lease| {
            next_lease_renewal_epoch_ms(now, lease.renewed_at_epoch_ms)
        })?;
        let lease = AgentPortLease {
            namespace: namespace.to_string(),
            owner: owner.clone(),
            acquired_at_epoch_ms: continued.map_or(now, |lease| lease.acquired_at_epoch_ms),
            renewed_at_epoch_ms,
            expires_at_epoch_ms: lease_expiry(renewed_at_epoch_ms),
            ports: ports.clone(),
        };
        put_lease(transaction, &lease)?;
        write_agent_port_owner(root, namespace, &owner)?;
        Ok((
            AgentPortAllocation {
                ports,
                owner_id: owner.id,
            },
            cleanup,
        ))
    })?;
    report_cleanup(&cleanup);
    Ok(allocation)
}

pub(crate) fn renew_agent_port_lease(
    base: &Path,
    namespace: &str,
    owner_id: &str,
    ports: &SidecarPorts,
) -> Result<()> {
    with_registry_transaction(base, |root, transaction| {
        let mut lease = load_lease(transaction, namespace)?
            .with_context(|| format!("agent sidecar namespace {namespace} has no port lease"))?;
        let owner = read_agent_port_owner(root, namespace)?
            .with_context(|| format!("agent sidecar namespace {namespace} has no port owner"))?;
        ensure_owner_matches(namespace, &lease, &owner, owner_id, ports)?;
        let now = next_lease_renewal_epoch_ms(now_epoch_ms(), lease.renewed_at_epoch_ms)?;
        lease.renewed_at_epoch_ms = now;
        lease.expires_at_epoch_ms = lease_expiry(now);
        put_lease(transaction, &lease)
    })
}

pub(crate) fn revalidate_agent_embedding_port(
    base: &Path,
    namespace: &str,
    owner_id: &str,
    ports: &SidecarPorts,
    force_rotation: bool,
) -> Result<SidecarPorts> {
    with_registry_transaction(base, |root, transaction| {
        let registry = load_registry(transaction)?;
        let mut lease = registry
            .get(namespace)
            .cloned()
            .with_context(|| format!("agent sidecar namespace {namespace} has no port lease"))?;
        let owner = read_agent_port_owner(root, namespace)?
            .with_context(|| format!("agent sidecar namespace {namespace} has no port owner"))?;
        ensure_owner_matches(namespace, &lease, &owner, owner_id, ports)?;

        let mut selected = lease.ports.clone();
        if force_rotation || !local_port_available(selected.embed_http) {
            let mut reserved = reserved_ports_excluding(&registry, namespace);
            if force_rotation {
                reserved.insert(selected.embed_http);
            }
            selected.embed_http = reserve_dynamic_port(namespace, "embed", &mut reserved);
            if selected.embed_http == 0 {
                anyhow::bail!("agent sidecar embedding port rotation is unavailable");
            }
            selected.embed_url = SidecarLayout::embed_base_url(selected.embed_http);
        }

        let now = next_lease_renewal_epoch_ms(now_epoch_ms(), lease.renewed_at_epoch_ms)?;
        lease.ports = selected.clone();
        lease.renewed_at_epoch_ms = now;
        lease.expires_at_epoch_ms = lease_expiry(now);
        put_lease(transaction, &lease)?;
        Ok(selected)
    })
}

fn with_registry_transaction<T>(
    base: &Path,
    operation: impl FnOnce(&Path, &Transaction<'_>) -> Result<T>,
) -> Result<T> {
    let root = base.join("sidecars");
    std::fs::create_dir_all(&root)
        .with_context(|| format!("create sidecar port registry dir {}", root.display()))?;
    let lock_path = root.join(LEGACY_LOCK_FILE);
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
        .with_context(|| format!("open sidecar port allocation lock {}", lock_path.display()))?;
    FileExt::lock_exclusive(&lock)
        .with_context(|| format!("take sidecar port allocation lock {}", lock_path.display()))?;

    let database_path = root.join(SQLITE_REGISTRY_FILE);
    let mut connection = Connection::open(&database_path)
        .with_context(|| format!("open sidecar port registry {}", database_path.display()))?;
    connection.busy_timeout(Duration::from_secs(5))?;
    connection.pragma_update(None, "foreign_keys", true)?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .context("begin immediate sidecar port registry transaction")?;
    initialize_schema(&transaction)?;
    let legacy_namespaces = import_legacy_bridge(&root, &transaction)?;
    let output = operation(&root, &transaction)?;
    let registry = load_registry(&transaction)?;
    validate_registry(&registry)?;
    project_legacy_bridge(&root, &registry, &legacy_namespaces)?;
    transaction
        .commit()
        .context("commit sidecar port registry transaction")?;
    Ok(output)
}

fn initialize_schema(transaction: &Transaction<'_>) -> Result<()> {
    let version: i64 = transaction.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if !matches!(version, 0 | SQLITE_REGISTRY_SCHEMA_VERSION) {
        anyhow::bail!("unsupported sidecar port registry SQLite schema {version}");
    }
    transaction.execute_batch(
        "CREATE TABLE IF NOT EXISTS port_leases (
             namespace TEXT PRIMARY KEY NOT NULL,
             owner_id TEXT NOT NULL,
             owner_process_id INTEGER NOT NULL,
             owner_created_at_epoch_ms INTEGER NOT NULL,
             acquired_at_epoch_ms INTEGER NOT NULL,
             renewed_at_epoch_ms INTEGER NOT NULL,
             expires_at_epoch_ms INTEGER NOT NULL
         );
         CREATE TABLE IF NOT EXISTS lease_ports (
             namespace TEXT NOT NULL REFERENCES port_leases(namespace) ON DELETE CASCADE,
             role TEXT NOT NULL CHECK (role IN ('qdrant_http', 'qdrant_grpc', 'embed_http')),
             port INTEGER NOT NULL CHECK (port BETWEEN 1 AND 65535),
             PRIMARY KEY (namespace, role),
             UNIQUE (port)
         );",
    )?;
    if version == 0 {
        transaction.pragma_update(None, "user_version", SQLITE_REGISTRY_SCHEMA_VERSION)?;
    }
    Ok(())
}

fn load_registry(transaction: &Transaction<'_>) -> Result<BTreeMap<String, AgentPortLease>> {
    let lease_count: i64 =
        transaction.query_row("SELECT COUNT(*) FROM port_leases", [], |row| row.get(0))?;
    let mut statement = transaction.prepare(
        "SELECT l.namespace, l.owner_id, l.owner_process_id,
                l.owner_created_at_epoch_ms, l.acquired_at_epoch_ms,
                l.renewed_at_epoch_ms, l.expires_at_epoch_ms, p.role, p.port
         FROM port_leases l
         JOIN lease_ports p ON p.namespace = l.namespace
         ORDER BY l.namespace, p.role",
    )?;
    let mut rows = statement.query([])?;
    let mut leases = BTreeMap::new();
    while let Some(row) = rows.next()? {
        let namespace: String = row.get(0)?;
        let owner_id: String = row.get(1)?;
        let process_id: i64 = row.get(2)?;
        let owner_created_at_epoch_ms: i64 = row.get(3)?;
        let acquired_at_epoch_ms: i64 = row.get(4)?;
        let renewed_at_epoch_ms: i64 = row.get(5)?;
        let expires_at_epoch_ms: i64 = row.get(6)?;
        let role: String = row.get(7)?;
        let port: i64 = row.get(8)?;
        let process_id =
            u32::try_from(process_id).context("sidecar port registry process id is outside u32")?;
        let port = u16::try_from(port).context("sidecar port registry port is outside u16")?;
        let lease = leases
            .entry(namespace.clone())
            .or_insert_with(|| AgentPortLease {
                namespace,
                owner: AgentPortOwner {
                    id: owner_id,
                    process_id,
                    created_at_epoch_ms: owner_created_at_epoch_ms,
                },
                acquired_at_epoch_ms,
                renewed_at_epoch_ms,
                expires_at_epoch_ms,
                ports: SidecarPorts {
                    embed_http: 0,
                    embed_url: String::new(),
                },
            });
        match role.as_str() {
            "embed_http" if lease.ports.embed_http == 0 => lease.ports.embed_http = port,
            "qdrant_http" | "qdrant_grpc" => continue,
            "embed_http" => {
                anyhow::bail!("sidecar port registry contains a duplicate port role")
            }
            _ => anyhow::bail!("sidecar port registry contains an unknown port role"),
        }
    }
    for lease in leases.values_mut() {
        lease.ports.embed_url = SidecarLayout::embed_base_url(lease.ports.embed_http);
    }
    if usize::try_from(lease_count).ok() != Some(leases.len()) {
        anyhow::bail!("sidecar port registry contains a lease without port rows");
    }
    validate_registry(&leases)?;
    Ok(leases)
}

fn load_lease(transaction: &Transaction<'_>, namespace: &str) -> Result<Option<AgentPortLease>> {
    Ok(load_registry(transaction)?.remove(namespace))
}

fn put_lease(transaction: &Transaction<'_>, lease: &AgentPortLease) -> Result<()> {
    validate_registry(&BTreeMap::from([(lease.namespace.clone(), lease.clone())]))?;
    transaction.execute(
        "DELETE FROM port_leases WHERE namespace = ?1",
        params![lease.namespace],
    )?;
    transaction.execute(
        "INSERT INTO port_leases (
             namespace, owner_id, owner_process_id, owner_created_at_epoch_ms,
             acquired_at_epoch_ms, renewed_at_epoch_ms, expires_at_epoch_ms
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            lease.namespace,
            lease.owner.id,
            i64::from(lease.owner.process_id),
            lease.owner.created_at_epoch_ms,
            lease.acquired_at_epoch_ms,
            lease.renewed_at_epoch_ms,
            lease.expires_at_epoch_ms,
        ],
    )?;
    transaction
        .execute(
            "INSERT INTO lease_ports (namespace, role, port) VALUES (?1, 'embed_http', ?2)",
            params![lease.namespace, i64::from(lease.ports.embed_http)],
        )
        .with_context(|| {
            format!(
                "reserve agent embedding port {} for namespace {}",
                lease.ports.embed_http, lease.namespace
            )
        })?;
    Ok(())
}

fn prune_registry(
    root: &Path,
    transaction: &Transaction<'_>,
    current_namespace: &str,
    now: i64,
) -> Result<RegistryCleanup> {
    let registry = load_registry(transaction)?;
    let mut cleanup = RegistryCleanup::default();
    for (namespace, lease) in registry {
        if namespace == current_namespace {
            continue;
        }
        match lease_is_live(root, &lease, now) {
            Ok(true) => cleanup.retained += 1,
            Ok(false) => {
                transaction.execute(
                    "DELETE FROM port_leases WHERE namespace = ?1",
                    params![namespace],
                )?;
                cleanup.pruned += 1;
            }
            Err(error) => {
                cleanup.failures += 1;
                cleanup.record_failure(format!(
                    "namespace={namespace} preserved unverified allocation: {error:#}"
                ));
            }
        }
    }
    Ok(cleanup)
}

fn import_legacy_bridge(root: &Path, transaction: &Transaction<'_>) -> Result<BTreeSet<String>> {
    let Some(legacy) = read_legacy_bridge(root)? else {
        return Ok(BTreeSet::new());
    };
    let observed = legacy.leases.keys().cloned().collect();
    let current = load_registry(transaction)?;
    for (namespace, candidate) in legacy.leases {
        match current.get(&namespace) {
            None => put_lease(transaction, &candidate).with_context(|| {
                format!("import legacy sidecar port lease for namespace {namespace}")
            })?,
            Some(existing) if existing == &candidate => {}
            Some(existing)
                if existing.owner.id == candidate.owner.id
                    && same_port_numbers(&existing.ports, &candidate.ports) =>
            {
                if candidate.renewed_at_epoch_ms > existing.renewed_at_epoch_ms {
                    put_lease(transaction, &candidate)?;
                }
            }
            Some(existing) => {
                if lease_is_live(root, existing, now_epoch_ms())?
                    || lease_is_live(root, &candidate, now_epoch_ms())?
                {
                    anyhow::bail!(
                        "live legacy sidecar port lease ambiguity for namespace {namespace}"
                    );
                }
                if candidate.renewed_at_epoch_ms > existing.renewed_at_epoch_ms {
                    put_lease(transaction, &candidate)?;
                }
            }
        }
    }
    validate_registry(&load_registry(transaction)?)?;
    Ok(observed)
}

fn read_legacy_bridge(root: &Path) -> Result<Option<LegacyPortRegistry>> {
    let registry_path = root.join(LEGACY_REGISTRY_FILE);
    let mut registry = match std::fs::read_to_string(&registry_path) {
        Ok(body) => parse_legacy_registry(&body, &registry_path)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => LegacyPortRegistry::default(),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("read legacy port registry {}", registry_path.display()));
        }
    };
    let had_compact = registry_path.exists();
    let lease_root = root.join("port-leases");
    let mut had_recovery = false;
    match std::fs::read_dir(&lease_root) {
        Ok(entries) => {
            for entry in entries {
                let entry =
                    entry.with_context(|| format!("read entry in {}", lease_root.display()))?;
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if !name.ends_with(".json") {
                    continue;
                }
                had_recovery = true;
                let path = entry.path();
                let bytes = std::fs::read(&path)
                    .with_context(|| format!("read legacy port lease {}", path.display()))?;
                let recovered: AgentPortLease = serde_json::from_slice(&bytes)
                    .with_context(|| format!("parse legacy port lease {}", path.display()))?;
                if agent_port_lease_path(root, &recovered.namespace) != path {
                    anyhow::bail!("legacy port lease filename does not match namespace");
                }
                match registry.leases.get(&recovered.namespace) {
                    None => {
                        registry
                            .leases
                            .insert(recovered.namespace.clone(), recovered);
                    }
                    Some(current) if current == &recovered => {}
                    Some(current)
                        if recovered.renewed_at_epoch_ms > current.renewed_at_epoch_ms =>
                    {
                        registry
                            .leases
                            .insert(recovered.namespace.clone(), recovered);
                    }
                    Some(_) => anyhow::bail!(
                        "legacy compact registry conflicts with recovery lease for namespace {}",
                        recovered.namespace
                    ),
                }
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(error).with_context(|| format!("read {}", lease_root.display()));
        }
    }
    if !had_compact && !had_recovery {
        return Ok(None);
    }
    validate_registry(&registry.leases)?;
    Ok(Some(registry))
}

fn parse_legacy_registry(body: &str, path: &Path) -> Result<LegacyPortRegistry> {
    if let Ok(registry) = serde_json::from_str::<LegacyPortRegistry>(body) {
        if registry.schema_version != LEGACY_REGISTRY_SCHEMA_VERSION {
            anyhow::bail!(
                "unsupported legacy sidecar port registry schema {}",
                registry.schema_version
            );
        }
        validate_registry(&registry.leases)?;
        return Ok(registry);
    }
    if let Ok(legacy) = serde_json::from_str::<BTreeMap<String, SidecarPorts>>(body) {
        let now = now_epoch_ms();
        let leases = legacy
            .into_iter()
            .map(|(namespace, ports)| {
                let owner = AgentPortOwner {
                    id: format!("legacy-{namespace}"),
                    process_id: 0,
                    created_at_epoch_ms: now,
                };
                (
                    namespace.clone(),
                    AgentPortLease {
                        namespace,
                        owner,
                        acquired_at_epoch_ms: 0,
                        renewed_at_epoch_ms: 0,
                        expires_at_epoch_ms: 0,
                        ports,
                    },
                )
            })
            .collect();
        let registry = LegacyPortRegistry {
            schema_version: LEGACY_REGISTRY_SCHEMA_VERSION,
            leases,
        };
        validate_registry(&registry.leases)?;
        return Ok(registry);
    }
    anyhow::bail!(
        "malformed legacy sidecar port allocation registry {}",
        path.display()
    )
}

fn project_legacy_bridge(
    root: &Path,
    registry: &BTreeMap<String, AgentPortLease>,
    observed_legacy_namespaces: &BTreeSet<String>,
) -> Result<()> {
    let lease_root = root.join("port-leases");
    std::fs::create_dir_all(&lease_root)
        .with_context(|| format!("create legacy port lease dir {}", lease_root.display()))?;
    for lease in registry.values() {
        write_agent_port_lease(root, lease)?;
    }
    let compact = LegacyPortRegistry {
        schema_version: LEGACY_REGISTRY_SCHEMA_VERSION,
        leases: registry.clone(),
    };
    write_legacy_registry(&root.join(LEGACY_REGISTRY_FILE), &compact)?;
    for namespace in observed_legacy_namespaces {
        if registry.contains_key(namespace) {
            continue;
        }
        remove_file_if_present(&agent_port_lease_path(root, namespace))?;
        remove_file_if_present(&agent_port_owner_path(root, namespace))?;
        remove_empty_agent_namespace_dir(root, namespace)?;
    }
    Ok(())
}

fn select_owner(
    root: &Path,
    namespace: &str,
    existing: Option<&AgentPortLease>,
    ports: &SidecarPorts,
    now: i64,
) -> Result<AgentPortOwner> {
    match existing {
        Some(lease) if lease.ports != *ports => {
            if lease_is_live(root, lease, now)? {
                anyhow::bail!("agent sidecar namespace {namespace} already has a live port lease");
            }
            Ok(new_agent_port_owner(now))
        }
        Some(lease) => match read_agent_port_owner(root, namespace)? {
            Some(owner) if owner.id == lease.owner.id => Ok(owner),
            Some(_) | None if sidecar_state_owns_ports(root, namespace, &lease.ports)? => {
                Ok(new_agent_port_owner(now))
            }
            Some(_) | None if sidecar_ports_are_bound(&lease.ports) => anyhow::bail!(
                "agent sidecar namespace {namespace} has bound ports without matching lease ownership"
            ),
            Some(_) | None => Ok(new_agent_port_owner(now)),
        },
        None => Ok(new_agent_port_owner(now)),
    }
}

fn ensure_owner_matches(
    namespace: &str,
    lease: &AgentPortLease,
    owner: &AgentPortOwner,
    owner_id: &str,
    ports: &SidecarPorts,
) -> Result<()> {
    if lease.owner.id != owner_id || owner.id != owner_id || !same_port_numbers(&lease.ports, ports)
    {
        anyhow::bail!("agent sidecar namespace {namespace} port lease ownership changed");
    }
    Ok(())
}

fn validate_registry(registry: &BTreeMap<String, AgentPortLease>) -> Result<()> {
    let mut ports = BTreeSet::new();
    for (namespace, lease) in registry {
        if namespace != &lease.namespace || !is_agent_namespace_path_component(namespace) {
            anyhow::bail!("sidecar port lease namespace is invalid");
        }
        if lease.owner.id.is_empty() {
            anyhow::bail!("sidecar port lease owner identity is empty");
        }
        let legacy_zero_timestamps = lease.owner.process_id == 0
            && lease.acquired_at_epoch_ms == 0
            && lease.renewed_at_epoch_ms == 0
            && lease.expires_at_epoch_ms == 0;
        if !legacy_zero_timestamps
            && (lease.acquired_at_epoch_ms < 0
                || lease.renewed_at_epoch_ms < lease.acquired_at_epoch_ms
                || lease.expires_at_epoch_ms <= lease.renewed_at_epoch_ms)
        {
            anyhow::bail!("sidecar port lease timestamps are invalid");
        }
        if lease.ports.embed_http == 0 {
            anyhow::bail!("sidecar port registry contains an incomplete port set");
        }
        for port in [lease.ports.embed_http] {
            if !ports.insert(port) {
                anyhow::bail!("sidecar port registry contains invalid or duplicate ports");
            }
        }
    }
    Ok(())
}

fn lease_is_live(root: &Path, lease: &AgentPortLease, now: i64) -> Result<bool> {
    if !is_agent_namespace_path_component(&lease.namespace) {
        anyhow::bail!("registry namespace is not a safe agent path component");
    }
    let owner = read_agent_port_owner(root, &lease.namespace)?;
    if owner
        .as_ref()
        .is_some_and(|owner| owner.id == lease.owner.id)
        && lease.expires_at_epoch_ms > now
    {
        return Ok(true);
    }
    Ok(sidecar_ports_are_bound(&lease.ports))
}

fn owned_sidecar_state_ports(root: &Path, namespace: &str) -> Result<Option<SidecarPorts>> {
    let path = root.join(namespace).join(SIDECAR_STATE_FILE_V3);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).with_context(|| format!("read {}", path.display())),
    };
    let value: serde_json::Value =
        serde_json::from_slice(&bytes).with_context(|| format!("parse {}", path.display()))?;
    if value.get("owner").and_then(serde_json::Value::as_str) != Some("codestory")
        || value.get("namespace").and_then(serde_json::Value::as_str) != Some(namespace)
    {
        return Ok(None);
    }
    sidecar_ports_from_value(&value)
        .context("sidecar state has incomplete ports")
        .map(Some)
}

fn sidecar_state_owns_ports(root: &Path, namespace: &str, ports: &SidecarPorts) -> Result<bool> {
    Ok(owned_sidecar_state_ports(root, namespace)?
        .as_ref()
        .is_some_and(|state_ports| same_port_numbers(state_ports, ports)))
}

fn read_agent_port_owner(root: &Path, namespace: &str) -> Result<Option<AgentPortOwner>> {
    let path = agent_port_owner_path(root, namespace);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).with_context(|| format!("read {}", path.display())),
    };
    serde_json::from_slice(&bytes)
        .with_context(|| format!("parse {}", path.display()))
        .map(Some)
}

fn write_agent_port_owner(root: &Path, namespace: &str, owner: &AgentPortOwner) -> Result<()> {
    std::fs::create_dir_all(root.join(namespace))
        .with_context(|| format!("create agent sidecar namespace {namespace}"))?;
    let bytes = serde_json::to_vec_pretty(owner)?;
    codestory_workspace::atomic_file::write_bytes_atomic(
        &agent_port_owner_path(root, namespace),
        "agent-port-owner",
        &bytes,
    )
    .with_context(|| format!("write agent port owner for {namespace}"))
}

fn write_agent_port_lease(root: &Path, lease: &AgentPortLease) -> Result<()> {
    let path = agent_port_lease_path(root, &lease.namespace);
    std::fs::create_dir_all(path.parent().context("agent port lease has no parent")?)
        .with_context(|| format!("create sidecar port lease dir for {}", lease.namespace))?;
    let bytes = serde_json::to_vec_pretty(lease)?;
    codestory_workspace::atomic_file::write_bytes_atomic(&path, "agent-port-lease", &bytes)
        .with_context(|| format!("write sidecar port lease {}", path.display()))
}

fn write_legacy_registry(path: &Path, registry: &LegacyPortRegistry) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(registry)?;
    codestory_workspace::atomic_file::write_bytes_atomic(path, "agent-port-registry", &bytes)
        .with_context(|| format!("write legacy port allocation registry {}", path.display()))
}

fn agent_port_owner_path(root: &Path, namespace: &str) -> PathBuf {
    root.join(namespace).join("port-owner.json")
}

fn agent_port_lease_path(root: &Path, namespace: &str) -> PathBuf {
    root.join("port-leases").join(format!("{namespace}.json"))
}

fn remove_file_if_present(path: &Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("remove {}", path.display())),
    }
}

fn remove_empty_agent_namespace_dir(root: &Path, namespace: &str) -> Result<()> {
    let path = root.join(namespace);
    match std::fs::remove_dir(&path) {
        Ok(()) => Ok(()),
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::DirectoryNotEmpty
            ) =>
        {
            Ok(())
        }
        Err(error) => Err(error).with_context(|| format!("remove {}", path.display())),
    }
}

fn new_agent_port_owner(now: i64) -> AgentPortOwner {
    AgentPortOwner {
        id: uuid::Uuid::new_v4().to_string(),
        process_id: std::process::id(),
        created_at_epoch_ms: now,
    }
}

fn lease_expiry(now: i64) -> i64 {
    now.saturating_add(AGENT_PORT_LEASE_TTL.as_millis() as i64)
}

fn next_lease_renewal_epoch_ms(now: i64, previous: i64) -> Result<i64> {
    let after_previous = previous
        .checked_add(1)
        .context("agent sidecar port lease renewal timestamp overflowed")?;
    Ok(now.max(after_previous))
}

fn now_epoch_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64
}

fn is_agent_namespace_path_component(namespace: &str) -> bool {
    !namespace.is_empty()
        && namespace
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

fn sidecar_ports_are_bound(ports: &SidecarPorts) -> bool {
    !local_port_available(ports.embed_http)
}

fn same_port_numbers(left: &SidecarPorts, right: &SidecarPorts) -> bool {
    left.embed_http == right.embed_http
}

fn reserved_ports_excluding(
    registry: &BTreeMap<String, AgentPortLease>,
    namespace: &str,
) -> BTreeSet<u16> {
    registry
        .iter()
        .filter(|(candidate, _)| candidate.as_str() != namespace)
        .map(|(_, lease)| lease.ports.embed_http)
        .filter(|port| *port != 0)
        .collect()
}

fn select_port(
    configured: Option<u16>,
    existing: Option<u16>,
    namespace: &str,
    salt: &str,
    reserved: &mut BTreeSet<u16>,
    state_owns_port: bool,
) -> Result<u16> {
    if let Some(port) = configured.or(existing) {
        if port == 0 {
            anyhow::bail!("agent sidecar port 0 cannot be leased");
        }
        if !reserved.insert(port) {
            anyhow::bail!("agent sidecar port {port} is already reserved");
        }
        if existing != Some(port) && !state_owns_port && !local_port_available(port) {
            anyhow::bail!("agent sidecar port {port} is already bound without matching ownership");
        }
        return Ok(port);
    }
    Ok(reserve_dynamic_port(namespace, salt, reserved))
}

fn reserve_dynamic_port(namespace: &str, salt: &str, reserved: &mut BTreeSet<u16>) -> u16 {
    let port = dynamic_port_excluding(namespace, salt, reserved);
    reserved.insert(port);
    port
}

fn dynamic_port_excluding(namespace: &str, salt: &str, reserved: &BTreeSet<u16>) -> u16 {
    let seed = fnv1a_hex(format!("{namespace}:{salt}").as_bytes());
    let parsed = u64::from_str_radix(&seed, 16).unwrap_or(0);
    let base = 20_000 + u16::try_from(parsed % 40_000).unwrap_or(0);
    for offset in 0..1000 {
        let port = 20_000 + ((u32::from(base - 20_000) + offset) % 40_000) as u16;
        if !reserved.contains(&port) && local_port_available(port) {
            return port;
        }
    }
    free_local_port_excluding(reserved)
}

fn local_port_available(port: u16) -> bool {
    TcpListener::bind(("127.0.0.1", port)).is_ok()
}

fn free_local_port_excluding(reserved: &BTreeSet<u16>) -> u16 {
    for _ in 0..100 {
        let port = free_local_port();
        if port == 0 || !reserved.contains(&port) {
            return port;
        }
    }
    0
}

pub(crate) fn free_local_port() -> u16 {
    TcpListener::bind(("127.0.0.1", 0))
        .and_then(|listener| listener.local_addr())
        .map(|addr| addr.port())
        .unwrap_or(0)
}

fn report_cleanup(cleanup: &RegistryCleanup) {
    if cleanup.pruned == 0 && cleanup.failures == 0 {
        return;
    }
    eprintln!(
        "CodeStory sidecar port cleanup: pruned={} retained={} failures={}",
        cleanup.pruned, cleanup.retained, cleanup.failures
    );
    for detail in &cleanup.failure_details {
        eprintln!("CodeStory sidecar port cleanup warning: {detail}");
    }
    if cleanup.failures > cleanup.failure_details.len() {
        eprintln!(
            "CodeStory sidecar port cleanup warning: {} additional failures omitted",
            cleanup.failures - cleanup.failure_details.len()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn configured_port(ports: &SidecarPorts) -> Option<u16> {
        Some(ports.embed_http)
    }

    fn bound_ports() -> (Vec<TcpListener>, SidecarPorts) {
        let listeners: Vec<_> = (0..1)
            .map(|_| TcpListener::bind(("127.0.0.1", 0)).expect("reserve test port"))
            .collect();
        let ports: Vec<_> = listeners
            .iter()
            .map(|listener| listener.local_addr().expect("local address").port())
            .collect();
        (
            listeners,
            SidecarPorts {
                embed_http: ports[0],
                embed_url: SidecarLayout::embed_base_url(ports[0]),
            },
        )
    }

    fn write_owned_state(root: &Path, namespace: &str, ports: &SidecarPorts) {
        std::fs::create_dir_all(root.join(namespace)).expect("namespace");
        std::fs::write(
            root.join(namespace).join(SIDECAR_STATE_FILE_V3),
            serde_json::to_vec(&serde_json::json!({
                "owner": "codestory",
                "profile": "agent",
                "namespace": namespace,
                "embed_http_port": ports.embed_http,
                "embed_url": ports.embed_url,
            }))
            .expect("state"),
        )
        .expect("state");
    }

    #[test]
    fn concurrent_allocations_are_unique() {
        let cache = tempdir().expect("cache");
        let base = std::sync::Arc::new(cache.path().to_path_buf());
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));
        let handles: Vec<_> = (0..8)
            .map(|index| {
                let base = base.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    allocate_agent_port_at(&base, &format!("worker-{index}"), None, 100)
                        .expect("allocation")
                        .ports
                })
            })
            .collect();
        let allocations: Vec<_> = handles
            .into_iter()
            .map(|handle| handle.join().expect("worker"))
            .collect();
        let unique: BTreeSet<_> = allocations.iter().map(|ports| ports.embed_http).collect();
        assert_eq!(unique.len(), 8);

        let connection = Connection::open(cache.path().join("sidecars").join(SQLITE_REGISTRY_FILE))
            .expect("registry");
        let count: i64 = connection
            .query_row("SELECT COUNT(*) FROM port_leases", [], |row| row.get(0))
            .expect("lease count");
        assert_eq!(count, 8);
    }

    #[test]
    fn registry_leases_only_the_embedding_port() {
        let cache = tempdir().expect("cache");
        let allocation = allocate_agent_embedding_port(cache.path(), "embedded", None)
            .expect("embedding allocation");
        assert_ne!(allocation.ports.embed_http, 0);

        let connection = Connection::open(cache.path().join("sidecars").join(SQLITE_REGISTRY_FILE))
            .expect("registry");
        let roles: Vec<String> = connection
            .prepare("SELECT role FROM lease_ports WHERE namespace = 'embedded' ORDER BY role")
            .expect("role query")
            .query_map([], |row| row.get(0))
            .expect("roles")
            .collect::<rusqlite::Result<_>>()
            .expect("collect roles");
        assert_eq!(roles, ["embed_http"]);
    }

    #[test]
    fn crashed_owner_is_reclaimed() {
        let cache = tempdir().expect("cache");
        let root = cache.path().join("sidecars");
        let first = allocate_agent_port_at(cache.path(), "crashed", None, 100).expect("first");
        let live = allocate_agent_port_at(cache.path(), "live", None, 100).expect("live");
        std::fs::remove_file(agent_port_owner_path(&root, "crashed")).expect("remove owner");
        renew_agent_port_lease(cache.path(), "live", &live.owner_id, &live.ports)
            .expect("renew unrelated lease");
        let replacement = allocate_agent_port_at(
            cache.path(),
            "replacement",
            configured_port(&first.ports),
            101,
        )
        .expect("replacement");
        assert_eq!(replacement.ports, first.ports);
    }

    #[test]
    fn bound_ports_fail_closed() {
        let cache = tempdir().expect("cache");
        let root = cache.path().join("sidecars");
        let (listeners, ports) = bound_ports();
        write_owned_state(&root, "live", &ports);
        allocate_agent_port_at(cache.path(), "live", configured_port(&ports), 100)
            .expect("owned allocation");
        let error = allocate_agent_port_at(
            cache.path(),
            "other",
            configured_port(&ports),
            lease_expiry(100) + 1,
        )
        .expect_err("bound ports remain reserved");
        assert_eq!(listeners.len(), 1);
        assert!(format!("{error:#}").contains("already reserved"));

        let unowned = tempdir().expect("unowned cache");
        let error = allocate_agent_port_at(unowned.path(), "unowned", configured_port(&ports), 100)
            .expect_err("bound ports require matching state");
        assert!(format!("{error:#}").contains("bound without matching ownership"));
    }

    #[test]
    fn stale_owner_token_cannot_renew_successor() {
        let cache = tempdir().expect("cache");
        let root = cache.path().join("sidecars");
        let first = allocate_agent_port_at(cache.path(), "same", None, 100).expect("first");
        std::fs::remove_file(agent_port_owner_path(&root, "same")).expect("remove owner");
        let successor = allocate_agent_port_at(cache.path(), "same", None, 101).expect("successor");
        assert_ne!(first.owner_id, successor.owner_id);
        let error = renew_agent_port_lease(cache.path(), "same", &first.owner_id, &first.ports)
            .expect_err("stale token");
        assert!(error.to_string().contains("ownership changed"));
    }

    #[test]
    fn legacy_migration_and_corruption_fail_closed() {
        let cache = tempdir().expect("cache");
        let root = cache.path().join("sidecars");
        std::fs::create_dir_all(&root).expect("root");
        let (listeners, ports) = bound_ports();
        drop(listeners);
        std::fs::write(
            root.join(LEGACY_REGISTRY_FILE),
            serde_json::to_vec(&BTreeMap::from([("legacy".to_string(), ports.clone())]))
                .expect("legacy registry"),
        )
        .expect("legacy registry");
        let migrated = allocate_agent_port_at(cache.path(), "legacy", configured_port(&ports), 100)
            .expect("migration");
        assert_eq!(migrated.ports, ports);
        assert!(root.join(SQLITE_REGISTRY_FILE).is_file());
        let projected: LegacyPortRegistry = serde_json::from_slice(
            &std::fs::read(root.join(LEGACY_REGISTRY_FILE)).expect("projection"),
        )
        .expect("projected registry");
        assert_eq!(projected.schema_version, LEGACY_REGISTRY_SCHEMA_VERSION);
        assert!(projected.leases.contains_key("legacy"));

        let corrupt = tempdir().expect("corrupt cache");
        let corrupt_root = corrupt.path().join("sidecars");
        std::fs::create_dir_all(&corrupt_root).expect("corrupt root");
        let corrupt_path = corrupt_root.join(LEGACY_REGISTRY_FILE);
        std::fs::write(&corrupt_path, b"{").expect("corrupt registry");
        let error = allocate_agent_port_at(corrupt.path(), "current", None, 100)
            .expect_err("corruption must fail closed");
        assert!(error.to_string().contains("malformed legacy"));
        assert_eq!(std::fs::read(&corrupt_path).expect("preserved"), b"{");
    }
}
