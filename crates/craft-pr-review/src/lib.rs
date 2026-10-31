//! Tauri shell around `pr-core`, `pr-github` and `pr-claude`: the commands the UI
//! calls, the tray icon, global shortcut and window management. The background
//! threads are in `poller`.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use craft_core::WindowState;
use poller::{ClaudeRequest, Status, Wake};
use pr_core::views::{self, PrFull, PrRow, RepoRow, RepoView};
use pr_core::{Config, DetailRef, Sibling, Store};
use serde::{Deserialize, Serialize};
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{
    AppHandle, Emitter, LogicalSize, Manager, RunEvent, State, WebviewWindow, WebviewWindowBuilder,
    Wry,
};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};

mod poller;
mod shrink;

const MAIN: &str = "main";
const TRAY: &str = "main";
const EV_CHANGED: &str = "pr:changed";
const EV_FOCUS: &str = "pr:focus";
const EV_META: &str = "pr:meta";
const STATE_FILE: &str = ".craft_pr_review_state.toml";
/// See `shrink`: wait past any Mission Control animation before checking.
const SHRINK_SETTLE: Duration = Duration::from_millis(1500);
/// How many PRs the review queue shows.
const QUEUE_LIMIT: usize = 400;

/// The parts of Craft Status's config this app shares.
#[derive(Clone, Default)]
struct Shared {
    repos: Vec<String>,
    github_token: String,
}

pub struct AppState {
    config: Mutex<Config>,
    shared: Mutex<Shared>,
    config_mtimes: Mutex<(Option<SystemTime>, Option<SystemTime>)>,
    store: Mutex<Store>,
    sibling: Mutex<Sibling>,
    status: Mutex<Status>,
    /// Where this app's cache lives; Claude runs in an empty directory inside it.
    dir: PathBuf,
    claude_dir: PathBuf,
    /// Shared with Craft Status: the PR ledgers, GitHub lease and budget.
    shared_cache: craft_core::SharedCache,
    forced_lists: Mutex<Vec<String>>,
    forced_details: Mutex<Vec<DetailRef>>,
    claude_requests: Mutex<VecDeque<ClaudeRequest>>,
    refresh_tx: Mutex<Option<mpsc::Sender<Wake>>>,
    claude_tx: Mutex<Option<mpsc::Sender<()>>>,
    window_state: Mutex<Option<WindowState>>,
    window_dirty: AtomicBool,
    rebuilding: AtomicBool,
    show_on_ready: AtomicBool,
    shrink_check_pending: AtomicBool,
    last_rebuild: Mutex<Option<Instant>>,
}

struct TrayItems {
    always_on_top: CheckMenuItem<Wry>,
}

#[derive(Serialize, Clone)]
struct Meta {
    config_path: String,
    shared_config_path: String,
    cache_path: String,
    opacity: f64,
    always_on_top: bool,
    platform: &'static str,
    version: &'static str,
    shortcut: String,
    frequent_over: u64,
    high_quality: u8,
}

fn meta(app: &AppHandle) -> Meta {
    let state = app.state::<AppState>();
    let cfg = state.config.lock().unwrap();
    let path = |p: pr_core::Result<PathBuf>| p.map(|p| p.display().to_string()).unwrap_or_default();
    Meta {
        config_path: path(Config::path()),
        shared_config_path: path(craft_core::Config::path()),
        cache_path: state.dir.display().to_string(),
        opacity: cfg.window.opacity,
        always_on_top: cfg.window.always_on_top,
        platform: std::env::consts::OS,
        version: env!("CARGO_PKG_VERSION"),
        shortcut: cfg.shortcuts.toggle_window.clone(),
        frequent_over: cfg.frequent_over,
        high_quality: cfg.high_quality,
    }
}

// ───────────────────────────── commands ─────────────────────────────
// `async` so reading thousands of PRs never blocks the main thread.

