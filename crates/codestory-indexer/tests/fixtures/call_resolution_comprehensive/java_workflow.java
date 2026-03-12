import java.util.concurrent.CompletableFuture;

interface Notifier {
    void notifyEvent(String value);
}

interface Repository<T> {
    void save(T value);
}

class EmailNotifier implements Notifier {
    @Override
    public void notifyEvent(String value) {
        writeLog(value);
    }

    void writeLog(String value) {}
}

class MemoryRepository<T> implements Repository<T> {
    @Override
    public void save(T value) {
        trackSave(value);
    }

    void trackSave(T value) {}
}

abstract class Workflow {
    <T> T identity(T value) {
        return value;
    }

    abstract void persist(String value);

    void run(Notifier notifier, Repository<String> repository, String value) {
        value = identity(value);
        notifier.notifyEvent(value);
        repository.save(value);
        persist(value);
        audit(value);
    }

    CompletableFuture<Void> runAsync(Notifier notifier, Repository<String> repository, String value) {
        run(notifier, repository, value);
        return CompletableFuture.completedFuture(null);
    }

    void audit(String value) {}
}

class CheckoutWorkflow extends Workflow {
    @Override
    void persist(String value) {
        saveRecord(value);
    }

    void saveRecord(String value) {}
}
