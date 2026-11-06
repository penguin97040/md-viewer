//! Viewer preferences, stored as `md-viewer/settings.json` in the user's config folder.
//!
//! The file name and the keys are those of version 0.2.6, so settings carry over. Anything missing,
//! of the wrong type or out of range falls back to its default, and writes replace the file whole.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use mdv_theme::Mode;

use crate::view::Settings as Layout;

/// Everything that is saved: the layout settings, the theme and whether the outline is open.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Prefs {
    pub layout: Layout,
    pub mode: Mode,
    pub toc: bool,
}

impl Default for Prefs {
    fn default() -> Self {
        Self { layout: Layout::DEFAULTS, mode: Mode::Dark, toc: true }
    }
}

impl Prefs {
    /// Clamps every value into its allowed range, as 0.2.6's `Clean` does.
    pub fn clean(mut self) -> Self {
        let d = Layout::DEFAULTS;
        self.layout.font_size = self.layout.font_size.round().clamp(12.0, 28.0);
        let lh = self.layout.line_height;
        self.layout.line_height = if lh.is_finite() && (1.2..=2.2).contains(&lh) { lh } else { d.line_height };
        self.layout.padding = self.layout.padding.round().clamp(20.0, 40.0);
        self
    }

    /// Reads the JSON text of a settings file. Never fails: a field that is not usable keeps its
    /// default, and text that is not a JSON object gives the defaults.
    pub fn parse(text: &str) -> Self {
        let mut p = Self::default();
        let Ok(serde_json::Value::Object(o)) = serde_json::from_str::<serde_json::Value>(text) else { return p };
        match o.get("theme").and_then(|v| v.as_str()) {
            Some("light") => p.mode = Mode::Light,
            Some("dark") => p.mode = Mode::Dark,
            _ => {}
        }
        match o.get("font").and_then(|v| v.as_str()) {
            Some("serif") => p.layout.serif = true,
            Some("sans") => p.layout.serif = false,
            _ => {}
        }
        let flag = |key: &str| o.get(key).and_then(|v| v.as_bool());
        p.layout.wrap = flag("wrap").unwrap_or(p.layout.wrap);
        p.layout.live_reload = flag("liveReload").unwrap_or(p.layout.live_reload);
        p.toc = flag("toc").unwrap_or(p.toc);
        // Whole numbers only, as in 0.2.6 (a Go int); anything else is the default.
        let whole = |key: &str| o.get(key).and_then(|v| v.as_i64());
        if let Some(n) = whole("fontSize") {
            p.layout.font_size = n.clamp(12, 28) as f32;
        }
        if let Some(n) = whole("padding") {
            p.layout.padding = n.clamp(20, 40) as f32;
        }
        if let Some(n) = o.get("lineHeight").and_then(|v| v.as_f64()) {
            p.layout.line_height = n as f32;
        }
        p.clean()
    }

    /// The text of the settings file, with 0.2.6's key order and two-space indent.
    pub fn to_json(self) -> String {
        let p = self.clean();
        let line_height = (f64::from(p.layout.line_height) * 100.0).round() / 100.0;
        format!(
            "{{\n  \"theme\": \"{}\",\n  \"wrap\": {},\n  \"fontSize\": {},\n  \"lineHeight\": {},\n  \"padding\": {},\n  \"font\": \"{}\",\n  \"liveReload\": {},\n  \"toc\": {}\n}}",
            if p.mode == Mode::Light { "light" } else { "dark" },
            p.layout.wrap,
            p.layout.font_size as i32,
            serde_json::Number::from_f64(line_height).map_or("1.65".to_string(), |n| n.to_string()),
            p.layout.padding as i32,
            if p.layout.serif { "serif" } else { "sans" },
            p.layout.live_reload,
            p.toc,
        )
    }
}

/// The user's config folder: `%AppData%`, `~/Library/Application Support`, or `$XDG_CONFIG_HOME`
/// (when absolute) falling back to `~/.config`.
fn config_dir() -> Option<PathBuf> {
    let var = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty()).map(PathBuf::from);
    if cfg!(windows) {
        var("APPDATA")
    } else if cfg!(target_os = "macos") {
        var("HOME").map(|h| h.join("Library").join("Application Support"))
    } else {
        var("XDG_CONFIG_HOME").filter(|d| d.is_absolute()).or_else(|| var("HOME").map(|h| h.join(".config")))
    }
}

fn path() -> Option<PathBuf> {
    config_dir().map(|d| d.join("md-viewer").join("settings.json"))
}

/// The saved preferences, or the defaults.
pub fn load() -> Prefs {
    path().map_or_else(Prefs::default, |p| load_from(&p))
}

fn load_from(path: &Path) -> Prefs {
    // A settings file is a few hundred bytes. Reading no more than 64 KiB keeps a hostile one cheap.
    use std::io::Read;
    let mut text = Vec::new();
    let read = std::fs::File::open(path).and_then(|f| f.take(64 * 1024).read_to_end(&mut text));
    if read.is_err() {
        return Prefs::default();
    }
    Prefs::parse(&String::from_utf8_lossy(&text))
}

// ---- shared by every window ---------------------------------------------------------------------

/// The preferences as the windows last agreed them, and whether they still need writing.
struct Shared {
    prefs: Prefs,
    unsaved: bool,
    generation: u64,
}

impl gpui::Global for Shared {}