#[derive(Serialize)]
struct Overview {
    repos: Vec<RepoRow>,
    status: Status,
}

struct Reading {
    cfg: Config,
    repos: Vec<String>,
    sibling: Sibling,
}

fn reading(state: &AppState) -> Reading {
    let cfg = state.config.lock().unwrap().clone();
    let repos = cfg.watched(&state.shared.lock().unwrap().repos);
    let sibling = state.sibling.lock().unwrap().clone();
    Reading {
        cfg,
        repos,
        sibling,
    }
}

#[tauri::command(async)]
fn get_overview(state: State<AppState>) -> Overview {
    let r = reading(&state);
    let rows = views::overview(&state.store.lock().unwrap(), &r.repos, &r.cfg, &r.sibling);
    Overview {
        repos: rows,
        status: state.status.lock().unwrap().clone(),
    }
}

#[tauri::command(async)]
fn get_repo(state: State<AppState>, repo: String) -> Option<RepoView> {
    let r = reading(&state);
    views::repo_view(
        &state.store.lock().unwrap(),
        &repo,
        &r.repos,
        &r.cfg,
        &r.sibling,
    )
}

#[tauri::command(async)]
fn get_queue(state: State<AppState>) -> Vec<PrRow> {
    let r = reading(&state);
    views::queue(&state.store.lock().unwrap(), &r.repos, &r.cfg, QUEUE_LIMIT)
}

#[tauri::command(async)]
fn get_pr(state: State<AppState>, repo: String, number: u64) -> Option<PrFull> {
    let r = reading(&state);
    views::pr_full(
        &state.store.lock().unwrap(),
        &repo,
        number,
        &r.repos,
        &r.cfg,
        &r.sibling,
    )
}

#[tauri::command]
fn get_meta(app: AppHandle) -> Meta {
    meta(&app)
}

#[tauri::command]
fn refresh_now(state: State<AppState>) {
    wake_fetcher(&state, Wake::All);
}

#[tauri::command]
fn refresh_repo(state: State<AppState>, repo: String) {
    let mut forced = state.forced_lists.lock().unwrap();
    if !forced.contains(&repo) {
        forced.push(repo);
    }
    drop(forced);
    wake_fetcher(&state, Wake::Repo);
}

/// Inspect one PR now.
#[tauri::command]
fn refresh_pr(state: State<AppState>, repo: String, number: u64) {
    let id = state
        .store
        .lock()
        .unwrap()
        .pr(&repo, number)
        .map(|p| p.id.clone());
    if let Some(id) = id.filter(|i| !i.is_empty()) {
        let re = DetailRef { repo, number, id };
        let mut forced = state.forced_details.lock().unwrap();
        if !forced.contains(&re) {
            forced.push(re);
        }
        drop(forced);
        wake_fetcher(&state, Wake::Pr);
    }
}

/// Ask Claude about one PR now, with its diff if `deep`.
#[tauri::command]
fn analyze_pr(state: State<AppState>, repo: String, number: u64, deep: bool) {
    state
        .claude_requests
        .lock()
        .unwrap()
        .push_back(ClaudeRequest { repo, number, deep });
    if let Some(tx) = state.claude_tx.lock().unwrap().as_ref() {
        let _ = tx.send(());
    }
}

/// Only GitHub links leave the app, and they go to the default browser.
#[tauri::command]
fn open_url(url: String) -> Result<(), String> {
    if !url.starts_with("https://github.com/") {
        return Err(format!("refusing to open {url:?}"));
    }
    open_external(&url)
}

#[tauri::command]
fn open_config() -> Result<(), String> {
    let path = Config::path().map_err(|e| e.to_string())?;
    open_external(&path.display().to_string())
}

#[tauri::command]
fn set_always_on_top(app: AppHandle, on: bool) {
    set_pinned(&app, on);
}

