use std::collections::VecDeque as Queue;
use std::future::ready;

trait Notifier {
    fn notify(&self, value: &str);
}

struct ConsoleNotifier;

impl ConsoleNotifier {
    fn write_log(&self, _value: &str) {}
}

impl Notifier for ConsoleNotifier {
    fn notify(&self, value: &str) {
        self.write_log(value);
    }
}

struct Event {
    name: String,
}

trait Repository<T> {
    fn save(&self, item: &T);
    fn track(&self, item: &T);
}

struct MemoryRepository;

impl Repository<Event> for MemoryRepository {
    fn save(&self, item: &Event) {
        self.track(item);
    }

    fn track(&self, _item: &Event) {}
}

struct Workflow;

impl Workflow {
    fn identity<T, F>(&self, value: T, mapper: F) -> T
    where
        F: FnOnce(T) -> T,
    {
        mapper(value)
    }

    fn run<N, R>(&self, notifier: &N, repository: &R, event: Event)
    where
        N: Notifier,
        R: Repository<Event>,
    {
        let mapped = self.identity(event, |value| value);
        notifier.notify(&mapped.name);
        repository.save(&mapped);
        self.decorate(&mapped);
    }

    async fn run_async<N, R>(&self, notifier: &N, repository: &R, event: Event)
    where
        N: Notifier,
        R: Repository<Event>,
    {
        self.run(notifier, repository, event);
        ready(()).await;
    }

    fn decorate(&self, event: &Event) -> usize {
        event.name.len()
    }
}

fn orchestrate_rust() {
    let mut queue = Queue::from([Event {
        name: "checkout".to_string(),
    }]);
    let workflow = Workflow;
    let notifier = ConsoleNotifier;
    let repository = MemoryRepository;
    if let Some(event) = queue.pop_front() {
        workflow.run(&notifier, &repository, event);
    }
}
