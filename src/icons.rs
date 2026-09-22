use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};

static ICON_CACHE: LazyLock<Mutex<HashMap<String, Option<String>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

static DESKTOP_ICON_INDEX: LazyLock<HashMap<String, String>> =
    LazyLock::new(build_desktop_icon_index);

fn get_active_theme() -> String {
    if let Ok(theme) = std::env::var("ICON_THEME") {
        if !theme.is_empty() {
            return theme;
        }
    }

    // Standard XDG config checks: ~/.config/gtk-3.0/settings.ini, ~/.config/gtk-4.0/settings.ini, etc.
    let mut config_paths = Vec::new();
    if let Ok(config_home) = std::env::var("XDG_CONFIG_HOME") {
        config_paths.push(format!("{}/gtk-3.0/settings.ini", config_home));
        config_paths.push(format!("{}/gtk-4.0/settings.ini", config_home));
    }
    if let Ok(home) = std::env::var("HOME") {
        config_paths.push(format!("{}/.config/gtk-3.0/settings.ini", home));
        config_paths.push(format!("{}/.config/gtk-4.0/settings.ini", home));
    }
    config_paths.push("/etc/gtk-3.0/settings.ini".to_string());
    config_paths.push("/etc/gtk-4.0/settings.ini".to_string());

    for config_path in &config_paths {
        if let Ok(content) = std::fs::read_to_string(config_path) {
            for line in content.lines() {
                if let Some((k, v)) = line.split_once('=') {
                    if k.trim() == "gtk-icon-theme-name" {
                        let val = v.trim().trim_matches('\'').trim_matches('"');
                        if !val.is_empty() {
                            return val.to_string();
                        }
                    }
                }
            }
        }
    }

    "hicolor".to_string()
}

fn resolve_theme_chain(active: &str, theme_roots: &[String]) -> Vec<String> {
    let mut chain = Vec::new();
    let mut visited = std::collections::HashSet::new();

    fn recurse(
        theme: &str,
        theme_roots: &[String],
        chain: &mut Vec<String>,
        visited: &mut std::collections::HashSet<String>,
    ) {
        if theme.is_empty() || !visited.insert(theme.to_string()) {
            return;
        }
        chain.push(theme.to_string());

        for root in theme_roots {
            let index_path = Path::new(root).join(theme).join("index.theme");
            if let Ok(content) = std::fs::read_to_string(&index_path) {
                for line in content.lines() {
                    if let Some(inherits) = line.strip_prefix("Inherits=") {
                        for parent in inherits.split(',') {
                            let p = parent.trim();
                            if !p.is_empty() {
                                recurse(p, theme_roots, chain, visited);
                            }
                        }
                    }
                }
                break;
            }
        }
    }

    recurse(active, theme_roots, &mut chain, &mut visited);
    for fallback in &[
        "Adwaita",
        "breeze",
        "AdwaitaLegacy",
        "Papirus",
        "elementary",
    ] {
        if !visited.contains(*fallback) {
            recurse(fallback, theme_roots, &mut chain, &mut visited);
        }
    }
    if !visited.contains("hicolor") {
        chain.push("hicolor".to_string());
    }
    chain
}

fn get_theme_directories(theme_dir: &Path) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let index_path = theme_dir.join("index.theme");
    if let Ok(content) = std::fs::read_to_string(&index_path) {
        for line in content.lines() {
            if let Some(raw_dirs) = line.strip_prefix("Directories=") {
                for dir in raw_dirs.split(',') {
                    let d = dir.trim();
                    if !d.is_empty() {
                        dirs.push(PathBuf::from(d));
                    }
                }
            }
        }
    }

    if dirs.is_empty() {
        fn collect_dirs(current: &Path, depth: usize, out: &mut Vec<PathBuf>, base: &Path) {
            if depth > 3 {
                return;
            }
            if let Ok(entries) = std::fs::read_dir(current) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_dir() {
                        if let Ok(rel) = path.strip_prefix(base) {
                            out.push(rel.to_path_buf());
                        }
                        collect_dirs(&path, depth + 1, out, base);
                    }
                }
            }
        }
        collect_dirs(theme_dir, 1, &mut dirs, theme_dir);
    }

    dirs.sort_by_key(|d| {
        let s = d.to_string_lossy();
        if s.starts_with("scalable") || s.contains("/scalable") {
            0
        } else if s.starts_with("48x48") || s.contains("/48x48") {
            1
        } else if s.starts_with("64x64") || s.contains("/64x64") {
            2
        } else if s.starts_with("128x128") || s.contains("/128x128") {
            3
        } else if s.starts_with("256x256") || s.contains("/256x256") {
            4
        } else if s.starts_with("512x512") || s.contains("/512x512") {
            5
        } else if s.starts_with("32x32") || s.contains("/32x32") {
            6
        } else if s.starts_with("24x24") || s.contains("/24x24") {
            7
        } else if s.starts_with("22x22") || s.contains("/22x22") {
            8
        } else if s.starts_with("16x16") || s.contains("/16x16") {
            9
        } else {
            10
        }
    });

    dirs
}