/// Called once the UI has painted, so the window never flashes empty.
#[tauri::command]
fn window_ready(app: AppHandle, state: State<AppState>) {
    let start_hidden = state.config.lock().unwrap().window.start_hidden;
    if !start_hidden || state.show_on_ready.swap(false, Ordering::Relaxed) {
        if let Some(w) = app.get_webview_window(MAIN) {
            show_window(&w);
        }
    }
}

#[tauri::command]
fn hide_window(app: AppHandle) {
    if let Some(w) = app.get_webview_window(MAIN) {
        let _ = w.hide();
    }
}

#[tauri::command]
fn log(msg: String) {
    eprintln!("[ui] {msg}");
}

// ───────────────────────────── shared state ─────────────────────────────

fn wake_fetcher(state: &AppState, why: Wake) {
    if let Some(tx) = state.refresh_tx.lock().unwrap().as_ref() {
        let _ = tx.send(why);
    }
}

/// Tells the UI to reread whatever it's showing.
pub(crate) fn emit_changed(app: &AppHandle) {
    let _ = app.emit(EV_CHANGED, ());
}

fn mtime(path: pr_core::Result<PathBuf>) -> Option<SystemTime> {
    path.ok()
        .and_then(|p| std::fs::metadata(p).ok())
        .and_then(|m| m.modified().ok())
}

fn read_shared() -> Result<Shared, String> {
    let path = craft_core::Config::path().map_err(|e| e.to_string())?;
    // Only ever read: if Craft Status hasn't created it, use its defaults.
    let cfg = if path.exists() {
        craft_core::Config::load_from(&path).map_err(|e| e.to_string())?
    } else {
        craft_core::Config::default()
    };
    Ok(Shared {
        repos: cfg.repos,
        github_token: cfg.github_token,
    })
}

/// Re-reads whichever config changed. A broken file keeps the old settings and
/// shows the error in the window.
pub(crate) fn reload_configs_if_changed(app: &AppHandle) {
    let state = app.state::<AppState>();
    let now = (mtime(Config::path()), mtime(craft_core::Config::path()));
    let mut was = state.config_mtimes.lock().unwrap();
    if *was == now {
        return;
    }
    *was = now;
    drop(was);
    let mut problems = vec![];
    match Config::load_or_create() {
        Ok(cfg) => {
            *state.config.lock().unwrap() = cfg.clone();
            apply_window_config(app, &cfg);
            let _ = app.emit(EV_META, meta(app));
        }
        Err(e) => problems.push(format!("Config error: {e}")),
    }
    match read_shared() {
        Ok(shared) => *state.shared.lock().unwrap() = shared,
        Err(e) => problems.push(format!("Craft Status config error: {e}")),
    }
    if !problems.is_empty() {
        state.status.lock().unwrap().error = Some(problems.join("; "));
    }
    emit_changed(app);
}

// ───────────────────────────── window & tray ─────────────────────────────

fn open_external(target: &str) -> Result<(), String> {
    use std::process::Command;
    #[cfg(target_os = "macos")]
    let mut cmd = {
        let mut c = Command::new("open");
        c.arg(target);
        c
    };
    #[cfg(target_os = "windows")]
    let mut cmd = {
        let mut c = Command::new("cmd");
        c.args(["/C", "start", ""]).arg(target);
        c
    };
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut cmd = {
        let mut c = Command::new("xdg-open");
        c.arg(target);
        c
    };
    cmd.spawn().map(|_| ()).map_err(|e| e.to_string())
}

fn show_window(w: &WebviewWindow) {
    let _ = w.show();
    let _ = w.unminimize();
    let _ = w.set_focus();
    let _ = w.emit(EV_FOCUS, true);
}

/// Tray click / global hotkey: hide if focused, otherwise bring to front on
/// the current desktop.
fn toggle_window(app: &AppHandle) {
    let Some(w) = app.get_webview_window(MAIN) else {
        recreate_main_window(app);
        return;
    };
    if w.is_visible().unwrap_or(false) && w.is_focused().unwrap_or(false) {
        let _ = w.hide();
    } else {
        show_window(&w);
    }
}

