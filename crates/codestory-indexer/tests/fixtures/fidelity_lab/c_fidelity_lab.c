#include <stddef.h>
#include <stdio.h>
#include <string.h>

#define ALIAS_LEN strlen

typedef struct Event {
    const char* name;
} Event;

typedef void (*NotifierFn)(const char* value);

typedef struct Notifier {
    NotifierFn notify;
} Notifier;

typedef struct Repository {
    void (*save)(struct Repository* self, Event event);
    int writes;
} Repository;

void repository_track(Repository* self, Event event) {
    self->writes += (int)ALIAS_LEN(event.name);
}

void repository_save(Repository* self, Event event) {
    repository_track(self, event);
}

void console_notify(const char* value) {
    printf("%s\n", value);
}

void workflow_run(Notifier* notifier, Repository* repository, Event event) {
    notifier->notify(event.name);
    repository->save(repository, event);
}

void orchestrate_c(void) {
    Notifier notifier = {console_notify};
    Repository repository = {repository_save, 0};
    workflow_run(&notifier, &repository, (Event){"checkout"});
}
