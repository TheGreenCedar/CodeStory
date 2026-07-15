use crate::config::{
    SIDECAR_STATE_FILE_V3, SidecarLayout, SidecarPorts, fnv1a_hex, sidecar_ports_from_value,
};
use anyhow::{Context, Result};
use rusqlite::{Connection, Transaction, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub(crate) const AGENT_PORT_LEASE_TTL: Duration = Duration::from_secs(10 * 60);
const SQLITE_REGISTRY_SCHEMA_VERSION: i64 = 3;
const SQLITE_REGISTRY_FILE: &str = "port-allocations.sqlite3";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct AgentPortOwner {
    id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct AgentPortLease {
    namespace: String,
    owner: AgentPortOwner,
    acquired_at_epoch_ms: i64,
    renewed_at_epoch_ms: i64,
    expires_at_epoch_ms: i64,
    embed_http: u16,
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
            existing.as_ref().map(|lease| lease.embed_http),
            namespace,
            &mut reserved,
            state_ports
                .as_ref()
                .is_some_and(|ports| Some(ports.embed_http) == configured),
        )?;
        let ports = SidecarPorts {
            embed_http,
            embed_url: SidecarLayout::embed_base_url(embed_http),
        };
        let owner = select_owner(root, namespace, existing.as_ref(), embed_http, now)?;
        let continued = existing
            .as_ref()
            .filter(|lease| lease.owner.id == owner.id && lease.embed_http == embed_http);
        let renewed_at_epoch_ms = existing.as_ref().map_or(Ok(now), |lease| {
            next_lease_renewal_epoch_ms(now, lease.renewed_at_epoch_ms)
        })?;
        let lease = AgentPortLease {
            namespace: namespace.to_string(),
            owner: owner.clone(),
            acquired_at_epoch_ms: continued.map_or(now, |lease| lease.acquired_at_epoch_ms),
            renewed_at_epoch_ms,
            expires_at_epoch_ms: lease_expiry(renewed_at_epoch_ms),
            embed_http,
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

        let mut selected_port = lease.embed_http;
        if force_rotation || !local_port_available(selected_port) {
            let mut reserved = reserved_ports_excluding(&registry, namespace);
            if force_rotation {
                reserved.insert(selected_port);
            }
            selected_port = reserve_dynamic_port(namespace, &mut reserved);
            if selected_port == 0 {
                anyhow::bail!("agent sidecar embedding port rotation is unavailable");
            }
        }

        let now = next_lease_renewal_epoch_ms(now_epoch_ms(), lease.renewed_at_epoch_ms)?;
        lease.embed_http = selected_port;
        lease.renewed_at_epoch_ms = now;
        lease.expires_at_epoch_ms = lease_expiry(now);
        put_lease(transaction, &lease)?;
        Ok(SidecarPorts {
            embed_http: selected_port,
            embed_url: SidecarLayout::embed_base_url(selected_port),
        })
    })
}

fn with_registry_transaction<T>(
    base: &Path,
    operation: impl FnOnce(&Path, &Transaction<'_>) -> Result<T>,
) -> Result<T> {
    let root = base.join("sidecars");
    std::fs::create_dir_all(&root)
        .with_context(|| format!("create sidecar port registry dir {}", root.display()))?;
    let database_path = root.join(SQLITE_REGISTRY_FILE);
    let mut connection = Connection::open(&database_path)
        .with_context(|| format!("open sidecar port registry {}", database_path.display()))?;
    connection.busy_timeout(Duration::from_secs(5))?;
    connection.pragma_update(None, "foreign_keys", true)?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .context("begin immediate sidecar port registry transaction")?;
    initialize_schema(&transaction)?;
    let output = operation(&root, &transaction)?;
    let registry = load_registry(&transaction)?;
    validate_registry(&registry)?;
    transaction
        .commit()
        .context("commit sidecar port registry transaction")?;
    Ok(output)
}

fn initialize_schema(transaction: &Transaction<'_>) -> Result<()> {
    let version: i64 = transaction.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if !matches!(version, 0 | 1 | 2 | SQLITE_REGISTRY_SCHEMA_VERSION) {
        anyhow::bail!("unsupported sidecar port registry SQLite schema {version}");
    }
    if matches!(version, 1 | 2) {
        transaction.execute_batch(
            "ALTER TABLE lease_ports RENAME TO lease_ports_v1;
             ALTER TABLE port_leases RENAME TO port_leases_v1;
             CREATE TABLE port_leases (
                 namespace TEXT PRIMARY KEY NOT NULL,
                 owner_id TEXT NOT NULL,
                 acquired_at_epoch_ms INTEGER NOT NULL,
                 renewed_at_epoch_ms INTEGER NOT NULL,
                 expires_at_epoch_ms INTEGER NOT NULL,
                 embed_http INTEGER NOT NULL UNIQUE CHECK (embed_http BETWEEN 1 AND 65535)
             );
             INSERT INTO port_leases (
                 namespace, owner_id, acquired_at_epoch_ms, renewed_at_epoch_ms,
                 expires_at_epoch_ms, embed_http
             ) SELECT l.namespace, l.owner_id, l.acquired_at_epoch_ms, l.renewed_at_epoch_ms,
                      l.expires_at_epoch_ms, p.port
                 FROM port_leases_v1 l
                 JOIN lease_ports_v1 p ON p.namespace = l.namespace
                 WHERE p.role = 'embed_http';
             DROP TABLE lease_ports_v1;
             DROP TABLE port_leases_v1;",
        )?;
    } else {
        transaction.execute_batch(
            "CREATE TABLE IF NOT EXISTS port_leases (
             namespace TEXT PRIMARY KEY NOT NULL,
             owner_id TEXT NOT NULL,
             acquired_at_epoch_ms INTEGER NOT NULL,
             renewed_at_epoch_ms INTEGER NOT NULL,
             expires_at_epoch_ms INTEGER NOT NULL,
             embed_http INTEGER NOT NULL UNIQUE CHECK (embed_http BETWEEN 1 AND 65535)
         );",
        )?;
    }
    if version != SQLITE_REGISTRY_SCHEMA_VERSION {
        transaction.pragma_update(None, "user_version", SQLITE_REGISTRY_SCHEMA_VERSION)?;
    }
    Ok(())
}

