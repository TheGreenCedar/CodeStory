import java.util.concurrent.CompletableFuture;
import java.util.function.Function;

interface Notifier {
    void notifyEvent(String value);
}

class ConsoleNotifier implements Notifier {
    @Override
    public void notifyEvent(String value) {
        writeLog(value);
    }

    void writeLog(String value) {
        String sink = value;
    }
}

class Repository<T> {
    void save(T item) {
        track(item);
    }

    void track(T item) {
        Object sink = item;
    }
}

class Event {
    final String name;

    Event(String name) {
        this.name = name;
    }
}

class Workflow {
    <T> T identity(T value, Function<T, T> mapper) {
        return mapper.apply(value);
    }

    void run(Notifier notifier, Repository<Event> repository, Event event) {
        Event mapped = identity(event, value -> value);
        notifier.notifyEvent(mapped.name);
        repository.save(mapped);
        decorate(mapped);
    }

    CompletableFuture<Void> runAsync(Notifier notifier, Repository<Event> repository, Event event) {
        run(notifier, repository, event);
        return CompletableFuture.completedFuture(null);
    }

    String decorate(Event event) {
        return event.name;
    }
}

class FidelityEntry {
    static void orchestrateJava() {
        Workflow workflow = new Workflow();
        workflow.run(new ConsoleNotifier(), new Repository<>(), new Event("checkout"));
    }
}
