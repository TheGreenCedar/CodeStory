trait Notifier {
    fn notify_event(&self, value: &str);
}

trait Repository<T> {
    fn save(&self, value: T);
}

struct EmailNotifier;
impl EmailNotifier {
    fn write_log(&self, _value: &str) {}
}

struct MemoryRepository;
impl MemoryRepository {
    fn track_save<T>(&self, _value: T) {}
}

impl Notifier for EmailNotifier {
    fn notify_event(&self, value: &str) {
        self.write_log(value);
    }
}

impl Repository<&str> for MemoryRepository {
    fn save(&self, value: &str) {
        self.track_save(value);
    }
}

trait Workflow {
    fn persist(&self, value: &str);

    fn run(&self, notifier: &dyn Notifier, repository: &dyn Repository<&str>, value: &str) {
        notifier.notify_event(value);
        repository.save(value);
        self.persist(value);
        self.audit(value);
    }

    fn audit(&self, _value: &str) {}
}

struct CheckoutWorkflow;
impl CheckoutWorkflow {
    fn save_record(&self, _value: &str) {}

    async fn run_async(
        &self,
        notifier: &dyn Notifier,
        repository: &dyn Repository<&str>,
        value: &str,
    ) {
        self.run(notifier, repository, value);
    }
}

impl Workflow for CheckoutWorkflow {
    fn persist(&self, value: &str) {
        self.save_record(value);
    }
}
