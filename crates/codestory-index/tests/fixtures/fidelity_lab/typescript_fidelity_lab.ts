import { readFileSync as readFile } from "fs";
import { join as pathJoin } from "path";

type Mapper<T> = (value: T) => T;

interface Notifier {
    notify(value: string): void;
}

class ConsoleNotifier implements Notifier {
    notify(value: string): void {
        this.writeLog(value);
    }

    writeLog(value: string): void {
        void value;
    }
}

class Repository<T> {
    save(item: T): void {
        this.track(item);
    }

    track(item: T): void {
        void item;
    }
}

class Workflow<T extends { name: string }> {
    identity(value: T, mapper: Mapper<T>): T {
        return mapper(value);
    }

    run(notifier: Notifier, repository: Repository<T>, item: T): void {
        const mapped = this.identity(item, (value) => value);
        notifier.notify(mapped.name);
        repository.save(mapped);
        this.decorate(mapped);
    }

    async runAsync(notifier: Notifier, repository: Repository<T>, item: T): Promise<void> {
        this.run(notifier, repository, item);
        await Promise.resolve();
    }

    decorate(item: T): string {
        return pathJoin(item.name, readFile.length.toString());
    }
}

export function orchestrateTs(): void {
    const workflow = new Workflow<{ name: string }>();
    workflow.run(new ConsoleNotifier(), new Repository<{ name: string }>(), { name: "checkout" });
}
