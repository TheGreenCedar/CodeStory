use super::super::{
    AwakeMonotonicClock, EmbeddingClientBudgets, EmbeddingClientTransport, EmbeddingCompatibility,
    EmbeddingConnectIntent, EmbeddingConnectOutcome, EmbeddingExecutableIdentity,
    EmbeddingSpawnAttempt, EmbeddingTransportFailure,
};
use super::identities::{test_executable, test_transport_identity};
use super::transport_fixtures::{ScriptOutcome, ScriptStream, StallingHelloStream, TestClock};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

pub(super) struct ClientTestTransport {
    pub(super) clock: Arc<TestClock>,
    pub(super) connect_count: AtomicUsize,
    pub(super) spawn_count: AtomicUsize,
    pub(super) loss_count: usize,
    pub(super) capacity: bool,
    pub(super) compatibility: EmbeddingCompatibility,
}

impl ClientTestTransport {
    pub(super) fn new(loss_count: usize, capacity: bool) -> Arc<Self> {
        Arc::new(Self {
            clock: TestClock::new(),
            connect_count: AtomicUsize::new(0),
            spawn_count: AtomicUsize::new(0),
            loss_count,
            capacity,
            compatibility: EmbeddingCompatibility::current(true),
        })
    }
}

impl EmbeddingClientTransport for ClientTestTransport {
    fn connect(
        &self,
        _intent: EmbeddingConnectIntent,
        _budget: Duration,
        _spawn_attempt: Option<&EmbeddingSpawnAttempt>,
    ) -> std::result::Result<EmbeddingConnectOutcome, EmbeddingTransportFailure> {
        let attempt = self.connect_count.fetch_add(1, Ordering::AcqRel) + 1;
        let outcome = if self.capacity {
            ScriptOutcome::Capacity
        } else if attempt <= self.loss_count {
            ScriptOutcome::Loss
        } else {
            ScriptOutcome::Success
        };
        Ok(EmbeddingConnectOutcome::Connected(Box::new(
            ScriptStream::new(outcome, self.compatibility.clone()),
        )))
    }

    fn spawn_exact_current_exe(
        &self,
    ) -> std::result::Result<EmbeddingSpawnAttempt, EmbeddingTransportFailure> {
        let generation = self.spawn_count.fetch_add(1, Ordering::AcqRel) as u64 + 1;
        Ok(EmbeddingSpawnAttempt::new(generation))
    }

    fn clock(&self) -> Arc<dyn AwakeMonotonicClock> {
        self.clock.clone()
    }

    fn executable_identity(&self) -> EmbeddingExecutableIdentity {
        test_executable()
    }

    fn budgets(&self) -> EmbeddingClientBudgets {
        EmbeddingClientBudgets::current()
    }
}

pub(super) struct ControlledCancelTestTransport {
    pub(super) clock: Arc<TestClock>,
    pub(super) connect_count: AtomicUsize,
    pub(super) request_started: Arc<AtomicBool>,
    pub(super) server_cancelled: Arc<AtomicBool>,
    pub(super) compatibility: EmbeddingCompatibility,
}

impl ControlledCancelTestTransport {
    pub(super) fn new() -> Arc<Self> {
        Arc::new(Self {
            clock: TestClock::new(),
            connect_count: AtomicUsize::new(0),
            request_started: Arc::new(AtomicBool::new(false)),
            server_cancelled: Arc::new(AtomicBool::new(false)),
            compatibility: EmbeddingCompatibility::current(true),
        })
    }
}

impl EmbeddingClientTransport for ControlledCancelTestTransport {
    fn connect(
        &self,
        _intent: EmbeddingConnectIntent,
        _budget: Duration,
        _spawn_attempt: Option<&EmbeddingSpawnAttempt>,
    ) -> std::result::Result<EmbeddingConnectOutcome, EmbeddingTransportFailure> {
        self.connect_count.fetch_add(1, Ordering::AcqRel);
        Ok(EmbeddingConnectOutcome::Connected(Box::new(
            ScriptStream::new(
                ScriptOutcome::Blocking {
                    request_started: Arc::clone(&self.request_started),
                    cancelled: Arc::clone(&self.server_cancelled),
                },
                self.compatibility.clone(),
            ),
        )))
    }

    fn spawn_exact_current_exe(
        &self,
    ) -> std::result::Result<EmbeddingSpawnAttempt, EmbeddingTransportFailure> {
        Ok(EmbeddingSpawnAttempt::new(1))
    }

    fn clock(&self) -> Arc<dyn AwakeMonotonicClock> {
        self.clock.clone()
    }

    fn executable_identity(&self) -> EmbeddingExecutableIdentity {
        test_executable()
    }

