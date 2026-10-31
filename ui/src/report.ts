// Surfaces load and runtime errors in the app's stderr. Loaded before main.ts
// so it also catches errors from loading the other modules, which never reach
// their own code.

const report = (m: unknown): void => { void window.__TAURI__?.core.invoke('log', { msg: String(m) }); };
addEventListener('error', (e) => report(`${e.message} @ ${e.filename}:${e.lineno}`));
addEventListener('unhandledrejection', (e) => report(`unhandled: ${e.reason}`));
