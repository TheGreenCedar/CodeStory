import Foundation

protocol Notifier {
    func notify(event: Event)
}

class ConsoleNotifier: Notifier {
    func notify(event: Event) {
        print(event.name)
    }
}

class Repository {
    func save(event: Event) {
        print(event.name)
    }
}

class Event {
    let name: String

    init(name: String) {
        self.name = name
    }
}

class Workflow {
    func run(event: Event, notifier: Notifier, repository: Repository) {
        notifier.notify(event: event)
        repository.save(event: event)
        decorate(event: event)
    }

    func decorate(event: Event) -> String {
        return event.name
    }
}

func orchestrateSwift() {
    let workflow = Workflow()
    workflow.run(event: Event(name: "ready"), notifier: ConsoleNotifier(), repository: Repository())
}
