// The slice of Tauri's global API (`app.withGlobalTauri`) the UI uses.

interface TauriGlobal {
  core: { invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> };
  event: { listen<T>(event: string, fn: (e: { payload: T }) => void): Promise<() => void> };
  window: { getCurrentWindow(): { startResizeDragging(direction: 'SouthEast'): Promise<void> } };
}

interface Window {
  __TAURI__?: TauriGlobal;
}
