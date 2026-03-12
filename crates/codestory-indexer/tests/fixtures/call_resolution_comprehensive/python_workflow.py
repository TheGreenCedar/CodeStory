import abc
import asyncio


class Notifier(abc.ABC):
    @abc.abstractmethod
    def notify_event(self, value):
        raise NotImplementedError


class EmailNotifier(Notifier):
    def notify_event(self, value):
        self.write_log(value)

    def write_log(self, value):
        return value


class Workflow(abc.ABC):
    @abc.abstractmethod
    def persist(self, value):
        raise NotImplementedError

    def run(self, notifier, value):
        notifier.notify_event(value)
        self.persist(value)
        self.audit(value)

    async def run_async(self, notifier, value):
        self.run(notifier, value)
        await asyncio.sleep(0)

    def audit(self, value):
        return value


class CheckoutWorkflow(Workflow):
    def persist(self, value):
        self.save_record(value)

    def save_record(self, value):
        return value
