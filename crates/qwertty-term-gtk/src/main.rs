//! `qwertty-term-gtk` binary: launches the GTK4 window, or runs a headless
//! smoke check with `--smoke` (realize the GLArea, render one frame, assert the
//! GL context is error-free and the framebuffer holds the clear color, then
//! quit). The smoke path runs the GTK main loop on the real process main
//! thread, which is the most robust way to exercise it in a container.

#[cfg(target_os = "linux")]
fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--text-smoke") {
        // Prove the GLArea presents real terminal glyphs (feed text → render →
        // readback asserts glyph ink), not just a clear color.
        let outcome = qwertty_term_gtk::run_text_smoke();
        println!("text-smoke: {outcome}");
        if outcome.glyphs_rendered() {
            println!("text-smoke: OK (bright_pixels={})", outcome.bright_pixels);
            std::process::ExitCode::SUCCESS
        } else {
            eprintln!("text-smoke: FAILED");
            std::process::ExitCode::FAILURE
        }
    } else if args.iter().any(|a| a == "--smoke") {
        let outcome = qwertty_term_gtk::run_smoke();
        println!("smoke: {outcome}");
        if outcome.is_ok() {
            println!("smoke: OK");
            std::process::ExitCode::SUCCESS
        } else {
            eprintln!("smoke: FAILED");
            std::process::ExitCode::FAILURE
        }
    } else {
        qwertty_term_gtk::run()
    }
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!(
        "qwertty-term-gtk is a Linux-only crate (GTK4 + libadwaita); nothing to run on this platform."
    );
}
