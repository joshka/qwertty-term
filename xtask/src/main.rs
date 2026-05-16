use std::{
    env,
    error::Error,
    fs,
    io::{BufWriter, Write},
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Command,
};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

fn main() -> Result<()> {
    let mut args = env::args().skip(1);
    match args.next().as_deref() {
        Some("bundle") => bundle(args.any(|arg| arg == "--release")),
        _ => {
            eprintln!("usage: cargo run -p xtask -- bundle [--release]");
            Ok(())
        }
    }
}

fn bundle(release: bool) -> Result<()> {
    build_app_binary(release)?;

    let profile = if release { "release" } else { "debug" };
    let binary = Path::new("target").join(profile).join("ghostty-rs");
    let app = Path::new("target").join("ghostty-rs.app");
    let contents = app.join("Contents");
    let macos = contents.join("MacOS");
    let resources = contents.join("Resources");

    if app.exists() {
        fs::remove_dir_all(&app)?;
    }
    fs::create_dir_all(&macos)?;
    fs::create_dir_all(&resources)?;

    fs::copy(&binary, macos.join("ghostty-rs-bin"))?;
    write_executable(macos.join("ghostty-rs"), launcher_script())?;
    write_app_icon(&resources)?;
    fs::write(contents.join("Info.plist"), info_plist())?;
    fs::write(contents.join("PkgInfo"), "APPL????")?;

    println!("{}", app.display());
    Ok(())
}

fn build_app_binary(release: bool) -> Result<()> {
    let mut command = Command::new("cargo");
    command.args(["build", "--package", "ghostty-rs"]);
    if release {
        command.arg("--release");
    }

    let status = command.status()?;
    if !status.success() {
        return Err("cargo build failed".into());
    }
    Ok(())
}

fn write_executable(path: PathBuf, contents: &str) -> Result<()> {
    let mut file = fs::File::create(&path)?;
    file.write_all(contents.as_bytes())?;
    let mut permissions = file.metadata()?.permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions)?;
    Ok(())
}

fn write_app_icon(resources: &Path) -> Result<()> {
    let iconset = Path::new("target").join("ghostty-rs.iconset");
    if iconset.exists() {
        fs::remove_dir_all(&iconset)?;
    }
    fs::create_dir_all(&iconset)?;

    for spec in icon_specs() {
        write_icon_png(&iconset.join(spec.file_name()), spec.pixels())?;
    }

    let icon_path = resources.join("ghostty-rs.icns");
    let status = Command::new("iconutil")
        .args(["-c", "icns", "-o"])
        .arg(&icon_path)
        .arg(&iconset)
        .status()?;
    if !status.success() {
        return Err("iconutil failed to create app icon".into());
    }
    fs::remove_dir_all(iconset)?;
    Ok(())
}

#[derive(Clone, Copy)]
struct IconSpec {
    points: u32,
    scale: u32,
}

impl IconSpec {
    fn file_name(self) -> String {
        if self.scale == 1 {
            format!("icon_{}x{}.png", self.points, self.points)
        } else {
            format!("icon_{}x{}@{}x.png", self.points, self.points, self.scale)
        }
    }

    fn pixels(self) -> u32 {
        self.points * self.scale
    }
}

fn icon_specs() -> [IconSpec; 10] {
    [
        IconSpec {
            points: 16,
            scale: 1,
        },
        IconSpec {
            points: 16,
            scale: 2,
        },
        IconSpec {
            points: 32,
            scale: 1,
        },
        IconSpec {
            points: 32,
            scale: 2,
        },
        IconSpec {
            points: 128,
            scale: 1,
        },
        IconSpec {
            points: 128,
            scale: 2,
        },
        IconSpec {
            points: 256,
            scale: 1,
        },
        IconSpec {
            points: 256,
            scale: 2,
        },
        IconSpec {
            points: 512,
            scale: 1,
        },
        IconSpec {
            points: 512,
            scale: 2,
        },
    ]
}

fn write_icon_png(path: &Path, size: u32) -> Result<()> {
    let mut rgba = vec![0; (size * size * 4) as usize];
    for y in 0..size {
        for x in 0..size {
            let idx = ((y * size + x) * 4) as usize;
            let pixel = icon_pixel(x, y, size);
            rgba[idx..idx + 4].copy_from_slice(&pixel);
        }
    }

    let file = fs::File::create(path)?;
    let writer = BufWriter::new(file);
    let mut encoder = png::Encoder::new(writer, size, size);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut png = encoder.write_header()?;
    png.write_image_data(&rgba)?;
    Ok(())
}

