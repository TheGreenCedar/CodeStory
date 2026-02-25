interface Notifier {
    notifyEvent(value: string): void;
}

interface Repository<T> {
    save(value: T): void;
}

class EmailNotifier implements Notifier {
    notifyEvent(value: string): void {
        this.writeLog(value);
    }

    writeLog(value: string): void {}
}

class MemoryRepository<T> implements Repository<T> {
    save(value: T): void {
        this.trackSave(value);
    }

    trackSave(value: T): void {}
}

class Workflow<T> {
    identity(value: T): T {
        return value;
    }

    persist(value: T): void {
        throw new Error(String(value));
    }

    run(notifier: Notifier, repository: Repository<T>, value: T): void {
        value = this.identity(value);
        notifier.notifyEvent(value);
        repository.save(value);
        this.persist(value);
        this.audit(value);
    }

    async runAsync(notifier: Notifier, repository: Repository<T>, value: T): Promise<void> {
        this.run(notifier, repository, value);
        await Promise.resolve();
    }

    audit(value: T): void {}
}

class CheckoutWorkflow extends Workflow<string> {
    persist(value: string): void {
        this.saveRecord(value);
    }

    saveRecord(value: string): void {}
}
