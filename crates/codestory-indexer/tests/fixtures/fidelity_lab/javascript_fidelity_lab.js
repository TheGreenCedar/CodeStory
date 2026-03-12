import { readFileSync as readFile } from "fs";
import { join as pathJoin } from "path";

class Notifier {
    notify(value) {
        throw new Error(value);
    }
}

class ConsoleNotifier extends Notifier {
    notify(value) {
        this.writeLog(value);
    }

    writeLog(value) {
        return value;
    }
}

class Repository {
    save(item) {
        this.track(item);
    }

    track(item) {
        return item;
    }
}

class Workflow {
    identity(value, mapper) {
        return mapper(value);
    }

    run(notifier, repository, item) {
        const mapped = this.identity(item, (value) => value);
        notifier.notify(mapped.name);
        repository.save(mapped);
        this.decorate(mapped);
    }

    async runAsync(notifier, repository, item) {
        this.run(notifier, repository, item);
        await Promise.resolve();
    }

    decorate(item) {
        return pathJoin(item.name, String(readFile.length));
    }
}

export function orchestrateJs() {
    const workflow = new Workflow();
    workflow.run(new ConsoleNotifier(), new Repository(), { name: "checkout" });
}
