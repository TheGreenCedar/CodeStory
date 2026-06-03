//! Nucleo full-table scan policy: suppressed on sidecar primary.

use std::cell::Cell;

thread_local! {
    static SUPPRESS_NUCLEO_SCAN: Cell<bool> = const { Cell::new(false) };
}

/// Run a closure while suppressing Nucleo full-table scan (sidecar primary retrieval path).
pub(crate) fn with_sidecar_primary_retrieval<R>(run: impl FnOnce() -> R) -> R {
    SUPPRESS_NUCLEO_SCAN.set(true);
    let result = run();
    SUPPRESS_NUCLEO_SCAN.set(false);
    result
}

/// Whether `search_symbols_with_scores` may run the Nucleo O(n) symbol table scan.
pub(crate) fn nucleo_full_scan_enabled() -> bool {
    if SUPPRESS_NUCLEO_SCAN.get() {
        return false;
    }
    if cfg!(test) {
        return true;
    }
    true
}
