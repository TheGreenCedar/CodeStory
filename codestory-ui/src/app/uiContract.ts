export const UI_CONTRACT = {
  schemaVersion: 2,
  shellMaxWidth: 1920,
  appNavWidth: {
    min: 220,
    max: 260,
  },
  paneMinHeight: {
    desktop: 440,
    laptop: 360,
  },
  breakpoints: {
    mobile: 700,
    tablet: 1024,
    desktop: 1280,
  },
  spaceScale: {
    xs: 0.375,
    sm: 0.625,
    md: 0.875,
    lg: 1.25,
  },
} as const;

export const UI_LAYOUT_SCHEMA_STORAGE_KEY = "codestory:ui-layout-schema-version";