fn load_registry(transaction: &Transaction<'_>) -> Result<BTreeMap<String, AgentPortLease>> {
    let mut statement = transaction.prepare(
        "SELECT l.namespace, l.owner_id, l.acquired_at_epoch_ms,
                l.renewed_at_epoch_ms, l.expires_at_epoch_ms, l.embed_http
         FROM port_leases l
         ORDER BY l.namespace",
    )?;
    let mut rows = statement.query([])?;
    let mut leases = BTreeMap::new();
    while let Some(row) = rows.next()? {
        let namespace: String = row.get(0)?;
        let owner_id: String = row.get(1)?;
        let acquired_at_epoch_ms: i64 = row.get(2)?;
        let renewed_at_epoch_ms: i64 = row.get(3)?;
        let expires_at_epoch_ms: i64 = row.get(4)?;
        let port: i64 = row.get(5)?;
        let port = u16::try_from(port).context("sidecar port registry port is outside u16")?;
        let lease = leases
            .entry(namespace.clone())
            .or_insert_with(|| AgentPortLease {
                namespace,
                owner: AgentPortOwner { id: owner_id },
                acquired_at_epoch_ms,
                renewed_at_epoch_ms,
                expires_at_epoch_ms,
                embed_http: port,
            });
        if lease.embed_http != port {
            anyhow::bail!("sidecar port registry contains a duplicate port role");
        }
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
    transaction
        .execute(
            "INSERT INTO port_leases (
             namespace, owner_id, acquired_at_epoch_ms, renewed_at_epoch_ms,
             expires_at_epoch_ms, embed_http
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                lease.namespace,
                lease.owner.id,
                lease.acquired_at_epoch_ms,
                lease.renewed_at_epoch_ms,
                lease.expires_at_epoch_ms,
                i64::from(lease.embed_http),
            ],
        )
        .with_context(|| {
            format!(
                "reserve agent embedding port {} for namespace {}",
                lease.embed_http, lease.namespace
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

fn select_owner(
    root: &Path,
    namespace: &str,
    existing: Option<&AgentPortLease>,
    embed_http: u16,
    now: i64,
) -> Result<AgentPortOwner> {
    match existing {
        Some(lease) if lease.embed_http != embed_http => {
            if lease_is_live(root, lease, now)? {
                anyhow::bail!("agent sidecar namespace {namespace} already has a live port lease");
            }
            Ok(new_agent_port_owner())
        }
        Some(lease) => match read_agent_port_owner(root, namespace)? {
            Some(owner) if owner.id == lease.owner.id => Ok(owner),
            Some(_) | None if sidecar_state_owns_port(root, namespace, lease.embed_http)? => {
                Ok(new_agent_port_owner())
            }
            Some(_) | None if sidecar_port_is_bound(lease.embed_http) => anyhow::bail!(
                "agent sidecar namespace {namespace} has a bound port without matching lease ownership"
            ),
            Some(_) | None => Ok(new_agent_port_owner()),
        },
        None => Ok(new_agent_port_owner()),
    }
}

fn ensure_owner_matches(
    namespace: &str,
    lease: &AgentPortLease,
    owner: &AgentPortOwner,
    owner_id: &str,
    ports: &SidecarPorts,
) -> Result<()> {
    if lease.owner.id != owner_id || owner.id != owner_id || lease.embed_http != ports.embed_http {
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
        if lease.acquired_at_epoch_ms < 0
            || lease.renewed_at_epoch_ms < lease.acquired_at_epoch_ms
            || lease.expires_at_epoch_ms <= lease.renewed_at_epoch_ms
        {
            anyhow::bail!("sidecar port lease timestamps are invalid");
        }
        if lease.embed_http == 0 {
            anyhow::bail!("sidecar port registry contains an invalid embedding port");
        }
        if !ports.insert(lease.embed_http) {
            anyhow::bail!("sidecar port registry contains a duplicate port");
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
    Ok(sidecar_port_is_bound(lease.embed_http))
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

fn sidecar_state_owns_port(root: &Path, namespace: &str, embed_http: u16) -> Result<bool> {
    Ok(owned_sidecar_state_ports(root, namespace)?
        .as_ref()
        .is_some_and(|state_ports| state_ports.embed_http == embed_http))
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

fn agent_port_owner_path(root: &Path, namespace: &str) -> PathBuf {
    root.join(namespace).join("port-owner.json")
}

fn new_agent_port_owner() -> AgentPortOwner {
    AgentPortOwner {
        id: uuid::Uuid::new_v4().to_string(),
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

fn sidecar_port_is_bound(port: u16) -> bool {
    !local_port_available(port)
}

fn reserved_ports_excluding(
    registry: &BTreeMap<String, AgentPortLease>,
    namespace: &str,
) -> BTreeSet<u16> {
    registry
        .iter()
        .filter(|(candidate, _)| candidate.as_str() != namespace)
        .map(|(_, lease)| lease.embed_http)
        .filter(|port| *port != 0)
        .collect()
}

fn select_port(
    configured: Option<u16>,
    existing: Option<u16>,
    namespace: &str,
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
    Ok(reserve_dynamic_port(namespace, reserved))
}

fn reserve_dynamic_port(namespace: &str, reserved: &mut BTreeSet<u16>) -> u16 {
    let port = dynamic_port_excluding(namespace, reserved);
    reserved.insert(port);
    port
}

fn dynamic_port_excluding(namespace: &str, reserved: &BTreeSet<u16>) -> u16 {
    let seed = fnv1a_hex(format!("{namespace}:embed").as_bytes());
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
        let stored: i64 = connection
            .query_row(
                "SELECT embed_http FROM port_leases WHERE namespace = 'embedded'",
                [],
                |row| row.get(0),
            )
            .expect("stored embedding port");
        assert_eq!(stored, i64::from(allocation.ports.embed_http));
    }

    #[test]
    fn schema_upgrade_drops_removed_port_roles() {
        let cache = tempdir().expect("cache");
        let root = cache.path().join("sidecars");
        std::fs::create_dir_all(&root).expect("registry root");
        let database = root.join(SQLITE_REGISTRY_FILE);
        let connection = Connection::open(&database).expect("registry");
        connection
            .execute_batch(
                "CREATE TABLE port_leases (
                     namespace TEXT PRIMARY KEY NOT NULL,
                     owner_id TEXT NOT NULL,
                     owner_process_id INTEGER NOT NULL,
                     owner_created_at_epoch_ms INTEGER NOT NULL,
                     acquired_at_epoch_ms INTEGER NOT NULL,
                     renewed_at_epoch_ms INTEGER NOT NULL,
                     expires_at_epoch_ms INTEGER NOT NULL
                 );
                 CREATE TABLE lease_ports (
                     namespace TEXT NOT NULL REFERENCES port_leases(namespace) ON DELETE CASCADE,
                     role TEXT NOT NULL CHECK (role IN ('embed_http', 'removed_http')),
                     port INTEGER NOT NULL CHECK (port BETWEEN 1 AND 65535),
                     PRIMARY KEY (namespace, role),
                     UNIQUE (port)
                 );
                 INSERT INTO port_leases VALUES ('old', 'owner', 1, 1, 1, 1, 2);
                 INSERT INTO lease_ports VALUES ('old', 'embed_http', 38080);
                 INSERT INTO lease_ports VALUES ('old', 'removed_http', 38081);
                 PRAGMA user_version = 1;",
            )
            .expect("version 1 registry");
        drop(connection);

        with_registry_transaction(cache.path(), |_root, _transaction| Ok(()))
            .expect("upgrade registry");

        let connection = Connection::open(database).expect("upgraded registry");
        let version: i64 = connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .expect("schema version");
        let stored: i64 = connection
            .query_row(
                "SELECT embed_http FROM port_leases WHERE namespace = 'old'",
                [],
                |row| row.get(0),
            )
            .expect("migrated embedding port");
        let legacy_table_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'lease_ports'",
                [],
                |row| row.get(0),
            )
            .expect("legacy table count");
        assert_eq!(version, SQLITE_REGISTRY_SCHEMA_VERSION);
        assert_eq!(stored, 38080);
        assert_eq!(legacy_table_count, 0);
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
}
