//! The macOS menu bar: SparkBox, File, Edit, View, Go, Cluster, Window and Help.
//!
//! Every custom item emits `menu://<id>` to the page, which runs the command with
//! the same id (src/lib/menuBridge.ts), so the shortcuts work even while a
//! terminal has keyboard focus. Items built from data (the Window list, Go ›
//! Nodes, Demo Scenarios, saved layouts) emit one event each, with their target
//! as the payload. The page reports back which inner windows are open
//! (`menu_set_windows`) and the state labels and checkmarks show (`menu_set_state`).
//!
//! The Help menu is built from `docs/help/manifest.json`, the same file the Help window's table
//! of contents comes from. Its `toc` entries expand to a section of that table, so every article
//! is in the menu, and macOS's Help search finds it, without anyone listing it twice.

use std::collections::HashMap;
use std::sync::Mutex;

use serde::Deserialize;
use tauri::menu::{AboutMetadata, CheckMenuItem, IsMenuItem, Menu, MenuItem, MenuItemKind, PredefinedMenuItem, Submenu};
use tauri::plugin::TauriPlugin;
use tauri::{App, AppHandle, Emitter, LogicalPosition, LogicalSize, Manager, WebviewWindow, Wry};
use tauri_plugin_window_state::StateFlags;

use crate::identity;

/// Every fixed custom item: (id, label, accelerator). src/lib/keymap.ts mirrors the
/// accelerators for Keyboard Shortcuts and the browser preview; a test keeps them in step.
pub const ITEMS: &[(&str, &str, Option<&str>)] = &[
    ("app:settings", "Settings…", Some("CmdOrCtrl+,")),
    ("term:new-tab", "New Terminal Tab", Some("CmdOrCtrl+T")),
    ("files:open", "Open Files", None),
    ("wm:close", "Close Window", Some("CmdOrCtrl+W")),
    ("wm:close-all", "Close All Windows", Some("CmdOrCtrl+Alt+W")),
    ("wm:reopen", "Reopen Closed Window", Some("CmdOrCtrl+Shift+T")),
    ("wm:find", "Find…", Some("CmdOrCtrl+F")),
    ("app:palette", "Command Palette…", Some("CmdOrCtrl+K")),
    ("app:sidebar", "Toggle Sidebar", Some("Ctrl+CmdOrCtrl+S")),
    ("wm:cascade", "Cascade", Some("CmdOrCtrl+Alt+C")),
    ("wm:tile", "Tile All", Some("CmdOrCtrl+Alt+T")),
    ("wm:tile-two", "Tile Two Side by Side", None),
    ("wm:tile-four", "Tile Four", None),
    ("wm:restore-layout", "Restore Layout", None),
    ("wm:popout", "Open in Separate Window", Some("CmdOrCtrl+Shift+O")),
    ("wm:snap-left", "Snap Left", Some("Ctrl+Alt+Left")),
    ("wm:snap-right", "Snap Right", Some("Ctrl+Alt+Right")),
    ("wm:fill", "Fill Desktop", Some("Ctrl+Alt+Up")),
    ("wm:restore", "Restore Size", Some("Ctrl+Alt+Down")),
    ("wm:overview", "Show All Windows", None),
    ("wm:refresh", "Refresh", Some("CmdOrCtrl+R")),
    ("app:zoom-reset", "Actual Size", Some("CmdOrCtrl+0")),
    ("app:zoom-in", "Zoom In", Some("CmdOrCtrl+=")),
    ("app:zoom-out", "Zoom Out", Some("CmdOrCtrl+-")),
    ("go:dashboard", "Dashboard", Some("CmdOrCtrl+1")),
    ("go:topology", "Topology", Some("CmdOrCtrl+2")),
    ("go:containers", "Containers", Some("CmdOrCtrl+3")),
    ("go:workloads", "Workloads", Some("CmdOrCtrl+4")),
    ("go:models", "Models", Some("CmdOrCtrl+5")),
    ("go:howto", "How To", None),
    ("go:rdp", "RDP", None),
    ("go:terminal", "Terminal", Some("CmdOrCtrl+6")),
    ("go:files", "Files", Some("CmdOrCtrl+7")),
    ("go:activity", "Activity Log", Some("CmdOrCtrl+8")),
    ("go:alerts", "Alerts", Some("CmdOrCtrl+9")),
    // ⇧⌘L, so ⌘L stays free for the editor's Go to Line.
    ("cluster:admin", "Unlock Admin Mode…", Some("CmdOrCtrl+Shift+L")),
    ("go:tasks", "Tasks", Some("CmdOrCtrl+Shift+J")),
    ("wm:minimize", "Minimize Window", Some("CmdOrCtrl+M")),
    ("app:minimize", "Minimize SparkBox", Some("CmdOrCtrl+Alt+M")),
    ("wm:next", "Next Window", Some("CmdOrCtrl+`")),
    ("wm:prev", "Previous Window", Some("CmdOrCtrl+Shift+`")),
    ("wm:save-layout", "Save Layout…", None),
    ("wm:front", "Bring All to Front", None),
    ("app:shortcuts", "Show Keyboard Shortcuts", Some("CmdOrCtrl+/")),
    // Shift-/ is the key the Mac calls ⌘?. The menu shows it as ⇧⌘/, and AppKit doesn't match it
    // (the keys type "?", this item's key is "/"): src/lib/keymap.ts answers ⌘? from the page.
    ("help:open", "SparkBox Help", Some("CmdOrCtrl+Shift+/")),
    ("app:welcome", "Show Welcome", None),
    ("app:doctor", "Open Diagnostics…", None),
    ("app:activity", "Show Activity Log", None),
];

/// Checkmark items the page drives through `menu_set_state`.
const SOURCE_DEMO: &str = "cluster:source-demo";
const SOURCE_LIVE: &str = "cluster:source-live";
const CONTROLS: &str = "app:controls-style";
/// View › Hide Sidebar and Tile All: the sidebar's room goes to the tile (the page decides the rest).
const TILE_WIDE: &str = "wm:tile-wide";

/// Prefixes of items built from data; what follows is the payload.
const WIN_PREFIX: &str = "win:";
const NODE_PREFIX: &str = "go-node:";
const SCENARIO_PREFIX: &str = "scenario:";
const LAYOUT_PREFIX: &str = "layout:";
/// A Help menu item for one article; what follows is the article's id.
const HELP_PREFIX: &str = "help-article:";
/// A Help menu item for a section's page of articles; what follows is the section's id.
const HELP_SECTION_PREFIX: &str = "help-section:";
/// An article may be listed twice (a main How-To is also in its group). Menu ids stay unique: what
/// follows this in an id is ignored.
const HELP_REPEAT: char = '@';
/// The Help menu opens Help itself where the manifest names no article.
const HELP_OPEN: &str = "help:open";

/// The spec of Help, compiled in. `helpMenu`, `toc` and the articles' titles are read here.
const HELP_MANIFEST: &str = include_str!("../../docs/help/manifest.json");

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HelpManifest {
    help_menu: Vec<HelpEntry>,
    toc: Vec<HelpSection>,
    articles: Vec<HelpArticle>,
}

