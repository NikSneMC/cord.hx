//! `'lastfm`: shows the track playing on Last.fm.
//!
//! Needs `LASTFM_USERNAME` and `LASTFM_API_KEY` in the environment, and `curl`.
//!
//! Config:
//! * `'interval` - milliseconds between checks (default 10000, at least 500)
//! * `'max-retries` - curl retries (default 3)
//! * `'override?` - replace the editor activity with the track (default #t)
//! * `'fallback-image` - image for tracks without cover art

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::{Extension, Setup};
use crate::plugin::core::Core;
use crate::plugin::log;
use crate::plugin::server;
use crate::plugin::util::{self, unix_time};
use crate::plugin::value::{non_empty, to_display};

const BASE_URL: &str = "http://ws.audioscrobbler.com/2.0/?method=";
const LOGO: &str = "https://us1.discourse-cdn.com/flex021/uploads/lastfm/optimized/2X/f/f1f8c34ea6f18aad5b8f89905e38ba7b3424d9b1_2_512x512.png";
const DEFAULT_IMAGE: &str = "https://lastfm.freetls.fastly.net/i/u/300x300/2a96cbd8b46e442fc41c2b86b821562f.png";

#[derive(Clone, PartialEq)]
struct Track {
    title: String,
    artist: String,
    album: Option<String>,
    url: Option<String>,
    image: String,
}

#[derive(Default)]
struct State {
    track: Option<Track>,
    fetched: Option<Instant>,
    fetching: bool,
    reported: Option<Track>,
    stopped: bool,
    ticking: bool,
}

struct Settings {
    interval: Duration,
    max_retries: u64,
    override_activity: bool,
    fallback_image: String,
    credentials: String,
}

/// Fetches the track playing now. Blocks: call it off the core thread.
fn fetch(settings: &Settings) -> Result<Option<Track>, String> {
    let url = format!(
        "{BASE_URL}user.getrecenttracks&format=json{}&limit=1&extended=1",
        settings.credentials
    );
    let retries = settings.max_retries.to_string();
    let output = util::run(
        "curl",
        &[
            &url,
            "--fail",
            "--silent",
            "--show-error",
            "--retry",
            &retries,
        ],
        None,
    )?;
    if output.code != 0 {
        let stderr = output.stderr.trim();
        return Err(format!(
            "curl exited with code {}{}",
            output.code,
            if stderr.is_empty() {
                String::new()
            } else {
                format!(": {stderr}")
            }
        ));
    }
    let data: Value = serde_json::from_str(&output.stdout)
        .map_err(|_| "Error while decoding JSON".to_string())?;
    let track = match data.pointer("/recenttracks/track") {
        Some(Value::Array(tracks)) => tracks.first(),
        Some(track @ Value::Object(_)) => Some(track),
        _ => None,
    };
    let Some(track) = track.filter(|t| {
        t.pointer("/@attr/nowplaying").is_some_and(|v| !v.is_null())
    }) else {
        return Ok(None);
    };
    let image = track
        .get("image")
        .and_then(Value::as_array)
        .and_then(|images| non_empty(images.last()?.get("#text")))
        .map(str::to_string)
        .unwrap_or_else(|| settings.fallback_image.clone());
    Ok(Some(Track {
        title: non_empty(track.get("name"))
            .unwrap_or("Unknown")
            .to_string(),
        artist: non_empty(track.pointer("/artist/name"))
            .unwrap_or("Unknown")
            .to_string(),
        album: non_empty(track.pointer("/album/#text")).map(str::to_string),
        url: non_empty(track.get("url")).map(str::to_string),
        image: if image == DEFAULT_IMAGE {
            settings.fallback_image.clone()
        } else {
            image
        },
    }))
}

