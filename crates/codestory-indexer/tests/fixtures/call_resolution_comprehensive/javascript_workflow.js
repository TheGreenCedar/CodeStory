class Notifier {
    notifyEvent(value) {
        throw new Error(value);
    }
}

class EmailNotifier extends Notifier {
    notifyEvent(value) {
        this.writeLog(value);
    }

    writeLog(value) {
        return value;
    }
}

class Workflow {
    persist(value) {
        throw new Error(value);
    }

    run(notifier, value) {
        notifier.notifyEvent(value);
        this.persist(value);
        this.audit(value);
    }

    async runAsync(notifier, value) {
        this.run(notifier, value);
        await Promise.resolve();
    }

    audit(value) {
        return value;
    }
}

class CheckoutWorkflow extends Workflow {
    persist(value) {
        this.saveRecord(value);
    }

    saveRecord(value) {
        return value;
    }
}