#[derive(Debug, Deserialize)]
struct HelpSection {
    id: String,
    groups: Vec<HelpGroup>,
}

#[derive(Debug, Deserialize)]
struct HelpGroup {
    title: Option<String>,
    articles: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct HelpArticle {
    id: String,
    title: String,
}

/// One line of the Help menu, as `docs/help/manifest.json` gives it.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
struct HelpEntry {
    label: String,
    article: Option<String>,
    /// An id from ITEMS, for the items that aren't articles.
    command: Option<String>,
    /// Opens a section's page of articles.
    section: Option<String>,
    /// Stands for every group and article of this section of the table of contents.
    toc: Option<String>,
    separator: bool,
    submenu: Option<Vec<HelpEntry>>,
}

/// The Help menu's entries, with its `toc` entries expanded. A malformed manifest can't ship: a
/// test reads this one.
fn help_entries() -> Vec<HelpEntry> {
    serde_json::from_str::<HelpManifest>(HELP_MANIFEST).map(|m| expand_toc(&m.help_menu, &m)).unwrap_or_default()
}

/// Replaces each `toc` entry with its section: a submenu per titled group, or the bare list.
fn expand_toc(entries: &[HelpEntry], m: &HelpManifest) -> Vec<HelpEntry> {
    let mut out = Vec::new();
    for e in entries {
        if let Some(section) = e.toc.as_deref().and_then(|id| m.toc.iter().find(|s| s.id == id)) {
            for g in &section.groups {
                let items: Vec<HelpEntry> = g.articles.iter().map(|id| article_entry(id, g.title.as_deref(), m)).collect();
                match &g.title {
                    Some(title) => out.push(HelpEntry { label: menu_label(title), submenu: Some(items), ..HelpEntry::default() }),
                    None => out.extend(items),
                }
            }
        } else if let Some(sub) = &e.submenu {
            out.push(HelpEntry { submenu: Some(expand_toc(sub, m)), ..e.clone() });
        } else {
            out.push(e.clone());
        }
    }
    out
}

fn article_entry(id: &str, group: Option<&str>, m: &HelpManifest) -> HelpEntry {
    let title = m.articles.iter().find(|a| a.id == id).map_or(id, |a| a.title.as_str());
    // "Settings: Fence" in the Settings submenu is "Fence".
    let short = group.and_then(|g| title.strip_prefix(g)).and_then(|t| t.strip_prefix(": ")).unwrap_or(title);
    HelpEntry { label: menu_label(short), article: Some(id.to_string()), ..HelpEntry::default() }
}

/// A title in sentence case as a menu item in title case, the Mac's style. Names keep their
/// spelling: "vLLM", "SSH". The first word, and a word after a colon, always take a capital.
fn menu_label(title: &str) -> String {
    const SMALL: &[&str] = &["a", "an", "and", "as", "at", "by", "for", "from", "in", "into", "of", "on", "or", "the", "to", "with"];
    const LOWER: &[&str] = &["sudo", "apt"];
    let capital = |w: &str| {
        let mut c = w.chars();
        c.next().map_or(String::new(), |f| f.to_uppercase().chain(c).collect())
    };
    let mut out: Vec<String> = Vec::new();
    for (i, word) in title.split(' ').enumerate() {
        let opens = i == 0 || out.last().is_some_and(|w: &String| w.ends_with(':'));
        let plain = word.trim_end_matches([',', ':']);
        let kept = plain.chars().any(|c| c.is_uppercase() || c.is_ascii_digit()) || LOWER.contains(&plain);
        out.push(if kept || (!opens && SMALL.contains(&plain)) {
            word.to_string()
        } else {
            word.split('-').map(capital).collect::<Vec<_>>().join("-")
        });
    }
    out.join(" ")
}

/// One inner window, as the page reports it.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowEntry {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub active: bool,
    #[serde(default)]
    pub minimized: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Named {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Titled {
    pub id: String,
    pub title: String,
}

/// What menu labels and checkmarks show.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct MenuState {
    pub admin: bool,
    pub demo: bool,
    pub scenario: Option<String>,
    pub nodes: Vec<Named>,
    pub scenarios: Vec<Titled>,
    pub layouts: Vec<String>,
    /// Command ids that can't run right now (greyed out).
    pub disabled: Vec<String>,
    /// "mac" or "winbox".
    pub controls: Option<String>,
}

struct Handles {
    items: HashMap<String, MenuItem<Wry>>,
    demo: CheckMenuItem<Wry>,
    live: CheckMenuItem<Wry>,
    controls: CheckMenuItem<Wry>,
    window_menu: Submenu<Wry>,
    /// Where the Window list starts inside the Window menu.
    list_at: usize,
    list: Vec<MenuItemKind<Wry>>,
    nodes: Submenu<Wry>,
    scenarios: Submenu<Wry>,
    layouts: Submenu<Wry>,
}

/// Managed state: the item handles, the last state applied, and how many inner windows are open.
pub struct MenuBar {
    handles: Mutex<Option<Handles>>,
    last_state: Mutex<Option<MenuState>>,
    last_windows: Mutex<Option<Vec<WindowEntry>>>,
    /// None until the page first reports.
    inner_windows: Mutex<Option<usize>>,
}

/// The main window's label, and its smallest size (tauri.conf.json's minWidth and minHeight).
const MAIN: &str = "main";
const MIN_SIZE: Size = Size { w: 960.0, h: 600.0 };

/// Remembers the main window's size, position and zoom between launches. Not full screen: a
/// window quit in full screen reopens windowed, where it can be moved and resized. Not its
/// visibility: a login launch starts hidden in the menu bar. Debug builds keep their own file
/// (identity::window_state_file), so a test run never moves the release window. The main window
/// is restored in `install`, then kept on a screen.
pub fn window_state_plugin() -> TauriPlugin<Wry> {
    tauri_plugin_window_state::Builder::default()
        .with_filename(identity::window_state_file())
        .with_state_flags(StateFlags::SIZE | StateFlags::POSITION | StateFlags::MAXIMIZED)
        .skip_initial_state(MAIN)
        .build()
}

/// A rectangle in logical points, in the desktop's coordinates (y grows downwards).
#[derive(Debug, Clone, Copy, PartialEq)]
struct Rect {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Size {
    w: f64,
    h: f64,
}

impl Rect {
    fn overlap(&self, o: &Rect) -> f64 {
        let w = (self.x + self.w).min(o.x + o.w) - self.x.max(o.x);
        let h = (self.y + self.h).min(o.y + o.h) - self.y.max(o.y);
        if w > 0.0 && h > 0.0 {
            w * h
        } else {
            0.0
        }
    }

