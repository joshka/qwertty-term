use std::{env, fs, path::PathBuf, process::Command};

use eframe::egui::{Context, Key, Modifiers, Vec2, ViewportCommand};

use crate::pty::PtyResult;

const DEFAULT_WINDOW_WIDTH: f32 = 960.0;
const DEFAULT_WINDOW_HEIGHT: f32 = 540.0;

#[derive(Clone, Debug)]
pub(crate) struct AppPreferences {
    pub(crate) window_size: Vec2,
    pub(crate) font_size: Option<f32>,
}

impl Default for AppPreferences {
    fn default() -> Self {
        Self {
            window_size: Vec2::new(DEFAULT_WINDOW_WIDTH, DEFAULT_WINDOW_HEIGHT),
            font_size: None,
        }
    }
}

impl AppPreferences {
    pub(crate) fn load() -> Self {
        let Some(path) = preferences_path() else {
            return Self::default();
        };
        let Ok(contents) = fs::read_to_string(path) else {
            return Self::default();
        };

        let mut preferences = Self::default();
        for line in contents.lines() {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            match key.trim() {
                "window_width" => {
                    if let Some(width) = parse_dimension(value) {
                        preferences.window_size.x = width;
                    }
                }
                "window_height" => {
                    if let Some(height) = parse_dimension(value) {
                        preferences.window_size.y = height;
                    }
                }
                "font_size" => preferences.font_size = parse_font_size(value),
                _ => {}
            }
        }
        preferences
    }

    pub(crate) fn save(&self) -> PtyResult<()> {
        let Some(path) = preferences_path() else {
            return Ok(());
        };
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        let font_size = self
            .font_size
            .map(|size| size.to_string())
            .unwrap_or_default();
        let contents = format!(
            "window_width={}\nwindow_height={}\nfont_size={}\n",
            self.window_size.x, self.window_size.y, font_size
        );
        fs::write(path, contents)?;
        Ok(())
    }
}

pub(crate) fn handle_shortcut(
    ctx: &Context,
    key: Key,
    modifiers: Modifiers,
    show_preferences: &mut bool,
) -> PtyResult<bool> {
    if !modifiers.mac_cmd {
        return Ok(false);
    }

    match key {
        Key::N => {
            spawn_new_window()?;
            Ok(true)
        }
        Key::Comma => {
            *show_preferences = !*show_preferences;
            Ok(true)
        }
        Key::Q => {
            ctx.send_viewport_cmd(ViewportCommand::Close);
            Ok(true)
        }
        _ => Ok(false),
    }
}

fn spawn_new_window() -> PtyResult<()> {
    Command::new(std::env::current_exe()?)
        .arg("--window")
        .spawn()?;
    Ok(())
}

fn preferences_path() -> Option<PathBuf> {
    let home = env::var_os("HOME")?;
    Some(
        PathBuf::from(home)
            .join("Library/Application Support")
            .join("qwertty-term")
            .join("preferences"),
    )
}

fn parse_dimension(value: &str) -> Option<f32> {
    value
        .trim()
        .parse::<f32>()
        .ok()
        .filter(|value| (320.0..=10_000.0).contains(value))
}

fn parse_font_size(value: &str) -> Option<f32> {
    value
        .trim()
        .parse::<f32>()
        .ok()
        .filter(|value| (6.0..=48.0).contains(value))
}
