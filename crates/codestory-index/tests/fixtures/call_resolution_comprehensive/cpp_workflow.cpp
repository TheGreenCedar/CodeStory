#include <future>
#include <string>

class Notifier {
public:
    virtual ~Notifier() = default;
    virtual void notifyEvent(const std::string& value) = 0;
};

template <typename T>
class Repository {
public:
    virtual ~Repository() = default;
    virtual void save(const T& value) = 0;
};

template <typename T>
class MemoryRepository : public Repository<T> {
public:
    void save(const T& value) override {
        trackSave(value);
    }

    void trackSave(const T& value) {}
};

class EmailNotifier : public Notifier {
public:
    void notifyEvent(const std::string& value) override {
        writeLog(value);
    }

    void writeLog(const std::string& value) {}
};

class Workflow {
public:
    virtual ~Workflow() = default;
    virtual void persist(const std::string& value) = 0;

    void run(Notifier& notifier, Repository<std::string>& repository, const std::string& value) {
        notifier.notifyEvent(value);
        repository.save(value);
        persist(value);
        audit(value);
    }

    std::future<void> runAsync(
        Notifier& notifier,
        Repository<std::string>& repository,
        const std::string& value
    ) {
        run(notifier, repository, value);
        return std::async([&]() { audit(value); });
    }

    virtual void audit(const std::string& value) {}
};

class CheckoutWorkflow : public Workflow {
public:
    void persist(const std::string& value) override {
        saveRecord(value);
    }

    void saveRecord(const std::string& value) {}
};