    fn contains(&self, x: f64, y: f64) -> bool {
        x >= self.x && x < self.x + self.w && y >= self.y && y < self.y + self.h
    }
}

/// Where a restored window goes. `work_areas` are the screens minus the menu bar and Dock, the
/// primary first. The screen holding the saved frame's centre wins, else the one it overlaps
/// most, else the primary (its screen is gone). The frame shrinks to fit that work area, never
/// below `min`, and moves fully inside it, title bar first.
fn clamp(saved: Rect, work_areas: &[Rect], min: Size) -> Rect {
    let (cx, cy) = (saved.x + saved.w / 2.0, saved.y + saved.h / 2.0);
    let area = work_areas
        .iter()
        .find(|a| a.contains(cx, cy))
        .or_else(|| work_areas.iter().filter(|a| a.overlap(&saved) > 0.0).max_by(|a, b| a.overlap(&saved).total_cmp(&b.overlap(&saved))))
        .or(work_areas.first());
    let Some(a) = area else { return saved };
    let w = saved.w.min(a.w).max(min.w);
    let h = saved.h.min(a.h).max(min.h);
    // Right and bottom edges inside first, then left and top, so a window bigger than a small
    // screen keeps its title bar and traffic lights in reach.
    let x = saved.x.min(a.x + a.w - w).max(a.x);
    let y = saved.y.min(a.y + a.h - h).max(a.y);
    Rect { x, y, w, h }
}

/// The main window's saved frame, as the window-state plugin wrote it: the content size and the
/// frame's top left, in the physical pixels of the screen it was on.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
struct Saved {
    width: u32,
    height: u32,
    x: i32,
    y: i32,
    #[serde(default)]
    maximized: bool,
}

/// The saved frame in the window-state file (`identity::window_state_file`), if there is one.
fn read_saved(app: &AppHandle) -> Option<Saved> {
    let path = app.path().app_config_dir().ok()?.join(identity::window_state_file());
    let all: HashMap<String, Saved> = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
    all.get(MAIN).copied().filter(|s| s.width > 0 && s.height > 0)
}

/// `saved` in logical points. The plugin saved physical pixels at the scale of the screen the
/// window was on, which a mixed-scale setup can't tell back apart; like the plugin's own restore,
/// this reads them at the scale the window opened with (the main screen's). `clamp` then puts the
/// frame on a screen either way.
fn saved_frame(saved: &Saved, scale: f64, chrome: Size) -> Rect {
    Rect { x: saved.x as f64 / scale, y: saved.y as f64 / scale, w: saved.width as f64 / scale + chrome.w, h: saved.height as f64 / scale + chrome.h }
}

/// Puts the main window where it was, kept on a screen (`clamp`), then zooms it if it was zoomed,
/// so un-zooming returns to an on-screen frame. The saved frame is read from the file, not from
/// the window: macOS applies a new size and position later, so the window can't be read back here.
fn restore_main(app: &AppHandle) {
    let Some(w) = app.get_webview_window(MAIN) else { return };
    let _ = w.set_min_size(Some(LogicalSize::new(MIN_SIZE.w, MIN_SIZE.h)));
    let saved = read_saved(app);
    if let Err(e) = place(&w, saved.as_ref()) {
        eprintln!("couldn't put the window back on screen: {e}");
    }
    if saved.is_some_and(|s| s.maximized) {
        let _ = w.maximize();
    }
}

/// Sizes and moves the window to the clamped saved frame (or its first-launch frame), in logical units.
fn place(w: &WebviewWindow, saved: Option<&Saved>) -> tauri::Result<()> {
    let scale = w.scale_factor()?;
    let outer = w.outer_size()?.to_logical::<f64>(scale);
    let inner = w.inner_size()?.to_logical::<f64>(scale);
    // What the frame adds to the content: nothing with the Overlay title bar.
    let chrome = Size { w: outer.width - inner.width, h: outer.height - inner.height };
    let primary = w.primary_monitor()?;
    let mut monitors = w.available_monitors()?;
    // The primary first: it takes a window whose screen is gone.
    if let Some(p) = &primary {
        if let Some(i) = monitors.iter().position(|m| m.position() == p.position() && m.size() == p.size()) {
            monitors.swap(0, i);
        }
    }
    let areas: Vec<Rect> = monitors
        .iter()
        .map(|m| {
            let s = m.scale_factor();
            let a = m.work_area();
            Rect { x: a.position.x as f64 / s, y: a.position.y as f64 / s, w: a.size.width as f64 / s, h: a.size.height as f64 / s }
        })
        .collect();
    let frame = match saved {
        Some(s) => saved_frame(s, scale, chrome),
        // First launch: where tauri.conf.json put it (centred).
        None => {
            let pos = w.outer_position()?.to_logical::<f64>(scale);
            Rect { x: pos.x, y: pos.y, w: outer.width, h: outer.height }
        }
    };
    let fit = clamp(frame, &areas, MIN_SIZE);
    // set_size takes the content size; the position is the frame's top left.
    w.set_size(LogicalSize::new(fit.w - chrome.w, fit.h - chrome.h))?;
    w.set_position(LogicalPosition::new(fit.x, fit.y))?;
    Ok(())
}

fn label_of(id: &str) -> &'static str {
    ITEMS.iter().find(|(i, _, _)| *i == id).map(|(_, l, _)| *l).unwrap_or("")
}

fn accel_of(id: &str) -> Option<&'static str> {
    ITEMS.iter().find(|(i, _, _)| *i == id).and_then(|(_, _, a)| *a)
}

/// Puts the main window back where it was, builds the menu bar, sets it on the app, and starts
/// handling its events.
pub fn install(app: &mut App) -> tauri::Result<()> {
    let handle = app.handle().clone();
    restore_main(&handle);
    let (menu, handles) = build(&handle)?;
    app.set_menu(menu)?;
    app.manage(MenuBar {
        handles: Mutex::new(Some(handles)),
        last_state: Mutex::new(None),
        last_windows: Mutex::new(None),
        inner_windows: Mutex::new(None),
    });
    app.on_menu_event(|app, event| handle_event(app, event.id().as_ref()));
    Ok(())
}

