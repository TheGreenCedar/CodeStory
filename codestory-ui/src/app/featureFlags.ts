export type FeatureFlagState = {
  uxResetV2: boolean;
  onboardingStarter: boolean;
  singlePaneInvestigate: boolean;
  spacesLibrary: boolean;
};

export const FEATURE_FLAGS_STORAGE_KEY = "codestory:feature-flags:v2";

const DEFAULT_FLAGS: FeatureFlagState = {
  uxResetV2: true,
  onboardingStarter: true,
  singlePaneInvestigate: true,
  spacesLibrary: true,
};

function getStorage(): Storage | null {
  if (typeof window === "undefined") {
    return null;
  }
  return window.localStorage;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

export function normalizeFeatureFlags(raw: unknown): FeatureFlagState {
  if (!isRecord(raw)) {
    return DEFAULT_FLAGS;
  }
  const legacyModernShell =
    typeof raw.modernShell === "boolean" ? raw.modernShell : DEFAULT_FLAGS.uxResetV2;
  const legacyOnboarding =
    typeof raw.onboarding === "boolean" ? raw.onboarding : DEFAULT_FLAGS.onboardingStarter;

  return {
    uxResetV2: typeof raw.uxResetV2 === "boolean" ? raw.uxResetV2 : legacyModernShell,
    onboardingStarter:
      typeof raw.onboardingStarter === "boolean" ? raw.onboardingStarter : legacyOnboarding,
    singlePaneInvestigate:
      typeof raw.singlePaneInvestigate === "boolean"
        ? raw.singlePaneInvestigate
        : DEFAULT_FLAGS.singlePaneInvestigate,
    spacesLibrary:
      typeof raw.spacesLibrary === "boolean" ? raw.spacesLibrary : DEFAULT_FLAGS.spacesLibrary,
  };
}

export function loadFeatureFlags(): FeatureFlagState {
  const storage = getStorage();
  if (!storage) {
    return DEFAULT_FLAGS;
  }
  const raw = storage.getItem(FEATURE_FLAGS_STORAGE_KEY);
  if (!raw) {
    return DEFAULT_FLAGS;
  }
  try {
    const parsed = JSON.parse(raw) as unknown;
    return normalizeFeatureFlags(parsed);
  } catch {
    return DEFAULT_FLAGS;
  }
}

export function saveFeatureFlags(flags: FeatureFlagState): void {
  const storage = getStorage();
  if (!storage) {
    return;
  }
  storage.setItem(FEATURE_FLAGS_STORAGE_KEY, JSON.stringify(flags));
}