/// Refreshes the track in the background when the cached one is stale, then
/// calls `then` with the current track.
fn refresh(
    core: &mut Core,
    settings: &Arc<Settings>,
    state: &Arc<Mutex<State>>,
    then: impl FnOnce(&mut Core, Option<Track>) + Send + 'static,
) {
    let track = {
        let mut s = state.lock().unwrap();
        let fresh = s.fetched.is_some_and(|t| t.elapsed() < settings.interval);
        if !fresh && !s.fetching {
            s.fetching = true;
            None
        } else {
            Some(s.track.clone())
        }
    };
    if let Some(track) = track {
        then(core, track);
        return;
    }

    let (settings, state) = (Arc::clone(settings), Arc::clone(state));
    core.spawn(
        move || fetch(&settings),
        move |core, result| {
            if let Err(e) = &result {
                core.log(log::ERROR, || format!("lastfm: {e}"));
            }
            let track = {
                let mut s = state.lock().unwrap();
                s.fetching = false;
                s.fetched = Some(Instant::now());
                if let Ok(track) = result {
                    s.track = track;
                }
                s.track.clone()
            };
            then(core, track);
        },
    );
}

fn tick(core: &mut Core, settings: Arc<Settings>, state: Arc<Mutex<State>>) {
    if state.lock().unwrap().stopped {
        return;
    }
    let reported = Arc::clone(&state);
    refresh(core, &settings, &state, move |core, track| {
        let changed = {
            let mut s = reported.lock().unwrap();
            let changed = s.reported != track;
            s.reported = track.clone();
            changed
        };
        match track {
            _ if !changed => {}
            None => core.manager_clear_activity(false),
            Some(track) => {
                let activity = json!({
                    "type": "listening",
                    "status_display_type": "state",
                    "details": track.title,
                    "state": track.artist,
                    "assets": {
                        "large_image": track.image,
                        "large_text": track.album,
                        "large_url": track.url
                    },
                    "timestamps": { "start": unix_time() },
                });
                server::send(
                    core,
                    "update_activity",
                    json!({ "activity": activity, "force": true }),
                );
            }
        }
    });
    let interval = settings.interval;
    core.after(interval, move |core| tick(core, settings, state));
}

pub fn setup(_: &mut Core, s: &Setup<'_>) -> Result<Extension, String> {
    let username =
        util::env("LASTFM_USERNAME").ok_or("LASTFM_USERNAME is not set")?;
    let api_key =
        util::env("LASTFM_API_KEY").ok_or("LASTFM_API_KEY is not set")?;
    let settings = Arc::new(Settings {
        interval: Duration::from_millis(
            s.number("interval", 10000.0)?.max(500.0) as u64,
        ),
        max_retries: s.number("max-retries", 3.0)?.max(0.0) as u64,
        override_activity: s.bool("override?", true)?,
        fallback_image: s
            .get("fallback-image")
            .map(to_display)
            .unwrap_or_else(|| LOGO.to_string()),
        credentials: format!(
            "&user={}&api_key={}",
            util::url_encode(&username),
            util::url_encode(&api_key)
        ),
    });
    let state: Arc<Mutex<State>> = Arc::default();

    let mut ext = Extension::default();
    type Field = (&'static str, fn(&Track) -> Value);
    let fields: [Field; 5] = [
        ("track-title", |t| json!(t.title)),
        ("track-artist", |t| json!(t.artist)),
        ("track-album", |t| json!(t.album)),
        ("track-url", |t| json!(t.url)),
        ("track-image", |t| json!(t.image)),
    ];
    for (name, get) in fields {
        let (settings, state) = (Arc::clone(&settings), Arc::clone(&state));
        ext.variable(name, move |core, _| {
            refresh(core, &settings, &state, |_, _| {});
            state
                .lock()
                .unwrap()
                .track
                .as_ref()
                .map(get)
                .unwrap_or(Value::Null)
        });
    }

    {
        let (settings, state) = (Arc::clone(&settings), Arc::clone(&state));
        ext.hook("ready", 100, move |core, _| {
            if settings.override_activity {
                core.manager_skip_update();
                core.manager_pause();
                let mut s = state.lock().unwrap();
                if !s.ticking {
                    s.ticking = true;
                    drop(s);
                    tick(core, Arc::clone(&settings), Arc::clone(&state));
                }
            }
            Value::Null
        });
    }
    ext.shutdown =
        Some(Box::new(move |_| state.lock().unwrap().stopped = true));
    Ok(ext)
}
