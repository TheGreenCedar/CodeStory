package app

import kotlin.math.abs

interface Notifier {
    fun notify(event: Event)
}

class ConsoleNotifier : Notifier {
    override fun notify(event: Event) {
        println(event.name)
    }
}

class Repository {
    fun save(event: Event) {
        println(event.name)
    }
}

class Event(val name: String)

class Workflow {
    fun run(event: Event, notifier: Notifier, repository: Repository) {
        notifier.notify(event)
        repository.save(event)
        decorate(event)
    }

    fun decorate(event: Event): String {
        return event.name
    }
}

fun orchestrateKotlin() {
    val workflow = Workflow()
    workflow.run(Event("ready"), ConsoleNotifier(), Repository())
    abs(1)
}
