#include <future>
#include <functional>
#include <string>

class Notifier {
public:
    virtual ~Notifier() = default;
    virtual void notifyEvent(const std::string& value) = 0;
};

class ConsoleNotifier : public Notifier {
public:
    void notifyEvent(const std::string& value) override {
        writeLog(value);
    }

    void writeLog(const std::string& value) {
        std::string sink = value;
    }
};

template <typename T>
class Repository {
public:
    void save(const T& item) {
        track(item);
    }

    void track(const T& item) {
        T sink = item;
        (void)sink;
    }
};

struct Event {
    std::string name;
};

class Workflow {
public:
    template <typename T>
    T identity(const T& value, const std::function<T(const T&)>& mapper) {
        return mapper(value);
    }

    void run(Notifier& notifier, Repository<Event>& repository, const Event& event) {
        Event mapped = identity<Event>(event, [](const Event& value) { return value; });
        notifier.notifyEvent(mapped.name);
        repository.save(mapped);
        decorate(mapped);
    }

    std::future<void> runAsync(Notifier& notifier, Repository<Event>& repository, const Event& event) {
        run(notifier, repository, event);
        return std::async([this, event]() { decorate(event); });
    }

    std::string decorate(const Event& event) {
        return event.name;
    }
};

void orchestrate_cpp() {
    Workflow workflow;
    ConsoleNotifier notifier;
    Repository<Event> repository;
    workflow.run(notifier, repository, Event{"checkout"});
}