fn build(app: &AppHandle) -> tauri::Result<(Menu<Wry>, Handles)> {
    let mut items: HashMap<String, MenuItem<Wry>> = HashMap::new();
    let mut item = |id: &str| -> tauri::Result<MenuItem<Wry>> {
        let it = MenuItem::with_id(app, id, label_of(id), true, accel_of(id))?;
        items.insert(id.to_string(), it.clone());
        Ok(it)
    };
    let sep = || PredefinedMenuItem::separator(app);

    let about = PredefinedMenuItem::about(
        app,
        Some("About SparkBox"),
        Some(AboutMetadata {
            name: Some("SparkBox".into()),
            version: Some(app.package_info().version.to_string()),
            comments: Some("Monitor and run a cluster of NVIDIA DGX Sparks from your Mac.".into()),
            ..Default::default()
        }),
    )?;
    let app_menu = Submenu::with_items(
        app,
        "SparkBox",
        true,
        &[
            &about,
            &sep()?,
            &item("app:settings")?,
            &sep()?,
            &PredefinedMenuItem::services(app, None)?,
            &sep()?,
            &PredefinedMenuItem::hide(app, Some("Hide SparkBox"))?,
            &PredefinedMenuItem::hide_others(app, None)?,
            &PredefinedMenuItem::show_all(app, None)?,
            &sep()?,
            &PredefinedMenuItem::quit(app, Some("Quit SparkBox"))?,
        ],
    )?;

    let file = Submenu::with_items(
        app,
        "File",
        true,
        &[
            &item("term:new-tab")?,
            &item("files:open")?,
            &sep()?,
            &item("wm:close")?,
            &item("wm:close-all")?,
            &item("wm:reopen")?,
        ],
    )?;

    // The predefined items send the standard selectors, so they work in text fields, the editor and terminals.
    let edit = Submenu::with_items(
        app,
        "Edit",
        true,
        &[
            &PredefinedMenuItem::undo(app, None)?,
            &PredefinedMenuItem::redo(app, None)?,
            &sep()?,
            &PredefinedMenuItem::cut(app, None)?,
            &PredefinedMenuItem::copy(app, None)?,
            &PredefinedMenuItem::paste(app, None)?,
            &PredefinedMenuItem::select_all(app, None)?,
            &sep()?,
            &item("wm:find")?,
        ],
    )?;

    let controls = CheckMenuItem::with_id(app, CONTROLS, "WinBox-Style Window Controls", true, false, None::<&str>)?;
    let tile_wide = MenuItem::with_id(app, TILE_WIDE, "Hide Sidebar and Tile All", true, None::<&str>)?;
    let view = Submenu::with_items(
        app,
        "View",
        true,
        &[
            &item("app:palette")?,
            &item("app:sidebar")?,
            &sep()?,
            &item("wm:tile")?,
            &tile_wide,
            &item("wm:tile-two")?,
            &item("wm:tile-four")?,
            &item("wm:cascade")?,
            &item("wm:restore-layout")?,
            &sep()?,
            &item("wm:snap-left")?,
            &item("wm:snap-right")?,
            &item("wm:fill")?,
            &item("wm:restore")?,
            &item("wm:overview")?,
            &item("wm:popout")?,
            &sep()?,
            &item("wm:refresh")?,
            &sep()?,
            &item("app:zoom-reset")?,
            &item("app:zoom-in")?,
            &item("app:zoom-out")?,
            &sep()?,
            &controls,
            &PredefinedMenuItem::fullscreen(app, Some("Enter Full Screen"))?,
        ],
    )?;

    let nodes = Submenu::with_id(app, "go:nodes", "Nodes", true)?;
    let go = Submenu::with_items(
        app,
        "Go",
        true,
        &[
            &item("go:dashboard")?,
            &item("go:topology")?,
            &item("go:containers")?,
            &item("go:workloads")?,
            &item("go:models")?,
            &item("go:howto")?,
            &item("go:terminal")?,
            &item("go:rdp")?,
            &item("go:files")?,
            &item("go:activity")?,
            &item("go:alerts")?,
            &sep()?,
            &nodes,
        ],
    )?;

    let demo = CheckMenuItem::with_id(app, SOURCE_DEMO, "Demo (Simulated Cluster)", true, true, None::<&str>)?;
    let live = CheckMenuItem::with_id(app, SOURCE_LIVE, "Live (Your Sparks over SSH)", true, false, None::<&str>)?;
    let source = Submenu::with_items(app, "Data Source", true, &[&demo, &live])?;
    let scenarios = Submenu::with_id(app, "cluster:scenarios", "Demo Scenarios", true)?;
    let cluster = Submenu::with_items(
        app,
        "Cluster",
        true,
        &[&item("cluster:admin")?, &sep()?, &source, &scenarios, &sep()?, &item("go:tasks")?],
    )?;

    // Window: inner-window commands, then the live list of inner windows.
    let layouts = Submenu::with_id(app, "wm:layouts", "Layouts", false)?;
    let fixed: Vec<Box<dyn IsMenuItem<Wry>>> = vec![
        Box::new(item("wm:minimize")?),
        Box::new(item("app:minimize")?),
        Box::new(PredefinedMenuItem::maximize(app, Some("Zoom"))?),
        Box::new(sep()?),
        Box::new(item("wm:next")?),
        Box::new(item("wm:prev")?),
        Box::new(sep()?),
        Box::new(item("wm:save-layout")?),
        Box::new(layouts.clone()),
        Box::new(sep()?),
    ];
    let list_at = fixed.len();
    let refs: Vec<&dyn IsMenuItem<Wry>> = fixed.iter().map(|b| b.as_ref()).collect();
    let window_menu = Submenu::with_items(app, "Window", true, &refs)?;
    let none = MenuItem::with_id(app, WIN_PREFIX, "No Windows Open", false, None::<&str>)?;
    window_menu.append(&none)?;
    window_menu.append(&sep()?)?;
    window_menu.append(&item("wm:front")?)?;

    let help = Submenu::new(app, "Help", true)?;
    fill_help(app, &help, &help_entries(), &mut item, &mut HashMap::new())?;
    help.set_as_help_menu_for_nsapp()?;

    let menu = Menu::with_items(app, &[&app_menu, &file, &edit, &view, &go, &cluster, &window_menu, &help])?;
    let handles = Handles {
        items,
        demo,
        live,
        controls,
        window_menu,
        list_at,
        list: vec![MenuItemKind::MenuItem(none)],
        nodes,
        scenarios,
        layouts,
    };
    Ok((menu, handles))
}

/// Adds the manifest's entries to a Help menu or one of its submenus.
fn fill_help(
    app: &AppHandle,
    menu: &Submenu<Wry>,
    entries: &[HelpEntry],
    item: &mut dyn FnMut(&str) -> tauri::Result<MenuItem<Wry>>,
    seen: &mut HashMap<String, usize>,
) -> tauri::Result<()> {
    for e in entries {
        if e.separator {
            menu.append(&PredefinedMenuItem::separator(app)?)?;
        } else if let Some(sub) = &e.submenu {
            let s = Submenu::new(app, &e.label, true)?;
            fill_help(app, &s, sub, item, seen)?;
            menu.append(&s)?;
        } else if let Some(article) = &e.article {
            let times = seen.entry(article.clone()).or_default();
            *times += 1;
            let id = format!("{HELP_PREFIX}{article}{HELP_REPEAT}{times}");
            menu.append(&MenuItem::with_id(app, id, &e.label, true, None::<&str>)?)?;
        } else if let Some(section) = &e.section {
            menu.append(&MenuItem::with_id(app, format!("{HELP_SECTION_PREFIX}{section}"), &e.label, true, None::<&str>)?)?;
        } else {
            menu.append(&item(e.command.as_deref().unwrap_or(HELP_OPEN))?)?;
        }
    }
    Ok(())
}

/// Pop-out windows (src/lib/popout.ts) are labelled "pop-<kind>-<n>".
const POPOUT_PREFIX: &str = "pop-";

/// The pop-out window in front, if one is: the menu bar then acts on it, not on the desk.
fn focused_popout(app: &AppHandle) -> Option<tauri::WebviewWindow> {
    app.webview_windows().into_values().find(|w| w.label().starts_with(POPOUT_PREFIX) && w.is_focused().unwrap_or(false))
}

