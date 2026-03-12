# ADR 0002: Workspace And Store Decoupling

Refresh planning now consumes plain inventory inputs instead of depending directly on a store object.

That keeps workspace logic testable and keeps persistence lifecycle out of the workspace layer.
