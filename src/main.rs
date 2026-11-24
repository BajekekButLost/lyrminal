mod anim;
mod app;
mod cli;
mod config;
mod library;
mod lyrics;
mod player;
mod providers;
mod render;
mod state;
mod term;
mod theme;
mod ui;
mod update;

use std::io::Write;
use std::time::{Duration, Instant};

use anyhow::Result;
use clap::{CommandFactory, Parser};
use crossterm::event::{DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture, EventStream};
use crossterm::execute;
use futures::StreamExt;
use ratatui::DefaultTerminal;
use tokio::sync::mpsc;

use crate::app::App;
use crate::config::Config;
use crate::ui::draw::{self, Graphics};

const IDLE_FRAME: Duration = Duration::from_millis(250);

fn setup_base(global: &cli::GlobalArgs) {
    if global.base_directory.is_some() && global.temp_base_directory {
        let _ = cli::Cli::command()
            .error(
                clap::error::ErrorKind::ArgumentConflict,
                "use only one of --base-directory and --temp-base-directory",
            )
            .exit();
    }
    let base = match (&global.base_directory, global.temp_base_directory) {
        (Some(dir), _) => Some(config::prepare_base_dir(&library::expand_path(dir))),
        (None, true) => Some(config::create_temp_base_dir()),
        (None, false) => None,
    };
    match base {
        Some(Ok(dir)) => config::set_base_dir(dir),
        Some(Err(e)) => cli::fail(format!("{e:#}")),
        None => {}
    }
    if let Some(file) = &global.config {
        config::set_config_file(file_arg("--config", file));
    }
    if let Some(file) = &global.state_file {
        config::set_state_file(file_arg("--state-file", file));
    }
    if let Some(dir) = &global.cache_dir {
        config::set_cache_dir(dir_arg("--cache-dir", dir));
    }
}

fn dir_arg(flag: &str, value: &str) -> std::path::PathBuf {
    let path = library::expand_path(value);
    if path.exists() && !path.is_dir() {
        cli::fail(format!("{flag} needs a folder, {} is a file", path.display()));
    }
    path
}

fn file_arg(flag: &str, value: &str) -> std::path::PathBuf {
    let path = library::expand_path(value);
    if path.is_dir() {
        cli::fail(format!("{flag} needs a file, {} is a folder", path.display()));
    }
    path
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = cli::Cli::parse();
    setup_base(&args.global);
    if let Some(command) = args.command {
        let code = cli::run(command).await;
        std::process::exit(code);
    }
    let temp = args.global.temp_base_directory;

    let fresh = !Config::path().exists();
    let cfg = match Config::load() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("lyrminal: {e:#}");
            eprintln!("Fix or delete the file to start with defaults.");
            std::process::exit(1);
        }
    };

    let caps = term::probe();
    let mut terminal = ratatui::init();
    term::reset_backdrop_on_panic();
    let mut mouse = cfg.appearance.mouse;
    let _ = execute!(std::io::stdout(), EnableBracketedPaste);
    if mouse {
        let _ = execute!(std::io::stdout(), EnableMouseCapture);
    }

    let (player_tx, player_rx) = player::spawn(cfg.player.clone());
    let (msg_tx, msg_rx) = mpsc::unbounded_channel();
    let mut app = App::new(cfg, caps, player_tx, msg_tx);
    app.maybe_onboard(fresh);
    app.enable_update_checks();
    let mut gfx = Graphics::default();
    let mut backdrop = term::Backdrop::default();

    let result = run(&mut terminal, &mut app, &mut gfx, &mut backdrop, player_rx, msg_rx, &mut mouse).await;

    let mut out = std::io::stdout();
    let _ = gfx.clear(&mut out);
    let _ = backdrop.reset(&mut out);
    let _ = out.flush();
    term::kitty::cleanup_temp_files();
    if mouse {
        let _ = execute!(out, DisableMouseCapture);
    }
    let _ = execute!(out, DisableBracketedPaste);
    ratatui::restore();
    if temp && let Some(dir) = config::base_dir() {
        eprintln!("lyrminal: the temporary base directory is {}", dir.display());
    }
    if result.is_ok()
        && let Some(path) = app.restart_into.take()
    {
        let e = update::restart(&path);
        eprintln!("lyrminal: could not start {}: {e}", path.display());
        std::process::exit(1);
    }
    result
}

async fn run(
    terminal: &mut DefaultTerminal,
    app: &mut App,
    gfx: &mut Graphics,
    backdrop: &mut term::Backdrop,
    mut player_rx: mpsc::UnboundedReceiver<player::PlayerEvent>,
    mut msg_rx: mpsc::UnboundedReceiver<app::AppMsg>,
    mouse: &mut bool,
) -> Result<()> {
    let mut events = EventStream::new();
    let mut next_frame = tokio::time::Instant::now();
    let mut last_area = None;
    loop {
        tokio::select! {
            _ = tokio::time::sleep_until(next_frame) => {}
            ev = events.next() => match ev {
                Some(Ok(ev)) => app.on_event(ev),
                Some(Err(_)) | None => return Ok(()),
            },
            Some(ev) = player_rx.recv() => app.on_player(ev),
            Some(m) = msg_rx.recv() => app.on_msg(m),
        }
        let now = Instant::now();
        app.tick(now);
        if app.quit {
            return Ok(());
        }
        if app.cfg.appearance.mouse != *mouse {
            *mouse = app.cfg.appearance.mouse;
            if *mouse {
                let _ = execute!(std::io::stdout(), EnableMouseCapture);
            } else {
                let _ = execute!(std::io::stdout(), DisableMouseCapture);
            }
        }
        let tnow = tokio::time::Instant::now();
        if tnow < next_frame && !app.dirty {
            continue;
        }
        let animating = app.needs_animation();
        let dt = app.begin_frame(now);
        let mut output = None;
        let area = terminal
            .draw(|f| {
                output = Some(draw::draw(f, app, now, dt));
            })?
            .area;
        let resized = last_area.replace(area).is_some_and(|a| a != area);
        app.dirty = false;
        let pixel = output.as_ref().is_some_and(|o| o.pixel);
        if let Some(out) = output {
            let mut stdout = std::io::stdout().lock();
            backdrop.sync(&mut stdout, app.theme.backdrop())?;
            if resized {
                gfx.screen_cleared(&mut stdout)?;
            }
            if out.pixel {
                if let Some(p) = app.pixel.as_ref() {
                    let size = terminal.size()?;
                    let bleed = render::pixel::BLEED_CELLS;
                    gfx.send_pixel(&mut stdout, &p.canvas, size.width + bleed, size.height + bleed, app.caps.remote)?;
                }
            } else {
                gfx.clear_pixel(&mut stdout)?;
            }
            gfx.sync_art(&mut stdout, app, out.art)?;
            draw::write_links(&mut stdout, &out.links)?;
            stdout.flush()?;
        }
        let fps = if pixel {
            let cap = if app.caps.remote { 8 } else { 60 };
            app.cfg.appearance.pixel_fps.clamp(5, cap) as f64
        } else {
            app.cfg.appearance.fps.clamp(15, 240) as f64
        };
        let frame = Duration::from_secs_f64(1.0 / fps);
        next_frame = tnow + if animating || app.needs_animation() { frame } else { IDLE_FRAME };
    }
}
