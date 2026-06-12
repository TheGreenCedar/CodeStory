package main

import "fmt"

type Notifier interface {
	Notify(Event)
}

type ConsoleNotifier struct{}

func (ConsoleNotifier) Notify(event Event) {
	fmt.Println(event.Name)
}

type Repository struct{}

func (Repository) Save(event Event) {
	fmt.Println(event.Name)
}

type Event struct {
	Name string
}

type Workflow struct {
	notifier Notifier
	repo     Repository
}

func (w Workflow) Run(event Event) {
	w.notifier.Notify(event)
	w.repo.Save(event)
	decorate(event.Name)
}

func decorate(name string) string {
	return name
}

func orchestrateGo() {
	workflow := Workflow{notifier: ConsoleNotifier{}, repo: Repository{}}
	workflow.Run(Event{Name: "ready"})
}