fn icon_pixel(x: u32, y: u32, size: u32) -> [u8; 4] {
    let edge = size as f32;
    let xf = x as f32 / edge;
    let yf = y as f32 / edge;
    let radius = edge * 0.22;
    if !inside_rounded_rect(x as f32, y as f32, edge, edge, radius) {
        return [0, 0, 0, 0];
    }

    let mut color = [
        (20.0 + 20.0 * yf) as u8,
        (26.0 + 36.0 * xf) as u8,
        (34.0 + 46.0 * (1.0 - yf)) as u8,
        255,
    ];

    let inset = edge * 0.16;
    let term_left = inset;
    let term_top = edge * 0.24;
    let term_right = edge - inset;
    let term_bottom = edge * 0.76;
    if x as f32 >= term_left
        && x as f32 <= term_right
        && y as f32 >= term_top
        && y as f32 <= term_bottom
    {
        color = [8, 12, 18, 255];
    }

    let stroke = (edge * 0.035).max(1.0);
    if near_rect_edge(
        x as f32,
        y as f32,
        term_left,
        term_top,
        term_right,
        term_bottom,
        stroke,
    ) {
        color = [120, 226, 190, 255];
    }

    let prompt_y = edge * 0.45;
    let prompt_x = edge * 0.30;
    let glyph = (edge * 0.055).max(1.0);
    if near_line(
        x as f32,
        y as f32,
        prompt_x,
        prompt_y,
        prompt_x + edge * 0.10,
        prompt_y + edge * 0.07,
        glyph,
    ) || near_line(
        x as f32,
        y as f32,
        prompt_x,
        prompt_y + edge * 0.14,
        prompt_x + edge * 0.10,
        prompt_y + edge * 0.07,
        glyph,
    ) || inside_rect(
        x as f32,
        y as f32,
        edge * 0.47,
        edge * 0.55,
        edge * 0.66,
        edge * 0.60,
    ) {
        color = [235, 248, 255, 255];
    }

    color
}

fn inside_rounded_rect(x: f32, y: f32, width: f32, height: f32, radius: f32) -> bool {
    let cx = x.clamp(radius, width - radius);
    let cy = y.clamp(radius, height - radius);
    let dx = x - cx;
    let dy = y - cy;
    dx * dx + dy * dy <= radius * radius
}

fn near_rect_edge(
    x: f32,
    y: f32,
    left: f32,
    top: f32,
    right: f32,
    bottom: f32,
    width: f32,
) -> bool {
    let on_horizontal =
        x >= left && x <= right && ((y - top).abs() <= width || (y - bottom).abs() <= width);
    let on_vertical =
        y >= top && y <= bottom && ((x - left).abs() <= width || (x - right).abs() <= width);
    on_horizontal || on_vertical
}

fn near_line(x: f32, y: f32, x1: f32, y1: f32, x2: f32, y2: f32, width: f32) -> bool {
    let dx = x2 - x1;
    let dy = y2 - y1;
    let length_sq = dx * dx + dy * dy;
    if length_sq == 0.0 {
        return false;
    }
    let t = (((x - x1) * dx + (y - y1) * dy) / length_sq).clamp(0.0, 1.0);
    let px = x1 + t * dx;
    let py = y1 + t * dy;
    let dist_x = x - px;
    let dist_y = y - py;
    dist_x * dist_x + dist_y * dist_y <= width * width
}

fn inside_rect(x: f32, y: f32, left: f32, top: f32, right: f32, bottom: f32) -> bool {
    x >= left && x <= right && y >= top && y <= bottom
}

fn launcher_script() -> &'static str {
    r#"#!/bin/sh
set -eu
dir="$(CDPATH= cd "$(dirname "$0")" && pwd)"
exec "$dir/ghostty-rs-bin" --window "$@"
"#
}

fn info_plist() -> &'static str {
    r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
  "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleDevelopmentRegion</key>
  <string>en</string>
  <key>CFBundleDisplayName</key>
  <string>ghostty-rs</string>
  <key>CFBundleExecutable</key>
  <string>ghostty-rs</string>
  <key>CFBundleIdentifier</key>
  <string>net.joshka.ghostty-rs</string>
  <key>CFBundleIconFile</key>
  <string>ghostty-rs</string>
  <key>CFBundleInfoDictionaryVersion</key>
  <string>6.0</string>
  <key>CFBundleName</key>
  <string>ghostty-rs</string>
  <key>CFBundlePackageType</key>
  <string>APPL</string>
  <key>CFBundleShortVersionString</key>
  <string>0.1.0</string>
  <key>CFBundleVersion</key>
  <string>0.1.0</string>
  <key>LSApplicationCategoryType</key>
  <string>public.app-category.utilities</string>
  <key>LSMinimumSystemVersion</key>
  <string>13.0</string>
  <key>NSPrincipalClass</key>
  <string>NSApplication</string>
  <key>NSHighResolutionCapable</key>
  <true/>
  <key>NSSupportsAutomaticGraphicsSwitching</key>
  <true/>
</dict>
</plist>
"#
}
