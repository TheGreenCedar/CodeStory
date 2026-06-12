require "logger"

class Notifier
  def notify(event)
    event.name
  end
end

class ConsoleNotifier < Notifier
  def notify(event)
    puts event.name
  end
end

class Repository
  def save(event)
    event.name
  end
end

Event = Struct.new(:name)

class Workflow
  def initialize(notifier, repository)
    @notifier = notifier
    @repository = repository
  end

  def run(event)
    @notifier.notify(event)
    @repository.save(event)
    decorate(event)
  end

  def decorate(event)
    event.name
  end
end

def orchestrate_ruby
  Workflow.new(ConsoleNotifier.new, Repository.new).run(Event.new("ready"))
end