fn set_pinned(app: &AppHandle, on: bool) {
    let state = app.state::<AppState>();
    state.config.lock().unwrap().window.always_on_top = on;
    if let Some(w) = app.get_webview_window(MAIN) {
        let _ = w.set_always_on_top(on);
    }
    if let Some(items) = app.try_state::<TrayItems>() {
        let _ = items.always_on_top.set_checked(on);
    }
    let _ = app.emit(EV_META, meta(app));
}

fn apply_window_config(app: &AppHandle, cfg: &Config) {
    if let Some(w) = app.get_webview_window(MAIN) {
        let _ = w.set_always_on_top(cfg.window.always_on_top);
        let _ = w.set_visible_on_all_workspaces(cfg.tray.visible_on_all_workspaces);
        #[cfg(not(target_os = "macos"))]
        let _ = w.set_skip_taskbar(true);
    }
    if let Some(items) = app.try_state::<TrayItems>() {
        let _ = items.always_on_top.set_checked(cfg.window.always_on_top);
    }
    let gs = app.global_shortcut();
    let _ = gs.unregister_all();
    let accel = cfg.shortcuts.toggle_window.trim();
    if accel.is_empty() {
        return;
    }
    let result = gs.on_shortcut(accel, |app, _shortcut, event| {
        if event.state() == ShortcutState::Pressed {
            toggle_window(app);
        }
    });
    if let Err(e) = result {
        eprintln!("Could not register shortcut {accel:?}: {e}");
    }
}

/// "Craft PR Review — 28 landable, 143 promising, of 7891 open".
fn tray_tooltip(rows: &[RepoRow]) -> String {
    let open: u64 = rows.iter().map(|r| r.open).sum();
    if open == 0 {
        return "Craft PR Review — no data yet".to_string();
    }
    let promising: u64 = rows.iter().map(|r| r.promising).sum();
    let land: u64 = rows.iter().map(|r| r.landable).sum();
    format!("Craft PR Review — {land} landable, {promising} promising, of {open} open")
}

pub(crate) fn update_tray(app: &AppHandle) {
    let state = app.state::<AppState>();
    let r = reading(&state);
    let rows = views::overview(&state.store.lock().unwrap(), &r.repos, &r.cfg, &r.sibling);
    if let Some(tray) = app.tray_by_id(TRAY) {
        let _ = tray.set_tooltip(Some(tray_tooltip(&rows)));
    }
}

#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
struct StateFile {
    window: Option<WindowState>,
}

fn state_path() -> Option<PathBuf> {
    craft_core::Config::path()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join(STATE_FILE)))
}

fn load_window_state() -> Option<WindowState> {
    let text = std::fs::read_to_string(state_path()?).ok()?;
    let f: StateFile = serde_json::from_str(&text).ok()?;
    f.window.filter(|w| w.width > 0 && w.height > 0)
}

fn note_window_geometry(app: &AppHandle) {
    let Some(w) = app.get_webview_window(MAIN) else {
        return;
    };
    let (Ok(pos), Ok(size)) = (w.outer_position(), w.outer_size()) else {
        return;
    };
    if size.width == 0 || size.height == 0 || !w.is_visible().unwrap_or(false) {
        return;
    }
    let state = app.state::<AppState>();
    *state.window_state.lock().unwrap() = Some(WindowState {
        x: pos.x,
        y: pos.y,
        width: size.width,
        height: size.height,
    });
    state.window_dirty.store(true, Ordering::Relaxed);
}

pub(crate) fn flush_window_state(app: &AppHandle) {
    let state = app.state::<AppState>();
    if !state.window_dirty.swap(false, Ordering::Relaxed) {
        return;
    }
    let saved = *state.window_state.lock().unwrap();
    if let (Some(ws), Some(path)) = (saved, state_path()) {
        let text = serde_json::to_string(&StateFile { window: Some(ws) }).unwrap_or_default();
        if let Err(e) = pr_core::atomic_write(&path, &text) {
            eprintln!("could not save window state: {e}");
        }
    }
}

