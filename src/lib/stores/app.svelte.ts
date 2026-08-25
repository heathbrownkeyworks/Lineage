/**
 * Tiny cross-component signal: bumped whenever settings are saved, so pages
 * that cached settings-derived state (API key presence, scan roots, backup
 * status) know to refresh — the Settings dialog lives in the TitleBar and
 * outlives every route.
 */
export const appEvents = $state({ settingsVersion: 0 });

export function bumpSettings(): void {
  appEvents.settingsVersion++;
}
