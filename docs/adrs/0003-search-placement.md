# ADR 0003: Search Placement

Search configuration is exposed through `codestory-runtime`, but `SearchEngine` remains an internal implementation detail behind runtime services.

This avoids leaking unstable orchestration concerns into adapters.