fn restore_window(app: &AppHandle, cfg: &Config) {
    let Some(w) = app.get_webview_window(MAIN) else {
        return;
    };
    let monitors: Vec<(i32, i32, u32, u32)> = app
        .available_monitors()
        .unwrap_or_default()
        .iter()
        .map(|m| {
            (
                m.position().x,
                m.position().y,
                m.size().width,
                m.size().height,
            )
        })
        .collect();
    if let Some(ws) = load_window_state().filter(|ws| ws.visible_on(&monitors)) {
        let _ = w.set_size(tauri::PhysicalSize::new(ws.width, ws.height));
        let _ = w.set_position(tauri::PhysicalPosition::new(ws.x, ws.y));
        *app.state::<AppState>().window_state.lock().unwrap() = Some(ws);
    } else {
        let _ = w.set_size(LogicalSize::new(cfg.window.width, cfg.window.height));
        let _ = w.center();
    }
}

/// Watch for the half-size glitch (see `shrink`) shortly after the window gains
/// focus. It must persist across two checks, then the window is recreated.
fn schedule_shrink_check(app: &AppHandle) {
    let state = app.state::<AppState>();
    if state.shrink_check_pending.swap(true, Ordering::Relaxed) {
        return;
    }
    let app = app.clone();
    let spawned = std::thread::Builder::new()
        .name("shrink-check".into())
        .spawn(move || {
            let confirmed = (0..2).all(|_| {
                std::thread::sleep(SHRINK_SETTLE);
                shrunk_now(&app)
            });
            if confirmed {
                let later = app.clone();
                let _ = app.run_on_main_thread(move || rebuild_main_window(&later));
            }
            app.state::<AppState>()
                .shrink_check_pending
                .store(false, Ordering::Relaxed);
        });
    if spawned.is_err() {
        state.shrink_check_pending.store(false, Ordering::Relaxed);
    }
}

fn shrunk_now(app: &AppHandle) -> bool {
    let (tx, rx) = mpsc::channel();
    let handle = app.clone();
    let asked = app.run_on_main_thread(move || {
        let shrunk = handle
            .get_webview_window(MAIN)
            .is_some_and(|w| shrink::window_shrunk(&w));
        let _ = tx.send(shrunk);
    });
    asked.is_ok() && rx.recv_timeout(Duration::from_secs(5)).unwrap_or(false)
}

fn rebuild_main_window(app: &AppHandle) {
    let state = app.state::<AppState>();
    let Ok(mut last) = state.last_rebuild.lock() else {
        return;
    };
    let now = Instant::now();
    if !shrink::may_rebuild(*last, now) {
        return;
    }
    let Some(w) = app.get_webview_window(MAIN) else {
        return;
    };
    *last = Some(now);
    drop(last);
    eprintln!("main window is shown shrunk; recreating it");
    note_window_geometry(app);
    flush_window_state(app);
    state.rebuilding.store(true, Ordering::Relaxed);
    if let Err(e) = w.destroy() {
        eprintln!("could not destroy the shrunk window: {e}");
        state.rebuilding.store(false, Ordering::Relaxed);
    }
}

fn recreate_main_window(app: &AppHandle) {
    let state = app.state::<AppState>();
    if app.get_webview_window(MAIN).is_none() {
        state.show_on_ready.store(true, Ordering::Relaxed);
        let built = app
            .config()
            .app
            .windows
            .iter()
            .find(|c| c.label == MAIN)
            .map(|c| WebviewWindowBuilder::from_config(app, c).and_then(|b| b.build()));
        match built {
            Some(Ok(_)) => {
                let cfg = state.config.lock().map(|c| c.clone()).unwrap_or_default();
                restore_window(app, &cfg);
                apply_window_config(app, &cfg);
            }
            Some(Err(e)) => eprintln!("could not recreate the main window: {e}"),
            None => eprintln!("no {MAIN:?} window in tauri.conf.json"),
        }
        if app.get_webview_window(MAIN).is_none() {
            state.show_on_ready.store(false, Ordering::Relaxed);
        }
    }
    state.rebuilding.store(false, Ordering::Relaxed);
}

