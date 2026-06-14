import 'dart:math';

abstract class Notifier {
  void notify(Event event);
}

class ConsoleNotifier implements Notifier {
  void notify(Event event) {
    print(event.name);
  }
}

class Repository {
  void save(Event event) {
    print(event.name);
  }
}

class Event {
  final String name;

  Event(this.name);
}

class Workflow {
  void run(Event event, Notifier notifier, Repository repository) {
    notifier.notify(event);
    repository.save(event);
    decorate(event);
  }

  String decorate(Event event) {
    return event.name;
  }
}

void orchestrateDart() {
  final workflow = Workflow();
  workflow.run(Event('ready'), ConsoleNotifier(), Repository());
  max(1, 2);
}
