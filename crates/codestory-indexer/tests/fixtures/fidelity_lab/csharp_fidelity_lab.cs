using System;

namespace App;

interface INotifier
{
    void Notify(Event evt);
}

class ConsoleNotifier : INotifier
{
    public void Notify(Event evt)
    {
        Console.WriteLine(evt.Name);
    }
}

class Repository
{
    public void Save(Event evt)
    {
        Console.WriteLine(evt.Name);
    }
}

class Event
{
    public Event(string name)
    {
        Name = name;
    }

    public string Name { get; }
}

class Workflow
{
    private readonly INotifier notifier;
    private readonly Repository repository;

    public Workflow(INotifier notifier, Repository repository)
    {
        this.notifier = notifier;
        this.repository = repository;
    }

    public void Run(Event evt)
    {
        notifier.Notify(evt);
        repository.Save(evt);
        Decorate(evt);
    }

    private string Decorate(Event evt)
    {
        return evt.Name;
    }
}

class Program
{
    static void Main()
    {
        new Workflow(new ConsoleNotifier(), new Repository()).Run(new Event("ready"));
    }
}