fn build_tray(app: &tauri::App, cfg: &Config) -> tauri::Result<()> {
    let toggle = MenuItem::with_id(app, "toggle", "Show / Hide", true, None::<&str>)?;
    let refresh = MenuItem::with_id(app, "refresh", "Refresh Now", true, None::<&str>)?;
    let on_top = CheckMenuItem::with_id(
        app,
        "on_top",
        "Always on Top",
        true,
        cfg.window.always_on_top,
        None::<&str>,
    )?;
    let open_cfg = MenuItem::with_id(app, "open_config", "Edit Config…", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit Craft PR Review", true, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[
            &toggle,
            &refresh,
            &on_top,
            &PredefinedMenuItem::separator(app)?,
            &open_cfg,
            &PredefinedMenuItem::separator(app)?,
            &quit,
        ],
    )?;
    app.manage(TrayItems {
        always_on_top: on_top.clone(),
    });

    #[cfg(target_os = "macos")]
    let icon = tauri::image::Image::from_bytes(include_bytes!("../icons/tray.png"))?;
    #[cfg(not(target_os = "macos"))]
    let icon = app
        .default_window_icon()
        .cloned()
        .unwrap_or(tauri::image::Image::from_bytes(include_bytes!(
            "../icons/32x32.png"
        ))?);

    TrayIconBuilder::with_id(TRAY)
        .icon(icon)
        .icon_as_template(true)
        .tooltip("Craft PR Review")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "toggle" => toggle_window(app),
            "refresh" => wake_fetcher(&app.state::<AppState>(), Wake::All),
            "on_top" => {
                let checked = app
                    .try_state::<TrayItems>()
                    .and_then(|t| t.always_on_top.is_checked().ok())
                    .unwrap_or(false);
                set_pinned(app, checked);
            }
            "open_config" => {
                let _ = open_config();
            }
            "quit" => {
                flush_window_state(app);
                app.exit(0)
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                toggle_window(tray.app_handle());
            }
        })
        .build(app)?;
    Ok(())
}

