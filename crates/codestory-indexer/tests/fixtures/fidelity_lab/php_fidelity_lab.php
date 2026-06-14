<?php

namespace App;

use Random\Randomizer;

interface Notifier
{
    public function notify(Event $event): void;
}

final class ConsoleNotifier implements Notifier
{
    public function notify(Event $event): void
    {
        echo $event->name;
    }
}

final class Repository
{
    public function save(Event $event): void
    {
        echo $event->name;
    }
}

final class Event
{
    public function __construct(public string $name)
    {
    }
}

final class Workflow
{
    public function __construct(
        private Notifier $notifier,
        private Repository $repository
    ) {
    }

    public function run(Event $event): void
    {
        $this->notifier->notify($event);
        $this->repository->save($event);
        $this->decorate($event);
    }

    private function decorate(Event $event): string
    {
        return $event->name;
    }
}

function orchestrate_php(): void
{
    $workflow = new Workflow(new ConsoleNotifier(), new Repository());
    $workflow->run(new Event((new Randomizer())->getBytes(4)));
}