    fn budgets(&self) -> EmbeddingClientBudgets {
        EmbeddingClientBudgets {
            connect: Duration::from_millis(100),
            spawn: Duration::from_millis(100),
            retry_after: Duration::from_millis(1),
            query_request: Duration::from_secs(1),
            bulk_request: Duration::from_secs(1),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(super) enum BootstrapConnectOutcome {
    Connected,
    Loss,
    HelloLoss,
    NoOwner,
    OwnerUnresponsive,
}

pub(super) struct BootstrapTestTransport {
    pub(super) clock: Arc<TestClock>,
    pub(super) connect_count: AtomicUsize,
    pub(super) spawn_count: AtomicUsize,
    pub(super) outcomes: Mutex<std::collections::VecDeque<BootstrapConnectOutcome>>,
    pub(super) fallback: BootstrapConnectOutcome,
    pub(super) budgets: EmbeddingClientBudgets,
    pub(super) compatibility: EmbeddingCompatibility,
}

pub(super) struct DeadlineBudgetTransport {
    pub(super) clock: Arc<TestClock>,
    pub(super) connect_count: AtomicUsize,
    pub(super) spawn_count: AtomicUsize,
    pub(super) compatibility: EmbeddingCompatibility,
}

pub(super) struct ExplicitDeadlineTransport {
    pub(super) clock: Arc<TestClock>,
    pub(super) connect_count: AtomicUsize,
    pub(super) observed_connect_budget: Mutex<Option<Duration>>,
    pub(super) observed_read_timeout: Arc<Mutex<Option<Duration>>>,
}

impl ExplicitDeadlineTransport {
    pub(super) fn new() -> Arc<Self> {
        Arc::new(Self {
            clock: TestClock::new(),
            connect_count: AtomicUsize::new(0),
            observed_connect_budget: Mutex::new(None),
            observed_read_timeout: Arc::new(Mutex::new(None)),
        })
    }
}

impl EmbeddingClientTransport for ExplicitDeadlineTransport {
    fn connect(
        &self,
        _intent: EmbeddingConnectIntent,
        budget: Duration,
        _spawn_attempt: Option<&EmbeddingSpawnAttempt>,
    ) -> std::result::Result<EmbeddingConnectOutcome, EmbeddingTransportFailure> {
        self.connect_count.fetch_add(1, Ordering::AcqRel);
        *self
            .observed_connect_budget
            .lock()
            .expect("observed connect budget") = Some(budget);
        Ok(EmbeddingConnectOutcome::Connected(Box::new(
            StallingHelloStream {
                identity: test_transport_identity(),
                read_timeout: Mutex::new(None),
                observed_read_timeout: Arc::clone(&self.observed_read_timeout),
            },
        )))
    }

    fn spawn_exact_current_exe(
        &self,
    ) -> std::result::Result<EmbeddingSpawnAttempt, EmbeddingTransportFailure> {
        panic!("an explicit deadline must expire before spawning")
    }

    fn clock(&self) -> Arc<dyn AwakeMonotonicClock> {
        self.clock.clone()
    }

    fn executable_identity(&self) -> EmbeddingExecutableIdentity {
        test_executable()
    }

    fn budgets(&self) -> EmbeddingClientBudgets {
        EmbeddingClientBudgets {
            connect: Duration::from_millis(500),
            spawn: Duration::from_millis(500),
            retry_after: Duration::from_millis(10),
            query_request: Duration::from_millis(500),
            bulk_request: Duration::from_millis(500),
        }
    }
}

impl DeadlineBudgetTransport {
    pub(super) fn new() -> Arc<Self> {
        Arc::new(Self {
            clock: TestClock::new(),
            connect_count: AtomicUsize::new(0),
            spawn_count: AtomicUsize::new(0),
            compatibility: EmbeddingCompatibility::current(true),
        })
    }
}

impl EmbeddingClientTransport for DeadlineBudgetTransport {
    fn connect(
        &self,
        _intent: EmbeddingConnectIntent,
        _budget: Duration,
        _spawn_attempt: Option<&EmbeddingSpawnAttempt>,
    ) -> std::result::Result<EmbeddingConnectOutcome, EmbeddingTransportFailure> {
        let attempt = self.connect_count.fetch_add(1, Ordering::AcqRel) + 1;
        Ok(match attempt {
            1 => EmbeddingConnectOutcome::Connected(Box::new(ScriptStream::new(
                ScriptOutcome::TimedBulk {
                    hello_delay: Duration::from_millis(200),
                    exchange_delay: Duration::from_millis(100),
                    lose_exchange: true,
                },
                self.compatibility.clone(),
            ))),
            2 => EmbeddingConnectOutcome::NoOwner,
            3 => {
                thread::sleep(Duration::from_millis(75));
                EmbeddingConnectOutcome::OwnerUnresponsive(EmbeddingTransportFailure {
                    code: "embedding_server_owner_unresponsive".into(),
                    message: "the fail-stopped owner is releasing authority".into(),
                })
            }
            _ => EmbeddingConnectOutcome::Connected(Box::new(ScriptStream::new(
                ScriptOutcome::TimedBulk {
                    hello_delay: Duration::ZERO,
                    exchange_delay: Duration::from_millis(100),
                    lose_exchange: false,
                },
                self.compatibility.clone(),
            ))),
        })
    }

    fn spawn_exact_current_exe(
        &self,
    ) -> std::result::Result<EmbeddingSpawnAttempt, EmbeddingTransportFailure> {
        let generation = self.spawn_count.fetch_add(1, Ordering::AcqRel) as u64 + 1;
        Ok(EmbeddingSpawnAttempt::new(generation))
    }

    fn clock(&self) -> Arc<dyn AwakeMonotonicClock> {
        self.clock.clone()
    }

    fn executable_identity(&self) -> EmbeddingExecutableIdentity {
        test_executable()
    }

    fn budgets(&self) -> EmbeddingClientBudgets {
        EmbeddingClientBudgets {
            connect: Duration::from_millis(10),
            spawn: Duration::from_millis(100),
            retry_after: Duration::from_millis(1),
            query_request: Duration::from_millis(400),
            bulk_request: Duration::from_millis(400),
        }
    }
}

impl BootstrapTestTransport {
    pub(super) fn new(
        outcomes: impl IntoIterator<Item = BootstrapConnectOutcome>,
        fallback: BootstrapConnectOutcome,
        spawn: Duration,
    ) -> Arc<Self> {
        Arc::new(Self {
            clock: TestClock::new(),
            connect_count: AtomicUsize::new(0),
            spawn_count: AtomicUsize::new(0),
            outcomes: Mutex::new(outcomes.into_iter().collect()),
            fallback,
            budgets: EmbeddingClientBudgets {
                connect: Duration::from_millis(1),
                spawn,
                retry_after: Duration::from_millis(1),
                query_request: Duration::from_secs(1),
                bulk_request: Duration::from_secs(1),
            },
            compatibility: EmbeddingCompatibility::current(true),
        })
    }
}

impl EmbeddingClientTransport for BootstrapTestTransport {
    fn connect(
        &self,
        _intent: EmbeddingConnectIntent,
        _budget: Duration,
        _spawn_attempt: Option<&EmbeddingSpawnAttempt>,
    ) -> std::result::Result<EmbeddingConnectOutcome, EmbeddingTransportFailure> {
        self.connect_count.fetch_add(1, Ordering::AcqRel);
        let outcome = self
            .outcomes
            .lock()
            .expect("bootstrap outcome script")
            .pop_front()
            .unwrap_or(self.fallback);
        Ok(match outcome {
            BootstrapConnectOutcome::Connected => EmbeddingConnectOutcome::Connected(Box::new(
                ScriptStream::new(ScriptOutcome::Success, self.compatibility.clone()),
            )),
            BootstrapConnectOutcome::Loss => EmbeddingConnectOutcome::Connected(Box::new(
                ScriptStream::new(ScriptOutcome::Loss, self.compatibility.clone()),
            )),
            BootstrapConnectOutcome::HelloLoss => EmbeddingConnectOutcome::Connected(Box::new(
                ScriptStream::new(ScriptOutcome::HelloLoss, self.compatibility.clone()),
            )),
            BootstrapConnectOutcome::NoOwner => EmbeddingConnectOutcome::NoOwner,
            BootstrapConnectOutcome::OwnerUnresponsive => {
                EmbeddingConnectOutcome::OwnerUnresponsive(EmbeddingTransportFailure {
                    code: "embedding_server_owner_unresponsive".into(),
                    message: "the lifetime authority exists without a live endpoint".into(),
                })
            }
        })
    }

    fn spawn_exact_current_exe(
        &self,
    ) -> std::result::Result<EmbeddingSpawnAttempt, EmbeddingTransportFailure> {
        let generation = self.spawn_count.fetch_add(1, Ordering::AcqRel) as u64 + 1;
        Ok(EmbeddingSpawnAttempt::new(generation))
    }

    fn clock(&self) -> Arc<dyn AwakeMonotonicClock> {
        self.clock.clone()
    }

    fn executable_identity(&self) -> EmbeddingExecutableIdentity {
        test_executable()
    }

    fn budgets(&self) -> EmbeddingClientBudgets {
        self.budgets
    }
}
