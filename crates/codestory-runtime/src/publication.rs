#![cfg(test)]

use super::{ApiError, CancellationToken, Path, Storage};

type ActivationSearchRevalidateHook = Box<dyn FnOnce(&Path)>;
type SemanticProjectionRevalidateHook = Box<dyn FnOnce(&Path)>;
type FullRefreshStagedStoreHook = Box<dyn FnOnce(&mut Storage)>;
type IncrementalStagedStoreHook = Box<dyn FnOnce(&mut Storage)>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PublicationTestBoundary {
    SemanticContextIndexes,
    SemanticNodePage,
    SemanticStoredDocumentPage,
    SemanticEndpointRead,
    ProjectionSnapshotFinalize,
    ProjectionSnapshotDetail,
    ProjectionManifestIdentity,
    Identity,
    SearchBuild,
    SearchSymbolPage,
    SearchIndexWrite,
    SearchValidation,
    SearchCompletion,
    CatalogLock,
    DatabaseReplacement,
    MarkerCompletion,
    RuntimeCache,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PublicationTestAction {
    Fail,
    Cancel,
}

thread_local! {
    pub(super) static PUBLICATION_TEST_FAULT: std::cell::RefCell<Option<(PublicationTestBoundary, PublicationTestAction)>> =
        const { std::cell::RefCell::new(None) };
    static ACTIVATION_SEARCH_BEFORE_REVALIDATE_HOOK: std::cell::RefCell<Option<ActivationSearchRevalidateHook>> =
        const { std::cell::RefCell::new(None) };
    static SEMANTIC_PROJECTION_BEFORE_REVALIDATE_HOOK: std::cell::RefCell<Option<SemanticProjectionRevalidateHook>> =
        const { std::cell::RefCell::new(None) };
    static SOURCE_POLICY_BEFORE_REVALIDATE_HOOK: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        const { std::cell::RefCell::new(None) };
    static SOURCE_POLICY_AFTER_PLAN_HOOK: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        const { std::cell::RefCell::new(None) };
    static FULL_REFRESH_STAGED_STORE_HOOK: std::cell::RefCell<Option<FullRefreshStagedStoreHook>> =
        const { std::cell::RefCell::new(None) };
    static INCREMENTAL_STAGED_STORE_HOOK: std::cell::RefCell<Option<IncrementalStagedStoreHook>> =
        const { std::cell::RefCell::new(None) };
}

pub(super) fn arm_publication_test_fault(
    boundary: PublicationTestBoundary,
    action: PublicationTestAction,
) {
    PUBLICATION_TEST_FAULT.with(|fault| *fault.borrow_mut() = Some((boundary, action)));
}

pub(super) fn arm_activation_search_before_revalidate_hook(hook: impl FnOnce(&Path) + 'static) {
    ACTIVATION_SEARCH_BEFORE_REVALIDATE_HOOK.with(|slot| {
        *slot.borrow_mut() = Some(Box::new(hook));
    });
}

pub(super) fn run_activation_search_before_revalidate_hook(storage_path: &Path) {
    ACTIVATION_SEARCH_BEFORE_REVALIDATE_HOOK.with(|slot| {
        if let Some(hook) = slot.borrow_mut().take() {
            hook(storage_path);
        }
    });
}

pub(super) fn arm_semantic_projection_before_revalidate_hook(hook: impl FnOnce(&Path) + 'static) {
    SEMANTIC_PROJECTION_BEFORE_REVALIDATE_HOOK.with(|slot| {
        *slot.borrow_mut() = Some(Box::new(hook));
    });
}

pub(super) fn run_semantic_projection_before_revalidate_hook(storage_path: &Path) {
    SEMANTIC_PROJECTION_BEFORE_REVALIDATE_HOOK.with(|slot| {
        let hook = slot.borrow_mut().take();
        if let Some(hook) = hook {
            hook(storage_path);
        }
    });
}

pub(super) fn arm_source_policy_before_revalidate_hook(hook: impl FnOnce() + 'static) {
    SOURCE_POLICY_BEFORE_REVALIDATE_HOOK.with(|slot| {
        *slot.borrow_mut() = Some(Box::new(hook));
    });
}

pub(super) fn run_source_policy_before_revalidate_hook() {
    SOURCE_POLICY_BEFORE_REVALIDATE_HOOK.with(|slot| {
        if let Some(hook) = slot.borrow_mut().take() {
            hook();
        }
    });
}

pub(super) fn arm_source_policy_after_plan_hook(hook: impl FnOnce() + 'static) {
    SOURCE_POLICY_AFTER_PLAN_HOOK.with(|slot| {
        *slot.borrow_mut() = Some(Box::new(hook));
    });
}

pub(super) fn run_source_policy_after_plan_hook() {
    SOURCE_POLICY_AFTER_PLAN_HOOK.with(|slot| {
        if let Some(hook) = slot.borrow_mut().take() {
            hook();
        }
    });
}

pub(super) fn arm_full_refresh_staged_store_hook(hook: impl FnOnce(&mut Storage) + 'static) {
    FULL_REFRESH_STAGED_STORE_HOOK.with(|slot| {
        *slot.borrow_mut() = Some(Box::new(hook));
    });
}

pub(super) fn run_full_refresh_staged_store_hook(storage: &mut Storage) {
    FULL_REFRESH_STAGED_STORE_HOOK.with(|slot| {
        if let Some(hook) = slot.borrow_mut().take() {
            hook(storage);
        }
    });
}

pub(super) fn arm_incremental_staged_store_hook(hook: impl FnOnce(&mut Storage) + 'static) {
    INCREMENTAL_STAGED_STORE_HOOK.with(|slot| {
        *slot.borrow_mut() = Some(Box::new(hook));
    });
}

pub(super) fn run_incremental_staged_store_hook(storage: &mut Storage) {
    INCREMENTAL_STAGED_STORE_HOOK.with(|slot| {
        if let Some(hook) = slot.borrow_mut().take() {
            hook(storage);
        }
    });
}

pub(super) fn publication_test_checkpoint(
    boundary: PublicationTestBoundary,
    cancel_token: Option<&CancellationToken>,
) -> Result<(), ApiError> {
    let action = PUBLICATION_TEST_FAULT.with(|fault| {
        let armed = *fault.borrow();
        matches!(armed, Some((armed_boundary, _)) if armed_boundary == boundary).then(|| {
            fault
                .borrow_mut()
                .take()
                .expect("armed publication fault")
                .1
        })
    });
    match action {
        Some(PublicationTestAction::Fail) => Err(ApiError::internal(format!(
            "Injected publication failure at {boundary:?}"
        ))),
        Some(PublicationTestAction::Cancel) => {
            if let Some(token) = cancel_token {
                token.cancel();
            }
            Ok(())
        }
        None => Ok(()),
    }
}