pub fn run() {
    let config = Config::load_or_create().unwrap_or_else(|e| {
        eprintln!("Could not load ~/.craft_pr_review_config.toml ({e}); using defaults");
        Config::default()
    });
    let shared = read_shared().unwrap_or_else(|e| {
        eprintln!("Could not load Craft Status's config ({e}); using its defaults");
        Shared {
            repos: craft_core::Config::default().repos,
            github_token: String::new(),
        }
    });
    let dir = Store::default_dir().unwrap_or_else(|_| std::env::temp_dir().join("craft-pr-review"));
    let mut store = Store::load(&dir);
    let shared_cache = craft_core::SharedCache::default_location()
        .unwrap_or_else(|_| craft_core::SharedCache::at(dir.join("shared")));
    store.hydrate(&shared_cache, &config.watched(&shared.repos));
    let mut sibling = Sibling::default();
    sibling.refresh();

    let (tx, rx) = mpsc::channel();
    let (claude_tx, claude_rx) = mpsc::channel();
    let state = AppState {
        config: Mutex::new(config),
        shared: Mutex::new(shared),
        config_mtimes: Mutex::new((mtime(Config::path()), mtime(craft_core::Config::path()))),
        store: Mutex::new(store),
        sibling: Mutex::new(sibling),
        status: Mutex::new(Status::default()),
        claude_dir: dir.join("claude-cwd"),
        shared_cache,
        dir,
        forced_lists: Mutex::new(vec![]),
        forced_details: Mutex::new(vec![]),
        claude_requests: Mutex::new(VecDeque::new()),
        refresh_tx: Mutex::new(Some(tx)),
        claude_tx: Mutex::new(Some(claude_tx)),
        window_state: Mutex::new(None),
        window_dirty: AtomicBool::new(false),
        rebuilding: AtomicBool::new(false),
        show_on_ready: AtomicBool::new(false),
        shrink_check_pending: AtomicBool::new(false),
        last_rebuild: Mutex::new(None),
    };

    tauri::Builder::default()
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .manage(state)
        .invoke_handler(tauri::generate_handler![
            get_overview,
            get_repo,
            get_queue,
            get_pr,
            get_meta,
            refresh_now,
            refresh_repo,
            refresh_pr,
            analyze_pr,
            open_url,
            open_config,
            set_always_on_top,
            window_ready,
            hide_window,
            log,
        ])
        .setup(move |app| {
            let cfg = app.state::<AppState>().config.lock().unwrap().clone();

            #[cfg(target_os = "macos")]
            if cfg.tray.hide_dock_icon {
                app.set_activation_policy(tauri::ActivationPolicy::Accessory);
            }

            restore_window(app.handle(), &cfg);
            build_tray(app, &cfg)?;
            apply_window_config(app.handle(), &cfg);
            update_tray(app.handle());
            poller::spawn_fetcher(app.handle().clone(), rx);
            poller::spawn_claude(app.handle().clone(), claude_rx);
            Ok(())
        })
        .on_window_event(|window, event| match event {
            tauri::WindowEvent::CloseRequested { api, .. } => {
                let close_to_tray = window
                    .state::<AppState>()
                    .config
                    .lock()
                    .unwrap()
                    .tray
                    .close_to_tray;
                flush_window_state(window.app_handle());
                if close_to_tray {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
            tauri::WindowEvent::Focused(focused) => {
                let _ = window.emit(EV_FOCUS, *focused);
                if *focused && window.label() == MAIN {
                    schedule_shrink_check(window.app_handle());
                }
            }
            tauri::WindowEvent::Destroyed if window.label() == MAIN => {
                let app = window.app_handle().clone();
                if app.state::<AppState>().rebuilding.load(Ordering::Relaxed) {
                    // Not from inside this event, nor queued onto the main
                    // thread: a webview built there never loads its page.
                    let later = app.clone();
                    let spawned = std::thread::Builder::new()
                        .name("recreate-window".into())
                        .spawn(move || recreate_main_window(&later));
                    if spawned.is_err() {
                        app.state::<AppState>()
                            .rebuilding
                            .store(false, Ordering::Relaxed);
                    }
                }
            }
            tauri::WindowEvent::Resized(_) | tauri::WindowEvent::Moved(_) => {
                note_window_geometry(window.app_handle())
            }
            _ => {}
        })
        .build(tauri::generate_context!())
        .expect("error while running Craft PR Review")
        .run(|app, event| {
            if let RunEvent::ExitRequested {
                code: None, api, ..
            } = event
            {
                if app.state::<AppState>().rebuilding.load(Ordering::Relaxed) {
                    api.prevent_exit();
                }
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tooltip_counts_what_can_be_reviewed() {
        assert!(tray_tooltip(&[]).contains("no data"));
        let rows = [
            RepoRow {
                open: 10,
                promising: 3,
                landable: 1,
                ..Default::default()
            },
            RepoRow {
                open: 5,
                promising: 2,
                ..Default::default()
            },
        ];
        assert_eq!(
            tray_tooltip(&rows),
            "Craft PR Review — 1 landable, 5 promising, of 15 open"
        );
    }

    #[test]
    fn only_github_links_open() {
        assert!(open_url("https://evil.example/x".into()).is_err());
        assert!(open_url("file:///etc/passwd".into()).is_err());
        assert!(open_url("https://github.com.evil.example/x".into()).is_err());
    }
}