/// What a menu click does: most become `menu://<id>` for the page; a few are handled here.
fn handle_event(app: &AppHandle, id: &str) {
    let inner = *app.state::<MenuBar>().inner_windows.lock().unwrap();
    let popout = focused_popout(app);
    match route(id, inner, popout.is_some()) {
        Route::Page(name, payload) => {
            // From a pop-out, the desk comes forward to do it (⌘K, ⌘1…). Help too: SparkBox's
            // window may be hidden in the menu bar, and Help opens inside it.
            if popout.is_some() || name.starts_with("menu://help") {
                if let Some(w) = app.get_webview_window("main") {
                    let _ = w.show();
                    let _ = w.set_focus();
                }
            }
            let _ = app.emit(&name, payload);
        }
        Route::MinimizeMain => {
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.minimize();
            }
        }
        Route::CloseMain => close_main(app),
        Route::ClosePopout => {
            if let Some(w) = popout {
                let _ = w.close();
            }
        }
        Route::MinimizePopout => {
            if let Some(w) = popout {
                let _ = w.minimize();
            }
        }
        Route::Ignore => {}
    }
}

#[derive(Debug, PartialEq)]
enum Route {
    Page(String, Option<String>),
    MinimizeMain,
    CloseMain,
    /// ⌘W in a pop-out closes it, like any Mac window.
    ClosePopout,
    MinimizePopout,
    Ignore,
}

/// Decides where a click goes. `inner` is how many inner windows the page last reported;
/// `popout` is whether a pop-out window is in front.
fn route(id: &str, inner: Option<usize>, popout: bool) -> Route {
    if popout {
        match id {
            "wm:close" => return Route::ClosePopout,
            "wm:minimize" => return Route::MinimizePopout,
            _ => {}
        }
    }
    let page = |name: &str, payload: &str| Route::Page(name.to_string(), Some(payload.to_string()));
    if let Some(rest) = id.strip_prefix(WIN_PREFIX) {
        return if rest.is_empty() { Route::Ignore } else { page("menu://window", rest) };
    }
    if let Some(rest) = id.strip_prefix(NODE_PREFIX) {
        return page("menu://node", rest);
    }
    if let Some(rest) = id.strip_prefix(SCENARIO_PREFIX) {
        return page("menu://scenario", rest);
    }
    if let Some(rest) = id.strip_prefix(LAYOUT_PREFIX) {
        return page("menu://layout", rest);
    }
    if let Some(rest) = id.strip_prefix(HELP_PREFIX) {
        return page("menu://help", rest.split(HELP_REPEAT).next().unwrap_or(rest));
    }
    if let Some(rest) = id.strip_prefix(HELP_SECTION_PREFIX) {
        return page("menu://help-section", rest);
    }
    match id {
        "app:minimize" => Route::MinimizeMain,
        // ⌘W with nothing open inside closes SparkBox's own window (it stays in the menu bar).
        "wm:close" if inner == Some(0) => Route::CloseMain,
        SOURCE_DEMO | SOURCE_LIVE | CONTROLS | TILE_WIDE => Route::Page(format!("menu://{id}"), None),
        _ if ITEMS.iter().any(|(i, _, _)| *i == id) => Route::Page(format!("menu://{id}"), None),
        // Ids from other menus (the menu bar icon's) aren't ours.
        _ => Route::Ignore,
    }
}

/// Same as clicking the red button: the close handler in lib.rs hides SparkBox to the menu bar.
fn close_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.close();
    }
}

/// The page's inner windows, for the Window menu's list (with a checkmark on the active one).
#[tauri::command]
pub fn menu_set_windows(app: AppHandle, windows: Vec<WindowEntry>) -> Result<(), String> {
    let bar = app.state::<MenuBar>();
    *bar.inner_windows.lock().unwrap() = Some(windows.len());
    {
        let mut last = bar.last_windows.lock().unwrap();
        if last.as_ref() == Some(&windows) {
            return Ok(());
        }
        *last = Some(windows.clone());
    }
    let mut guard = bar.handles.lock().unwrap();
    let Some(h) = guard.as_mut() else { return Ok(()) };
    for old in h.list.drain(..) {
        h.window_menu.remove(&old).map_err(|e| e.to_string())?;
    }
    let mut fresh: Vec<MenuItemKind<Wry>> = Vec::new();
    if windows.is_empty() {
        fresh.push(MenuItemKind::MenuItem(
            MenuItem::with_id(&app, WIN_PREFIX, "No Windows Open", false, None::<&str>).map_err(|e| e.to_string())?,
        ));
    }
    for w in &windows {
        let title = if w.minimized { format!("{} (minimized)", w.title) } else { w.title.clone() };
        let item = CheckMenuItem::with_id(&app, format!("{WIN_PREFIX}{}", w.id), title, true, w.active, None::<&str>)
            .map_err(|e| e.to_string())?;
        fresh.push(MenuItemKind::Check(item));
    }
    for (i, item) in fresh.iter().enumerate() {
        h.window_menu.insert(item, h.list_at + i).map_err(|e| e.to_string())?;
    }
    h.list = fresh;
    Ok(())
}

/// Labels, checkmarks and the data-driven submenus: Admin, Data Source, Nodes, Demo Scenarios, Layouts.
#[tauri::command]
pub fn menu_set_state(app: AppHandle, state: MenuState) -> Result<(), String> {
    let bar = app.state::<MenuBar>();
    let previous = {
        let mut last = bar.last_state.lock().unwrap();
        if last.as_ref() == Some(&state) {
            return Ok(());
        }
        last.replace(state.clone())
    };
    let guard = bar.handles.lock().unwrap();
    let Some(h) = guard.as_ref() else { return Ok(()) };
    let e = |r: tauri::Result<()>| r.map_err(|e| e.to_string());

    if let Some(admin) = h.items.get("cluster:admin") {
        e(admin.set_text(if state.admin { "Lock Admin Mode" } else { "Unlock Admin Mode…" }))?;
    }
    e(h.demo.set_checked(state.demo))?;
    e(h.live.set_checked(!state.demo))?;
    e(h.controls.set_checked(state.controls.as_deref() == Some("winbox")))?;
    for (id, item) in &h.items {
        e(item.set_enabled(!state.disabled.contains(id)))?;
    }

    let changed = |f: fn(&MenuState) -> Vec<String>| previous.as_ref().is_none_or(|p| f(p) != f(&state));
    if changed(|s| s.nodes.iter().map(|n| format!("{}={}", n.id, n.name)).collect()) {
        let items: Vec<MenuItemKind<Wry>> = state
            .nodes
            .iter()
            .map(|n| MenuItem::with_id(&app, format!("{NODE_PREFIX}{}", n.id), &n.name, true, None::<&str>).map(MenuItemKind::MenuItem))
            .collect::<tauri::Result<_>>()
            .map_err(|e| e.to_string())?;
        replace_all(&h.nodes, &items)?;
        e(h.nodes.set_enabled(!items.is_empty()))?;
    }
    if changed(|s| {
        let mut v: Vec<String> = s.scenarios.iter().map(|x| format!("{}={}", x.id, x.title)).collect();
        v.push(format!("{}|{}", s.demo, s.scenario.clone().unwrap_or_default()));
        v
    }) {
        let current = state.scenario.clone().unwrap_or_default();
        // The everyday world is "default" when the simulator lists it, else no scenario at all.
        let has_default = state.scenarios.iter().any(|s| s.id == "default");
        let mut items: Vec<MenuItemKind<Wry>> = Vec::new();
        if !state.scenarios.is_empty() && !has_default {
            items.push(MenuItemKind::Check(
                CheckMenuItem::with_id(&app, SCENARIO_PREFIX, "None (Everything Healthy)", true, current.is_empty(), None::<&str>)
                    .map_err(|e| e.to_string())?,
            ));
            items.push(MenuItemKind::Predefined(PredefinedMenuItem::separator(&app).map_err(|e| e.to_string())?));
        }
        for s in &state.scenarios {
            items.push(MenuItemKind::Check(
                CheckMenuItem::with_id(&app, format!("{SCENARIO_PREFIX}{}", s.id), &s.title, true, playing(&current, &s.id), None::<&str>)
                    .map_err(|e| e.to_string())?,
            ));
        }
        replace_all(&h.scenarios, &items)?;
        e(h.scenarios.set_enabled(state.demo && !state.scenarios.is_empty()))?;
    }
    if changed(|s| s.layouts.clone()) {
        let items: Vec<MenuItemKind<Wry>> = state
            .layouts
            .iter()
            .map(|n| MenuItem::with_id(&app, format!("{LAYOUT_PREFIX}{n}"), n, true, None::<&str>).map(MenuItemKind::MenuItem))
            .collect::<tauri::Result<_>>()
            .map_err(|e| e.to_string())?;
        replace_all(&h.layouts, &items)?;
        e(h.layouts.set_enabled(!items.is_empty()))?;
    }
    Ok(())
}