/// Loads the saved preferences and makes sure they are written on the way out.
pub fn install(cx: &mut gpui::App) {
    cx.set_global(Shared { prefs: load(), unsaved: false, generation: 0 });
    cx.on_app_quit(|cx| {
        let pending = cx.try_global::<Shared>().filter(|s| s.unsaved).map(|s| s.prefs);
        async move {
            if let Some(p) = pending {
                if let Err(e) = save(p) {
                    eprintln!("md-viewer: could not save settings: {e}");
                }
            }
        }
    })
    .detach();
}

/// What a new window starts with.
pub fn current(cx: &gpui::App) -> Prefs {
    cx.try_global::<Shared>().map_or_else(Prefs::default, |s| s.prefs)
}

/// A window changed a setting: every other window follows, and the file is written shortly after
/// the last change (a mouse wheel makes many).
pub fn publish(source: &gpui::Entity<crate::view::AppView>, prefs: Prefs, cx: &mut gpui::App) {
    let prefs = prefs.clean();
    let changed = cx.try_global::<Shared>().is_none_or(|s| s.prefs != prefs);
    if !changed {
        return;
    }
    let generation = {
        let shared = cx.global_mut::<Shared>();
        shared.prefs = prefs;
        shared.unsaved = true;
        shared.generation += 1;
        shared.generation
    };
    for view in crate::tab_move::all_views(cx) {
        if view != *source {
            view.update(cx, |v, cx| v.apply_prefs(prefs, cx));
        }
    }
    source.update(cx, |v, _| v.sync_watches());
    cx.spawn(async move |cx| {
        cx.background_executor().timer(std::time::Duration::from_millis(250)).await;
        let latest = cx.update(|cx| {
            let shared = cx.global_mut::<Shared>();
            (shared.generation == generation).then(|| {
                shared.unsaved = false;
                shared.prefs
            })
        });
        if let Some(p) = latest {
            let result = cx.background_executor().spawn(async move { save(p) }).await;
            if let Err(e) = result {
                eprintln!("md-viewer: could not save settings: {e}");
            }
        }
    })
    .detach();
}

static SAVE: Mutex<()> = Mutex::new(());

/// Writes the preferences, replacing the file atomically. Writes are serialised.
pub fn save(prefs: Prefs) -> std::io::Result<()> {
    let path = path().ok_or_else(|| std::io::Error::other("no config folder"))?;
    save_to(&path, prefs)
}

fn save_to(path: &Path, prefs: Prefs) -> std::io::Result<()> {
    let _guard = SAVE.lock().unwrap_or_else(|e| e.into_inner());
    let dir = path.parent().ok_or_else(|| std::io::Error::other("no folder"))?;
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join(format!("settings-{}.tmp", std::process::id()));
    let result = std::fs::write(&tmp, prefs.to_json()).and_then(|()| std::fs::rename(&tmp, path));
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_the_old_app() {
        let p = Prefs::default();
        assert_eq!((p.layout.font_size, p.layout.line_height, p.layout.padding), (17.0, 1.65, 20.0));
        assert!(p.layout.wrap && p.layout.live_reload && p.toc && !p.layout.serif && p.mode == Mode::Dark);
    }

    #[test]
    fn reads_a_0_2_6_file() {
        let p = Prefs::parse(r#"{"theme":"light","wrap":false,"fontSize":20,"lineHeight":1.8,"padding":32,"font":"serif","liveReload":false,"toc":false}"#);
        assert_eq!(p.mode, Mode::Light);
        assert_eq!((p.layout.font_size, p.layout.line_height, p.layout.padding), (20.0, 1.8, 32.0));
        assert!(p.layout.serif && !p.layout.wrap && !p.layout.live_reload && !p.toc);
    }

    #[test]
    fn bad_values_fall_back() {
        let d = Prefs::default();
        for text in ["", "not json", "[]", "null", "{\"theme\": 3, \"fontSize\": \"big\"}"] {
            assert_eq!(Prefs::parse(text), d, "{text}");
        }
        let p = Prefs::parse(r#"{"theme":"blue","font":"mono","fontSize":99,"padding":-4,"lineHeight":9,"wrap":"yes"}"#);
        assert_eq!(p.mode, Mode::Dark);
        assert!(!p.layout.serif && p.layout.wrap);
        assert_eq!((p.layout.font_size, p.layout.padding, p.layout.line_height), (28.0, 20.0, 1.65));
        assert_eq!(Prefs::parse(r#"{"fontSize":17.5,"lineHeight":1.2}"#).layout.font_size, 17.0);
        assert_eq!(Prefs::parse(r#"{"lineHeight":1.19}"#).layout.line_height, 1.65);
    }

    #[test]
    fn json_round_trips_with_the_old_keys() {
        let mut p = Prefs::default();
        p.layout.line_height = 1.75;
        p.layout.serif = true;
        p.mode = Mode::Light;
        let text = p.to_json();
        assert!(text.starts_with("{\n  \"theme\": \"light\",\n  \"wrap\": true,\n  \"fontSize\": 17,\n  \"lineHeight\": 1.75,"), "{text}");
        assert_eq!(Prefs::parse(&text), p);
    }

    #[test]
    fn saves_atomically_and_reloads() {
        let dir = std::env::temp_dir().join(format!("mdv-settings-{}", std::process::id()));
        let file = dir.join("md-viewer").join("settings.json");
        assert_eq!(load_from(&file), Prefs::default());
        let mut p = Prefs::default();
        p.layout.font_size = 22.0;
        p.toc = false;
        save_to(&file, p).unwrap();
        assert_eq!(load_from(&file), p);
        let left: Vec<_> = std::fs::read_dir(file.parent().unwrap()).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(left.len(), 1, "temporary file left behind: {left:?}");
        std::fs::write(&file, "{ truncated").unwrap();
        assert_eq!(load_from(&file), Prefs::default());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
