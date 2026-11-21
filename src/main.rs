mod anim;
mod app;
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

use std::io::Write;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture, EventStream};
use crossterm::execute;
use futures::StreamExt;
use ratatui::DefaultTerminal;
use tokio::sync::mpsc;

use crate::app::App;
use crate::config::Config;
use crate::ui::draw::{self, Graphics};

const IDLE_FRAME: Duration = Duration::from_millis(250);

fn usage() {
    println!(
        "Lyrminal v{} — synced lyrics in your terminal\n\n\
         Usage: lyrminal [--help] [--version]\n\n\
         Everything else is configured inside the app (press Esc).\n\
         Config: {}",
        env!("CARGO_PKG_VERSION"),
        Config::path().display()
    );
}

#[tokio::main]
async fn main() -> Result<()> {
    if let Some(arg) = std::env::args().nth(1) {
        match arg.as_str() {
            "-h" | "--help" => usage(),
            "-V" | "--version" => println!("lyrminal {}", env!("CARGO_PKG_VERSION")),
            other => {
                eprintln!("unknown argument: {other}");
                usage();
                std::process::exit(2);
            }
        }
        return Ok(());
    }

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
    let mut mouse = cfg.appearance.mouse;
    let _ = execute!(std::io::stdout(), EnableBracketedPaste);
    if mouse {
        let _ = execute!(std::io::stdout(), EnableMouseCapture);
    }

    let (player_tx, player_rx) = player::spawn(cfg.player.clone());
    let (msg_tx, msg_rx) = mpsc::unbounded_channel();
    let mut app = App::new(cfg, caps, player_tx, msg_tx);
    let mut gfx = Graphics::default();

    let result = run(&mut terminal, &mut app, &mut gfx, player_rx, msg_rx, &mut mouse).await;

    let mut out = std::io::stdout();
    let _ = gfx.clear(&mut out);
    let _ = out.flush();
    term::kitty::cleanup_temp_files();
    if mouse {
        let _ = execute!(out, DisableMouseCapture);
    }
    let _ = execute!(out, DisableBracketedPaste);
    ratatui::restore();
    result
}

async fn run(
    terminal: &mut DefaultTerminal,
    app: &mut App,
    gfx: &mut Graphics,
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
            if resized {
                gfx.screen_cleared(&mut stdout)?;
            }
            if out.pixel {
                if let Some(p) = app.pixel.as_ref() {
                    let size = terminal.size()?;
                    gfx.send_pixel(&mut stdout, &p.canvas, size.width, size.height, app.caps.remote)?;
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