fn build_desktop_icon_index() -> HashMap<String, String> {
    let mut map = HashMap::new();
    let home = std::env::var("HOME").unwrap_or_default();
    let dirs = [
        format!("{}/.local/share/applications", home),
        format!("{}/.local/share/flatpak/exports/share/applications", home),
        "/var/lib/flatpak/exports/share/applications".to_string(),
        "/usr/local/share/applications".to_string(),
        "/usr/share/applications".to_string(),
    ];

    for dir_str in &dirs {
        let dir = Path::new(dir_str);
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(_) => continue,
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("desktop") {
                continue;
            }

            let file_stem = match path.file_stem().and_then(|s| s.to_str()) {
                Some(s) => s.to_string(),
                None => continue,
            };

            let content = match std::fs::read_to_string(&path) {
                Ok(c) => c,
                Err(_) => continue,
            };

            let mut icon = None;
            let mut exec = None;
            let mut wm_class = None;
            let mut app_name = None;
            let mut is_game_launch = false;

            for line in content.lines() {
                if let Some(val) = line.strip_prefix("Icon=") {
                    if icon.is_none() {
                        icon = Some(val.trim().to_string());
                    }
                } else if let Some(val) = line.strip_prefix("Exec=") {
                    if exec.is_none() {
                        let trimmed_exec = val.trim();
                        if trimmed_exec.contains("steam://") || trimmed_exec.contains("rungameid") {
                            is_game_launch = true;
                        }
                        let prog = trimmed_exec.split_whitespace().next().unwrap_or("");
                        let prog_name = Path::new(prog)
                            .file_name()
                            .and_then(|s| s.to_str())
                            .unwrap_or(prog);
                        exec = Some(prog_name.to_lowercase());
                    }
                } else if let Some(val) = line.strip_prefix("StartupWMClass=") {
                    if wm_class.is_none() {
                        wm_class = Some(val.trim().to_string());
                    }
                } else if let Some(val) = line.strip_prefix("Name=") {
                    if app_name.is_none() {
                        app_name = Some(val.trim().to_string());
                    }
                }
            }

            if let Some(icon_name) = icon {
                let lower_stem = file_stem.to_lowercase();
                map.entry(lower_stem.clone())
                    .or_insert_with(|| icon_name.clone());
                map.entry(file_stem.clone())
                    .or_insert_with(|| icon_name.clone());

                if let Some(last_part) = lower_stem.split('.').next_back() {
                    map.entry(last_part.to_string())
                        .or_insert_with(|| icon_name.clone());
                }

                if let Some(ref wmc) = wm_class {
                    map.entry(wmc.clone()).or_insert_with(|| icon_name.clone());
                    map.entry(wmc.to_lowercase())
                        .or_insert_with(|| icon_name.clone());
                }

                if let Some(ref name) = app_name {
                    let lower_name = name.to_lowercase();
                    map.entry(lower_name.clone())
                        .or_insert_with(|| icon_name.clone());
                    let hyphenated = lower_name.replace(' ', "-");
                    map.entry(hyphenated).or_insert_with(|| icon_name.clone());
                    let squashed = lower_name.replace(' ', "");
                    map.entry(squashed).or_insert_with(|| icon_name.clone());

                    // Generate acronym for game titles (e.g. "Counter-Strike 2" -> "cs2")
                    let words: Vec<&str> = name
                        .split(&[' ', '-', '_'][..])
                        .filter(|w| !w.is_empty())
                        .collect();
                    if words.len() >= 2 {
                        let mut acronym = String::new();
                        for w in words {
                            if let Some(first) = w.chars().next() {
                                acronym.push(first.to_ascii_lowercase());
                            }
                        }
                        if acronym.len() >= 2 {
                            map.entry(acronym).or_insert_with(|| icon_name.clone());
                        }
                    }
                }

                if let Some(exec_name) = exec {
                    if !is_game_launch {
                        map.entry(exec_name).or_insert_with(|| icon_name.clone());
                    }
                }
            }
        }
    }

    // Explicit fallback aliases for common apps
    map.entry("spotify".to_string())
        .or_insert_with(|| "spotify-launcher".to_string());
    map.entry("steam".to_string())
        .or_insert_with(|| "steam".to_string());
    map.entry("cs2".to_string())
        .or_insert_with(|| "steam_icon_730".to_string());

    map
}

