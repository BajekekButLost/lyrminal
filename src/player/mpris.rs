use std::collections::HashMap;
use std::pin::Pin;
use std::time::{Duration, Instant};

use futures::{Stream, StreamExt, stream};
use tokio::sync::mpsc;
use zbus::fdo::{DBusProxy, PropertiesProxy};
use zbus::names::BusName;
use zbus::proxy::CacheProperties;
use zbus::{Connection, ObjectPath, OwnedVariant, Variant, proxy};

use super::{
    BUS_PREFIX, Capabilities, MediaKind, PlayerCommand, PlayerEvent, PlayerInfo, Snapshot, Status, TrackMeta, choose,
    spotify_id_from,
};
use crate::config::PlayerConfig;

#[proxy(interface = "org.mpris.MediaPlayer2.Player", default_path = "/org/mpris/MediaPlayer2")]
trait Player {
    fn play_pause(&self) -> zbus::Result<()>;
    fn next(&self) -> zbus::Result<()>;
    fn previous(&self) -> zbus::Result<()>;
    fn seek(&self, offset: i64) -> zbus::Result<()>;
    fn set_position(&self, track_id: &ObjectPath<'_>, position: i64) -> zbus::Result<()>;

    #[zbus(property)]
    fn metadata(&self) -> zbus::Result<HashMap<String, OwnedVariant>>;
    #[zbus(property)]
    fn playback_status(&self) -> zbus::Result<String>;
    #[zbus(property)]
    fn position(&self) -> zbus::Result<i64>;
    #[zbus(property)]
    fn rate(&self) -> zbus::Result<f64>;
    #[zbus(property)]
    fn can_control(&self) -> zbus::Result<bool>;
    #[zbus(property)]
    fn can_seek(&self) -> zbus::Result<bool>;
    #[zbus(property)]
    fn can_go_next(&self) -> zbus::Result<bool>;
    #[zbus(property)]
    fn can_go_previous(&self) -> zbus::Result<bool>;
    #[zbus(property)]
    fn can_play(&self) -> zbus::Result<bool>;
    #[zbus(property)]
    fn can_pause(&self) -> zbus::Result<bool>;

    #[zbus(signal)]
    fn seeked(&self, position: i64) -> zbus::Result<()>;
}

#[proxy(interface = "org.mpris.MediaPlayer2", default_path = "/org/mpris/MediaPlayer2")]
trait MediaPlayer2 {
    #[zbus(property)]
    fn identity(&self) -> zbus::Result<String>;
    #[zbus(property)]
    fn desktop_entry(&self) -> zbus::Result<String>;
}

const BURST_INTERVAL: Duration = Duration::from_millis(100);
const BURST_LENGTH: Duration = Duration::from_millis(3500);

enum SelEvent {
    Changed,
    Seeked(i64),
}

type SelStream = Pin<Box<dyn Stream<Item = SelEvent> + Send>>;

struct Selected {
    bus: String,
    player: PlayerProxy<'static>,
    track_id: Option<String>,
    last: Option<Snapshot>,
}

fn str_of(v: &Variant<'_>) -> Option<String> {
    match v {
        Variant::Str(s) => Some(s.to_string()),
        Variant::ObjectPath(p) => Some(p.as_str().to_string()),
        Variant::Variant(inner) => str_of(inner),
        _ => None,
    }
}

fn strs_of(v: &Variant<'_>) -> Vec<String> {
    match v {
        Variant::Array(a) => a.inner().iter().filter_map(str_of).collect(),
        Variant::Variant(inner) => strs_of(inner),
        other => str_of(other).into_iter().collect(),
    }
}

fn num_of(v: &Variant<'_>) -> Option<f64> {
    match v {
        Variant::I64(n) => Some(*n as f64),
        Variant::U64(n) => Some(*n as f64),
        Variant::I32(n) => Some(*n as f64),
        Variant::U32(n) => Some(*n as f64),
        Variant::I16(n) => Some(*n as f64),
        Variant::U16(n) => Some(*n as f64),
        Variant::U8(n) => Some(*n as f64),
        Variant::F64(n) => Some(*n),
        Variant::Variant(inner) => num_of(inner),
        _ => None,
    }
}

fn parse_meta(map: &HashMap<String, OwnedVariant>) -> Option<TrackMeta> {
    let get = |k: &str| map.get(k).map(|v| &**v);
    let title = get("xesam:title").and_then(str_of).unwrap_or_default();
    let url = get("xesam:url").and_then(str_of).filter(|s| !s.is_empty());
    let track_id = get("mpris:trackid").and_then(str_of);
    if title.trim().is_empty() && url.is_none() {
        return None;
    }
    let (spotify_id, kind) = spotify_id_from(url.as_deref(), track_id.as_deref());
    let isrc = map
        .iter()
        .find(|(k, _)| k.to_lowercase().ends_with("isrc"))
        .and_then(|(_, v)| str_of(v))
        .filter(|s| s.len() == 12);
    let length = get("mpris:length").and_then(num_of).map(|us| us / 1_000_000.0).filter(|l| *l > 0.0);
    Some(TrackMeta {
        title,
        artists: get("xesam:artist").map(strs_of).unwrap_or_default(),
        album: get("xesam:album").and_then(str_of).filter(|s| !s.is_empty()),
        album_artists: get("xesam:albumArtist").map(strs_of).unwrap_or_default(),
        length,
        track_id,
        url,
        art_url: get("mpris:artUrl").and_then(str_of).filter(|s| !s.is_empty()),
        spotify_id,
        isrc,
        track_number: get("xesam:trackNumber").and_then(num_of).map(|n| n as i64),
        disc_number: get("xesam:discNumber").and_then(num_of).map(|n| n as i64),
        kind: kind.or(Some(MediaKind::Track)),
    })
}

fn process_name(pid: u32) -> Option<String> {
    std::fs::read_to_string(format!("/proc/{pid}/comm"))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

async fn player_proxy(conn: &Connection, bus: &str) -> zbus::Result<PlayerProxy<'static>> {
    PlayerProxy::builder(conn)
        .destination(bus.to_string())
        .cache_properties(CacheProperties::No)
        .build()
        .await
}

async fn describe(conn: &Connection, dbus: &DBusProxy<'_>, bus: &str) -> Option<PlayerInfo> {
    let name = BusName::try_from(bus).ok()?;
    let pid = dbus.get_connection_unix_process_id(name).await.ok();
    let mp = MediaPlayer2Proxy::builder(conn)
        .destination(bus.to_string())
        .cache_properties(CacheProperties::No)
        .build()
        .await
        .ok();
    let (identity, desktop_entry) = match &mp {
        Some(mp) => (
            mp.identity().await.unwrap_or_default(),
            mp.desktop_entry().await.ok().filter(|s| !s.is_empty()),
        ),
        None => (String::new(), None),
    };
    let player = player_proxy(conn, bus).await.ok()?;
    let status = Status::parse(&player.playback_status().await.unwrap_or_default());
    let now_playing = player.metadata().await.ok().as_ref().and_then(parse_meta).map(|m| {
        if m.artists.is_empty() {
            m.title
        } else {
            format!("{} — {}", m.title, m.artists.join(", "))
        }
    });
    Some(PlayerInfo {
        bus: bus.to_string(),
        identity,
        process: pid.and_then(process_name),
        pid,
        desktop_entry,
        status,
        now_playing,
    })
}

async fn scan(conn: &Connection, dbus: &DBusProxy<'_>) -> Vec<PlayerInfo> {
    let names = dbus.list_names().await.unwrap_or_default();
    let mut out = Vec::new();
    for n in names {
        let n = n.to_string();
        if n.starts_with(BUS_PREFIX)
            && let Some(info) = describe(conn, dbus, &n).await
        {
            out.push(info);
        }
    }
    out.sort_by(|a, b| a.bus.cmp(&b.bus));
    out
}

async fn snapshot(player: &PlayerProxy<'_>, info: &PlayerInfo) -> Snapshot {
    let track = player.metadata().await.ok().as_ref().and_then(parse_meta);
    let status = Status::parse(&player.playback_status().await.unwrap_or_default());
    let rate = player.rate().await.unwrap_or(1.0);
    let before = Instant::now();
    let position = player.position().await.unwrap_or(0) as f64 / 1_000_000.0;
    let after = Instant::now();
    let sampled_at = before + (after - before) / 2;
    let caps = Capabilities {
        can_control: player.can_control().await.unwrap_or(false),
        can_seek: player.can_seek().await.unwrap_or(false),
        can_go_next: player.can_go_next().await.unwrap_or(false),
        can_go_previous: player.can_go_previous().await.unwrap_or(false),
        can_play: player.can_play().await.unwrap_or(false),
        can_pause: player.can_pause().await.unwrap_or(false),
    };
    let mut info = info.clone();
    info.status = status;
    Snapshot {
        player: info,
        track,
        status,
        rate: if rate > 0.0 { rate } else { 1.0 },
        position: position.max(0.0),
        sampled_at,
        caps,
    }
}

async fn watch(conn: &Connection, bus: &str) -> zbus::Result<(PlayerProxy<'static>, SelStream)> {
    let player = player_proxy(conn, bus).await?;
    let props = PropertiesProxy::builder(conn)
        .destination(bus.to_string())
        .path("/org/mpris/MediaPlayer2")
        .build()
        .await?;
    let changed = props.receive_properties_changed().await?.map(|_| SelEvent::Changed);
    let seeked = player
        .receive_seeked()
        .await?
        .filter_map(|s| async move { s.args().ok().map(|a| SelEvent::Seeked(*a.position())) });
    Ok((player, Box::pin(stream::select(changed, seeked))))
}

pub async fn run(
    mut cfg: PlayerConfig,
    mut cmd_rx: mpsc::UnboundedReceiver<PlayerCommand>,
    ev_tx: mpsc::UnboundedSender<PlayerEvent>,
) {
    loop {
        match Connection::session().await {
            Ok(conn) => {
                if let Err(e) = run_conn(&conn, &mut cfg, &mut cmd_rx, &ev_tx).await {
                    let _ = ev_tx.send(PlayerEvent::Error(format!("media bus: {e}")));
                }
            }
            Err(e) => {
                let _ = ev_tx.send(PlayerEvent::Error(format!("cannot reach the session bus: {e}")));
            }
        }
        if ev_tx.is_closed() {
            return;
        }
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
}

async fn run_conn(
    conn: &Connection,
    cfg: &mut PlayerConfig,
    cmd_rx: &mut mpsc::UnboundedReceiver<PlayerCommand>,
    ev_tx: &mpsc::UnboundedSender<PlayerEvent>,
) -> zbus::Result<()> {
    let dbus = DBusProxy::new(conn).await?;
    let mut owners = dbus.receive_name_owner_changed().await?;
    let mut players = scan(conn, &dbus).await;
    let _ = ev_tx.send(PlayerEvent::Players(players.clone()));
    let mut selected: Option<Selected> = None;
    let mut sel_stream: SelStream = Box::pin(stream::pending());
    let mut poll = tokio::time::interval(BURST_INTERVAL);
    poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut next_poll = Instant::now();
    let mut burst_until = Instant::now() + BURST_LENGTH;
    let mut rescan = tokio::time::interval(Duration::from_secs(2));
    rescan.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut need_reselect = true;

    loop {
        if need_reselect {
            need_reselect = false;
            let current = selected.as_ref().map(|s| s.bus.clone());
            let target = choose(&players, cfg, current.as_deref()).cloned();
            if target.as_ref().map(|t| &t.bus) != current.as_ref() {
                selected = None;
                sel_stream = Box::pin(stream::pending());
                match target {
                    Some(info) => match watch(conn, &info.bus).await {
                        Ok((player, s)) => {
                            let snap = snapshot(&player, &info).await;
                            let track_id = snap.track.as_ref().and_then(|t| t.track_id.clone());
                            let _ = ev_tx.send(PlayerEvent::Active(Some(snap.clone())));
                            selected = Some(Selected {
                                bus: info.bus.clone(),
                                player,
                                track_id,
                                last: Some(snap),
                            });
                            sel_stream = s;
                            burst_until = Instant::now() + BURST_LENGTH;
                            next_poll = Instant::now();
                        }
                        Err(e) => {
                            let _ = ev_tx.send(PlayerEvent::Error(format!("cannot watch {}: {e}", info.bus)));
                            let _ = ev_tx.send(PlayerEvent::Active(None));
                        }
                    },
                    None => {
                        let _ = ev_tx.send(PlayerEvent::Active(None));
                    }
                }
            }
        }

        tokio::select! {
            sig = owners.next() => {
                let Some(sig) = sig else { return Ok(()) };
                if let Ok(args) = sig.args()
                    && args.name().as_str().starts_with(BUS_PREFIX)
                {
                    players = scan(conn, &dbus).await;
                    let _ = ev_tx.send(PlayerEvent::Players(players.clone()));
                    if selected.as_ref().is_some_and(|s| !players.iter().any(|p| p.bus == s.bus)) {
                        selected = None;
                        sel_stream = Box::pin(stream::pending());
                    }
                    need_reselect = true;
                }
            }
            _ = rescan.tick() => {
                let fresh = scan(conn, &dbus).await;
                let statuses_changed = fresh.len() != players.len()
                    || fresh.iter().zip(players.iter()).any(|(a, b)| a.bus != b.bus || a.status != b.status);
                if fresh != players {
                    players = fresh;
                    let _ = ev_tx.send(PlayerEvent::Players(players.clone()));
                }
                if statuses_changed {
                    need_reselect = true;
                }
            }
            _ = poll.tick() => {
                let now = Instant::now();
                if now < next_poll {
                    continue;
                }
                let interval = if now < burst_until {
                    BURST_INTERVAL
                } else {
                    Duration::from_millis(cfg.poll_interval_ms.clamp(100, 5000))
                };
                next_poll = now + interval - Duration::from_millis(5);
                if let Some(sel) = selected.as_ref() {
                    let before = Instant::now();
                    if let Ok(us) = sel.player.position().await {
                        let after = Instant::now();
                        let _ = ev_tx.send(PlayerEvent::Position {
                            position: (us as f64 / 1_000_000.0).max(0.0),
                            at: before + (after - before) / 2,
                            seeked: false,
                        });
                    }
                }
            }
            ev = sel_stream.next() => {
                match ev {
                    Some(SelEvent::Changed) => {
                        if let Some(sel) = selected.as_mut() {
                            let info = players.iter().find(|p| p.bus == sel.bus).cloned().unwrap_or_else(|| PlayerInfo {
                                bus: sel.bus.clone(),
                                identity: String::new(),
                                process: None,
                                pid: None,
                                desktop_entry: None,
                                status: Status::Stopped,
                                now_playing: None,
                            });
                            let snap = snapshot(&sel.player, &info).await;
                            sel.track_id = snap.track.as_ref().and_then(|t| t.track_id.clone());
                            if let Some(p) = players.iter_mut().find(|p| p.bus == sel.bus)
                                && p.status != snap.status
                            {
                                p.status = snap.status;
                                let _ = ev_tx.send(PlayerEvent::Players(players.clone()));
                                need_reselect = true;
                            }
                            sel.last = Some(snap.clone());
                            let _ = ev_tx.send(PlayerEvent::Active(Some(snap)));
                            burst_until = Instant::now() + BURST_LENGTH;
                            next_poll = Instant::now();
                        }
                    }
                    Some(SelEvent::Seeked(us)) => {
                        let _ = ev_tx.send(PlayerEvent::Position {
                            position: (us as f64 / 1_000_000.0).max(0.0),
                            at: Instant::now(),
                            seeked: true,
                        });
                        burst_until = Instant::now() + BURST_LENGTH;
                        next_poll = Instant::now() + BURST_INTERVAL;
                    }
                    None => {
                        selected = None;
                        sel_stream = Box::pin(stream::pending());
                        need_reselect = true;
                    }
                }
            }
            cmd = cmd_rx.recv() => {
                let Some(cmd) = cmd else { return Ok(()) };
                match cmd {
                    PlayerCommand::Reconfigure(new_cfg) => {
                        *cfg = new_cfg;
                        next_poll = Instant::now();
                        let current = selected.as_ref().map(|s| s.bus.clone());
                        if choose(&players, cfg, current.as_deref()).map(|p| p.bus.clone()) != current {
                            selected = None;
                            sel_stream = Box::pin(stream::pending());
                        }
                        need_reselect = true;
                    }
                    other => {
                        if let Some(sel) = selected.as_ref() {
                            let result = control(sel, other).await;
                            if let Err(e) = result {
                                let _ = ev_tx.send(PlayerEvent::Error(format!("control failed: {e}")));
                            }
                        }
                    }
                }
            }
        }
    }
}

async fn control(sel: &Selected, cmd: PlayerCommand) -> zbus::Result<()> {
    match cmd {
        PlayerCommand::PlayPause => sel.player.play_pause().await,
        PlayerCommand::Next => sel.player.next().await,
        PlayerCommand::Previous => sel.player.previous().await,
        PlayerCommand::SeekBy(secs) => sel.player.seek((secs * 1_000_000.0) as i64).await,
        PlayerCommand::SetPosition(secs) => {
            let target = (secs.max(0.0) * 1_000_000.0) as i64;
            match sel.track_id.as_deref().and_then(|t| ObjectPath::try_from(t).ok()) {
                Some(path) => sel.player.set_position(&path, target).await,
                None => {
                    let current = sel.player.position().await.unwrap_or(0);
                    sel.player.seek(target - current).await
                }
            }
        }
        PlayerCommand::Reconfigure(_) => Ok(()),
    }
}