/// Whether the scenario `id` is the one playing (`current` is empty for the everyday world).
fn playing(current: &str, id: &str) -> bool {
    current == id || (current.is_empty() && id == "default")
}

fn replace_all(menu: &Submenu<Wry>, items: &[MenuItemKind<Wry>]) -> Result<(), String> {
    for old in menu.items().map_err(|e| e.to_string())? {
        menu.remove(&old).map_err(|e| e.to_string())?;
    }
    for item in items {
        menu.append(item).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// ⌘W with no inner windows: close (hide) the app window, as File › Close Window does.
#[tauri::command]
pub fn menu_close_main(app: AppHandle) {
    close_main(&app);
}

/// Window › Minimize SparkBox from the palette.
#[tauri::command]
pub fn menu_minimize_main(app: AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.minimize();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique_and_events_are_valid_names() {
        let mut seen = std::collections::HashSet::new();
        for (id, label, _) in ITEMS {
            assert!(seen.insert(*id), "duplicate menu id {id}");
            assert!(!label.is_empty());
            let event = format!("menu://{id}");
            // Tauri accepts alphanumerics and - / : _ in event names.
            assert!(event.chars().all(|c| c.is_alphanumeric() || "-/:_".contains(c)), "{event}");
        }
    }

    #[test]
    fn accelerators_use_the_menu_syntax() {
        let mods = ["CmdOrCtrl", "Ctrl", "Alt", "Shift"];
        let keys = ["`", ",", "/", "=", "-", "Left", "Right", "Up", "Down"];
        let mut used = std::collections::HashSet::new();
        for (id, _, accel) in ITEMS {
            let Some(accel) = accel else { continue };
            assert!(used.insert(*accel), "{accel} is used twice");
            let (m, key) = accel.rsplit_once('+').expect(accel);
            assert!(m.split('+').all(|x| mods.contains(&x)), "{id}: {accel}");
            let ok = keys.contains(&key) || (key.len() == 1 && key.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit()));
            assert!(ok, "{id}: {accel}");
        }
    }

    #[test]
    fn clicks_route_to_the_page_or_the_window() {
        assert_eq!(route("go:howto", Some(1), false), Route::Page("menu://go:howto".into(), None));
        assert_eq!(route("wm:tile", Some(2), false), Route::Page("menu://wm:tile".into(), None));
        assert_eq!(route("wm:close", Some(3), false), Route::Page("menu://wm:close".into(), None));
        // Nothing open inside: ⌘W closes the app window; before the page reports, the page decides.
        assert_eq!(route("wm:close", Some(0), false), Route::CloseMain);
        assert_eq!(route("wm:close", None, false), Route::Page("menu://wm:close".into(), None));
        assert_eq!(route("app:minimize", Some(1), false), Route::MinimizeMain);
        assert_eq!(route("win:node?id=cell1", Some(1), false), Route::Page("menu://window".into(), Some("node?id=cell1".into())));
        assert_eq!(route("win:", Some(0), false), Route::Ignore);
        assert_eq!(route("go-node:cell3", None, false), Route::Page("menu://node".into(), Some("cell3".into())));
        assert_eq!(route("scenario:", None, false), Route::Page("menu://scenario".into(), Some(String::new())));
        assert_eq!(route("scenario:disk-filling", None, false), Route::Page("menu://scenario".into(), Some("disk-filling".into())));
        assert_eq!(route("layout:Bring-up", None, false), Route::Page("menu://layout".into(), Some("Bring-up".into())));
        assert_eq!(route("cluster:source-live", None, false), Route::Page("menu://cluster:source-live".into(), None));
        assert_eq!(route("wm:tile-wide", Some(3), false), Route::Page("menu://wm:tile-wide".into(), None));
        // The menu bar icon's own items go to its handler, not here.
        assert_eq!(route("node:cell1", None, false), Route::Ignore);
        assert_eq!(route("quit", None, false), Route::Ignore);
    }

    #[test]
    fn help_items_open_their_article() {
        assert_eq!(route("help-article:download-a-model", Some(2), false), Route::Page("menu://help".into(), Some("download-a-model".into())));
        // From a pop-out too: the desk comes forward to show it.
        assert_eq!(route("help-article:glossary", Some(0), true), Route::Page("menu://help".into(), Some("glossary".into())));
        assert_eq!(route("help:open", Some(1), false), Route::Page("menu://help:open".into(), None));
        // An article listed twice has two ids, and both open it.
        assert_eq!(route("help-article:restart-a-node@2", Some(1), false), Route::Page("menu://help".into(), Some("restart-a-node".into())));
        assert_eq!(route("help-section:how-to", Some(1), false), Route::Page("menu://help-section".into(), Some("how-to".into())));
    }

    #[test]
    fn titles_become_menu_labels_in_title_case() {
        assert_eq!(menu_label("Load a model on one node"), "Load a Model on One Node");
        assert_eq!(menu_label("SSH, sudo and host keys"), "SSH, sudo and Host Keys");
        assert_eq!(menu_label("vLLM and the other engines"), "vLLM and the Other Engines");
        assert_eq!(menu_label("Use look-only mode"), "Use Look-Only Mode");
        assert_eq!(menu_label("The Fence"), "The Fence");
        assert_eq!(menu_label("Switch window: a CRS804 DDQ"), "Switch Window: A CRS804 DDQ");
    }

    /// Every article the manifest lists, by id.
    fn manifest_articles() -> std::collections::HashSet<String> {
        let m: serde_json::Value = serde_json::from_str(HELP_MANIFEST).unwrap();
        m["articles"].as_array().unwrap().iter().map(|a| a["id"].as_str().unwrap().to_string()).collect()
    }

    fn walk<'a>(entries: &'a [HelpEntry], out: &mut Vec<&'a HelpEntry>) {
        for e in entries {
            out.push(e);
            if let Some(sub) = &e.submenu {
                walk(sub, out);
            }
        }
    }

    #[test]
    fn the_help_menu_comes_from_the_manifest_and_every_item_leads_somewhere() {
        let entries = help_entries();
        assert!(entries.len() >= 10, "the Help menu didn't parse: {} entries", entries.len());
        let known = manifest_articles();
        let mut all = Vec::new();
        walk(&entries, &mut all);
        let mut articles = 0;
        for e in all {
            if e.separator {
                continue;
            }
            assert!(!e.label.is_empty());
            if let Some(a) = &e.article {
                assert!(known.contains(a), "{}: no article {a}", e.label);
                let event = format!("menu://help/{a}");
                assert!(event.chars().all(|c| c.is_alphanumeric() || "-/:_".contains(c)), "{event}");
                articles += 1;
            } else if let Some(c) = &e.command {
                assert!(ITEMS.iter().any(|(id, _, _)| id == c), "{}: no command {c}", e.label);
            } else if let Some(s) = &e.section {
                let m: serde_json::Value = serde_json::from_str(HELP_MANIFEST).unwrap();
                assert!(m["toc"].as_array().unwrap().iter().any(|t| t["id"] == s.as_str()), "{}: no section {s}", e.label);
            } else {
                assert!(e.submenu.is_some() || e.label == "SparkBox Help", "{}: leads nowhere", e.label);
            }
        }
        assert!(articles >= 40, "only {articles} articles in the Help menu");
    }

    #[test]
    fn every_article_is_in_the_help_menu() {
        let entries = help_entries();
        let mut all = Vec::new();
        walk(&entries, &mut all);
        let listed: std::collections::HashSet<String> = all.iter().filter_map(|e| e.article.clone()).collect();
        let missing: Vec<String> = manifest_articles().difference(&listed).cloned().collect();
        assert!(missing.is_empty(), "not in the Help menu: {missing:?}");
    }

    #[test]
    fn macos_help_search_finds_the_words_a_reader_types() {
        let entries = help_entries();
        let mut all = Vec::new();
        walk(&entries, &mut all);
        for word in ["fence", "token", "memory", "download", "restart", "vpn"] {
            assert!(all.iter().any(|e| e.label.to_lowercase().contains(word)), "no menu item has {word}");
        }
    }

    #[test]
    fn the_settings_sections_are_listed_by_their_own_names() {
        let entries = help_entries();
        let features = entries.iter().find(|e| e.label == "Feature Guide").unwrap().submenu.as_ref().unwrap();
        let settings = features.iter().find(|e| e.label == "Settings").unwrap().submenu.as_ref().unwrap();
        assert!(settings.len() >= 14, "{} items", settings.len());
        assert!(settings.iter().any(|e| e.label == "Fence" && e.article.as_deref() == Some("settings-fence")));
    }

    #[test]
    fn long_lists_end_with_their_whole_section() {
        let entries = help_entries();
        for (name, section) in [("How-To Guides", "how-to"), ("Feature Guide", "features"), ("Concepts", "concepts"), ("Reference", "reference"), ("Troubleshooting", "troubleshooting")] {
            let e = entries.iter().find(|e| e.label == name).unwrap();
            assert_eq!(e.submenu.as_ref().unwrap().last().unwrap().section.as_deref(), Some(section), "{name}");
        }
    }

    #[test]
    fn the_help_menu_has_what_the_owner_asked_for_in_order() {
        let labels: Vec<String> = help_entries().iter().filter(|e| !e.separator).map(|e| e.label.clone()).collect();
        let want = [
            "SparkBox Help", "Getting Started", "How-To Guides", "Feature Guide", "Concepts", "Reference", "Troubleshooting", "Glossary",
            "Keyboard Shortcuts", "What's Proven on Your Cluster", "What's Coming",
        ];
        assert_eq!(&labels[..want.len()], &want);
        let entries = help_entries();
        for name in ["How-To Guides", "Feature Guide", "Concepts", "Reference"] {
            let e = entries.iter().find(|e| e.label == name).unwrap();
            assert!(e.submenu.as_ref().is_some_and(|s| s.len() >= 6), "{name} has its submenu");
        }
        // ⌘? and ⌘/ stay what they are, and both have an item to live on.
        assert_eq!(accel_of(HELP_OPEN), Some("CmdOrCtrl+Shift+/"));
        assert!(entries.iter().any(|e| e.command.as_deref() == Some("app:shortcuts")));
        let manifest: serde_json::Value = serde_json::from_str(HELP_MANIFEST).unwrap();
        assert_eq!(manifest["helpMenu"][0]["accelerator"], "CmdOrCtrl+Shift+/");
    }

    /// Help › the Diagnostics item says what it does: it opens Settings, and runs nothing.
    #[test]
    fn the_diagnostics_item_reads_open_diagnostics_in_the_menu_and_the_manifest() {
        assert_eq!(label_of("app:doctor"), "Open Diagnostics…");
        let entry = help_entries().into_iter().find(|e| e.command.as_deref() == Some("app:doctor")).unwrap();
        assert_eq!(entry.label, label_of("app:doctor"));
    }

    #[test]
    fn a_popout_in_front_takes_close_and_minimize() {
        assert_eq!(route("wm:close", Some(3), true), Route::ClosePopout);
        assert_eq!(route("wm:close", Some(0), true), Route::ClosePopout);
        assert_eq!(route("wm:minimize", Some(2), true), Route::MinimizePopout);
        // Everything else still goes to the desk (which comes forward to do it).
        assert_eq!(route("app:palette", Some(2), true), Route::Page("menu://app:palette".into(), None));
        assert_eq!(route("wm:popout", Some(1), false), Route::Page("menu://wm:popout".into(), None));
    }

    #[test]
    fn the_everyday_world_is_checked_when_nothing_plays() {
        assert!(playing("", "default"));
        assert!(!playing("", "disk-filling"));
        assert!(playing("disk-filling", "disk-filling"));
        assert!(!playing("disk-filling", "default"));
    }

    /// The owner's display as the debug build read it (Monitor::work_area): 2560×1440 points
    /// less the 31 pt menu bar and the 50 pt Dock.
    const MAIN_AREA: Rect = Rect { x: 0.0, y: 31.0, w: 2560.0, h: 1359.0 };

    fn r(x: f64, y: f64, w: f64, h: f64) -> Rect {
        Rect { x, y, w, h }
    }

    #[test]
    fn a_window_on_screen_stays_where_it_was() {
        let saved = r(560.0, 261.0, 1440.0, 900.0);
        assert_eq!(clamp(saved, &[MAIN_AREA], MIN_SIZE), saved);
    }

    #[test]
    fn a_window_off_the_right_edge_comes_back_inside() {
        assert_eq!(clamp(r(2400.0, 261.0, 1440.0, 900.0), &[MAIN_AREA], MIN_SIZE), r(1120.0, 261.0, 1440.0, 900.0));
        // Entirely off to the right: no screen holds it, so the primary takes it.
        assert_eq!(clamp(r(5000.0, 300.0, 1440.0, 900.0), &[MAIN_AREA], MIN_SIZE), r(1120.0, 300.0, 1440.0, 900.0));
    }

    #[test]
    fn a_window_below_the_screen_comes_back_up() {
        assert_eq!(clamp(r(560.0, 1300.0, 1440.0, 900.0), &[MAIN_AREA], MIN_SIZE), r(560.0, 490.0, 1440.0, 900.0));
        // The acceptance check's file, 3000×2000 at (5000, 3000) in 2x pixels: what the debug build showed.
        assert_eq!(clamp(r(2500.0, 1500.0, 1500.0, 1000.0), &[MAIN_AREA], MIN_SIZE), r(1060.0, 390.0, 1500.0, 1000.0));
        assert_eq!(clamp(r(5000.0, 3000.0, 3000.0, 2000.0), &[MAIN_AREA], MIN_SIZE), MAIN_AREA);
    }

    #[test]
    fn a_window_larger_than_the_screen_shrinks_to_the_work_area() {
        assert_eq!(clamp(r(-100.0, 0.0, 2880.0, 1800.0), &[MAIN_AREA], MIN_SIZE), MAIN_AREA);
        // Only the dimension that doesn't fit shrinks.
        assert_eq!(clamp(r(100.0, 100.0, 1440.0, 1600.0), &[MAIN_AREA], MIN_SIZE), r(100.0, 31.0, 1440.0, 1359.0));
    }

    #[test]
    fn a_window_saved_on_a_screen_that_is_gone_goes_to_the_primary() {
        // Saved on a display left of the main one, unplugged since.
        assert_eq!(clamp(r(-1800.0, 200.0, 1440.0, 900.0), &[MAIN_AREA], MIN_SIZE), r(0.0, 200.0, 1440.0, 900.0));
        // With the display still there, the window stays on it.
        let left = r(-1920.0, 0.0, 1920.0, 1055.0);
        assert_eq!(clamp(r(-1800.0, 100.0, 1440.0, 900.0), &[MAIN_AREA, left], MIN_SIZE), r(-1800.0, 100.0, 1440.0, 900.0));
        // Straddling both, the screen holding its centre wins; then it moves fully onto it.
        assert_eq!(clamp(r(-400.0, 100.0, 1440.0, 900.0), &[MAIN_AREA, left], MIN_SIZE), r(0.0, 100.0, 1440.0, 900.0));
        // No screens reported at all: left as it was.
        assert_eq!(clamp(r(-1800.0, 200.0, 1440.0, 900.0), &[], MIN_SIZE), r(-1800.0, 200.0, 1440.0, 900.0));
    }

    #[test]
    fn the_largest_overlap_wins_when_the_centre_is_between_screens() {
        // Two screens side by side with a gap the centre falls into.
        let a = r(0.0, 25.0, 1000.0, 1000.0);
        let b = r(1200.0, 0.0, 1000.0, 1000.0);
        let saved = r(500.0, 100.0, 1000.0, 600.0);
        assert_eq!(clamp(saved, &[a, b], MIN_SIZE), r(0.0, 100.0, 1000.0, 600.0));
        let saved = r(650.0, 100.0, 1000.0, 600.0);
        assert_eq!(clamp(saved, &[a, b], MIN_SIZE), r(1200.0, 100.0, 1000.0, 600.0));
    }

    #[test]
    fn a_window_under_the_menu_bar_moves_below_it() {
        assert_eq!(clamp(r(560.0, 0.0, 1440.0, 900.0), &[MAIN_AREA], MIN_SIZE), r(560.0, 31.0, 1440.0, 900.0));
        assert_eq!(clamp(r(560.0, -500.0, 1440.0, 900.0), &[MAIN_AREA], MIN_SIZE), r(560.0, 31.0, 1440.0, 900.0));
    }

    #[test]
    fn never_smaller_than_the_minimum() {
        // Exactly the minimum is kept.
        let min = r(100.0, 100.0, 960.0, 600.0);
        assert_eq!(clamp(min, &[MAIN_AREA], MIN_SIZE), min);
        // A smaller saved frame grows to it.
        assert_eq!(clamp(r(100.0, 100.0, 500.0, 300.0), &[MAIN_AREA], MIN_SIZE), min);
        // A work area smaller than the minimum: the minimum, at the area's top left.
        let small = r(0.0, 25.0, 800.0, 575.0);
        assert_eq!(clamp(r(300.0, 300.0, 1440.0, 900.0), &[small], MIN_SIZE), r(0.0, 25.0, 960.0, 600.0));
    }

    #[test]
    fn the_saved_frame_is_read_in_logical_points() {
        let saved: HashMap<String, Saved> = serde_json::from_str(
            r#"{"main":{"width":2880,"height":1800,"x":1120,"y":522,"prev_x":1120,"prev_y":522,"maximized":false,"visible":true,"decorated":true,"fullscreen":true}}"#,
        )
        .unwrap();
        let s = saved["main"];
        assert!(!s.maximized, "full screen isn't restored, only zoom");
        // The owner's 5K display at 2x: the release file's 2880×1800 at (1120, 522) is 1440×900 at (560, 261).
        assert_eq!(saved_frame(&s, 2.0, Size { w: 0.0, h: 0.0 }), r(560.0, 261.0, 1440.0, 900.0));
        // A title bar (none with Overlay) adds to the saved content size.
        let zoomed = Saved { width: 2000, height: 1200, x: -40, y: 0, maximized: true };
        assert_eq!(saved_frame(&zoomed, 1.0, Size { w: 0.0, h: 28.0 }), r(-40.0, 0.0, 2000.0, 1228.0));
    }

    #[test]
    fn the_window_config_matches_what_the_code_assumes() {
        let conf: serde_json::Value = serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        // The release app's data lives under this identifier: it must never change.
        assert_eq!(conf["identifier"], "com.nemecj.sparkbox");
        let w = &conf["app"]["windows"][0];
        // The harness finds the window by its title.
        assert_eq!(w["title"], "SparkBox");
        assert_eq!(w["minWidth"].as_f64(), Some(MIN_SIZE.w));
        assert_eq!(w["minHeight"].as_f64(), Some(MIN_SIZE.h));
        assert_ne!(w["resizable"], false);
        assert_ne!(w["fullscreen"], true);
        assert_eq!(w["label"].as_str().unwrap_or(MAIN), MAIN);
    }

    #[test]
    fn menu_state_reads_what_the_page_sends() {
        let s: MenuState = serde_json::from_value(serde_json::json!({
            "admin": true, "demo": true, "scenario": "cell3-offline",
            "nodes": [{ "id": "cell1", "name": "cell1" }],
            "scenarios": [{ "id": "cell3-offline", "title": "cell3 offline" }],
            "layouts": ["Bring-up"], "disabled": ["go:alerts"], "controls": "winbox"
        }))
        .unwrap();
        assert!(s.admin && s.demo);
        assert_eq!(s.scenario.as_deref(), Some("cell3-offline"));
        assert_eq!(s.nodes[0].name, "cell1");
        assert_eq!(s.controls.as_deref(), Some("winbox"));
        let w: Vec<WindowEntry> = serde_json::from_value(serde_json::json!([{ "id": "files", "title": "Files", "active": true, "minimized": false }])).unwrap();
        assert!(w[0].active);
    }
}