pub fn resolve_icon(name: &str) -> Option<String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return None;
    }

    if (trimmed.starts_with('/') || trimmed.starts_with('~'))
        && (trimmed.ends_with(".svg") || trimmed.ends_with(".png") || trimmed.ends_with(".jpg"))
    {
        let expanded = if trimmed.starts_with('~') {
            if let Ok(home) = std::env::var("HOME") {
                PathBuf::from(trimmed.replacen('~', &home, 1))
            } else {
                PathBuf::from(trimmed)
            }
        } else {
            PathBuf::from(trimmed)
        };
        if expanded.is_file() {
            return Some(expanded.to_string_lossy().to_string());
        }
    }

    {
        let cache = ICON_CACHE.lock().unwrap();
        if let Some(cached) = cache.get(trimmed) {
            return cached.clone();
        }
    }

    let mut candidates = Vec::new();
    let mut seen = std::collections::HashSet::new();

    fn push_candidate(
        candidates: &mut Vec<String>,
        seen: &mut std::collections::HashSet<String>,
        cand: &str,
    ) {
        let t = cand.trim();
        if !t.is_empty() && seen.insert(t.to_string()) {
            candidates.push(t.to_string());
        }
    }

    push_candidate(&mut candidates, &mut seen, trimmed);
    push_candidate(&mut candidates, &mut seen, &trimmed.to_lowercase());

    if let Some(mapped) = DESKTOP_ICON_INDEX.get(&trimmed.to_lowercase()) {
        push_candidate(&mut candidates, &mut seen, mapped);
    }
    if let Some(mapped) = DESKTOP_ICON_INDEX.get(trimmed) {
        push_candidate(&mut candidates, &mut seen, mapped);
    }

    if trimmed.contains('.') {
        if let Some(last) = trimmed.split('.').next_back() {
            push_candidate(&mut candidates, &mut seen, last);
            push_candidate(&mut candidates, &mut seen, &last.to_lowercase());
            if let Some(mapped) = DESKTOP_ICON_INDEX.get(&last.to_lowercase()) {
                push_candidate(&mut candidates, &mut seen, mapped);
            }
        }
    }

    let cands_snapshot = candidates.clone();
    for cand in &cands_snapshot {
        if let Some(base) = cand.strip_suffix("-symbolic") {
            push_candidate(&mut candidates, &mut seen, base);
        } else {
            push_candidate(&mut candidates, &mut seen, &format!("{}-symbolic", cand));
        }
    }

    let active_theme = get_active_theme();
    let home = std::env::var("HOME").unwrap_or_default();
    let theme_roots = [
        format!("{}/.local/share/icons", home),
        format!("{}/.icons", home),
        format!("{}/.local/share/flatpak/exports/share/icons", home),
        "/var/lib/flatpak/exports/share/icons".to_string(),
        "/usr/local/share/icons".to_string(),
        "/usr/share/icons".to_string(),
    ];

    let themes = resolve_theme_chain(&active_theme, &theme_roots);

    for theme in &themes {
        for root in &theme_roots {
            let theme_path = format!("{}/{}", root, theme);
            let theme_dir = Path::new(&theme_path);
            if !theme_dir.is_dir() {
                continue;
            }

            let dirs = get_theme_directories(theme_dir);
            for sub in &dirs {
                let dir_path = theme_dir.join(sub);
                if !dir_path.is_dir() {
                    continue;
                }

                for cand in &candidates {
                    for ext in &["svg", "png"] {
                        let cand_file = dir_path.join(format!("{}.{}", cand, ext));
                        if cand_file.is_file() {
                            let res = Some(cand_file.to_string_lossy().to_string());
                            ICON_CACHE
                                .lock()
                                .unwrap()
                                .insert(trimmed.to_string(), res.clone());
                            return res;
                        }
                    }
                }
            }
        }
    }

    let pixmap_dirs = [
        format!("{}/.local/share/pixmaps", home),
        "/usr/local/share/pixmaps".to_string(),
        "/usr/share/pixmaps".to_string(),
    ];

    for pdir in &pixmap_dirs {
        if !Path::new(pdir).is_dir() {
            continue;
        }

        for cand in &candidates {
            for ext in &["svg", "png"] {
                let cand_file = format!("{}/{}.{}", pdir, cand, ext);
                if Path::new(&cand_file).is_file() {
                    let res = Some(cand_file);
                    ICON_CACHE
                        .lock()
                        .unwrap()
                        .insert(trimmed.to_string(), res.clone());
                    return res;
                }
            }
        }
    }

    ICON_CACHE.lock().unwrap().insert(trimmed.to_string(), None);
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_standard_icons() {
        if std::env::var("CI").is_ok() || resolve_icon("system-shutdown").is_none() {
            return;
        }
        let code_icon = resolve_icon("code");
        assert!(code_icon.is_some(), "code icon should resolve");

        let chrome_icon = resolve_icon("google-chrome");
        assert!(chrome_icon.is_some(), "chrome icon should resolve");

        let shutdown_icon = resolve_icon("system-shutdown");
        assert!(
            shutdown_icon.is_some(),
            "system-shutdown icon should resolve dynamically"
        );

        let spotify_icon = resolve_icon("Spotify");
        assert!(
            spotify_icon.is_some(),
            "Spotify icon should resolve via StartupWMClass/desktop indexing"
        );

        let steam_icon = resolve_icon("steam");
        assert!(
            steam_icon.is_some(),
            "steam icon should resolve without game shortcuts collision"
        );
    }
}
