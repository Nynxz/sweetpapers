//! Daemon: owns the swap loop and serves the control socket.
//!
//! Architecture:
//!
//! - Main thread owns mutable state and runs the swap loop.
//! - A listener thread accepts Unix-socket connections and spawns a per-
//!   connection handler thread for each.
//! - Each handler thread reads one JSON request, hands it (plus a reply
//!   channel) to the main thread via an mpsc channel, waits for the reply,
//!   writes it back to the socket, and exits.
//! - Signals (SIGINT, SIGTERM, SIGHUP) are caught by a signal thread that
//!   pushes a [`LoopEvent::Shutdown`] into the same mpsc channel.
//!
//! Single-instance is enforced by binding the socket file: if another daemon
//! is already up and responsive, `bind_socket` errors. Stale sockets (from a
//! crashed previous run) are detected and removed.

use std::fs;
use std::io::BufReader;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use signal_hook::consts::{SIGHUP, SIGINT, SIGTERM};
use signal_hook::iterator::Signals;
use tracing::{debug, error, info, warn};

use crate::awww;
use crate::config::Config;
use crate::pack::{
    self, OrientationCache, Pack, Selection, pick_by_alphapack, pick_by_orientation,
};
use crate::paths::socket_path;
use crate::proto::{
    self, ListData, MonitorState, PackEntry, Request, Response, StatusData, ThumbnailData,
};
use crate::thumbnail::ThumbnailManager;

// ---- Loop events ----------------------------------------------------------

enum LoopEvent {
    Command {
        req: Request,
        reply: Sender<Response>,
    },
    Shutdown,
}

// ---- Daemon state ---------------------------------------------------------

struct DaemonState {
    config: Config,
    config_path: PathBuf,
    pack: Pack,
    profile: String,
    directory_index: usize,
    paused: bool,
    last_images: Vec<PathBuf>,
    monitors: Vec<MonitorState>,
    current_directory: Option<String>,
    orientation_cache: OrientationCache,
    thumbnails: ThumbnailManager,
}

impl DaemonState {
    fn new(config: Config, config_path: PathBuf, pack: Pack, profile: String) -> Result<Self> {
        let monitors = config
            .screens
            .iter()
            .map(|(k, v)| MonitorState {
                screen: k.clone(),
                name: v.name.clone(),
                image: None,
            })
            .collect();
        Ok(Self {
            config,
            config_path,
            pack,
            profile,
            directory_index: 0,
            paused: false,
            last_images: Vec::new(),
            monitors,
            current_directory: None,
            orientation_cache: OrientationCache::new(),
            thumbnails: ThumbnailManager::new()?,
        })
    }

    fn refresh_monitor_list(&mut self) {
        self.monitors = self
            .config
            .screens
            .iter()
            .map(|(k, v)| MonitorState {
                screen: k.clone(),
                name: v.name.clone(),
                image: None,
            })
            .collect();
    }
}

// ---- Entry point ----------------------------------------------------------

pub fn run(config_path: PathBuf, profile: String) -> Result<()> {
    let config = Config::load(&config_path)?;
    let pack_dir = config.pack_dir(&profile);
    let pack = Pack::load(&profile, pack_dir)?;
    info!(profile, directories = pack.directories.len(), "loaded pack");

    let sock_path = socket_path();
    let listener = bind_socket(&sock_path)?;
    info!(socket = %sock_path.display(), "listening");

    let (tx, rx) = mpsc::channel::<LoopEvent>();
    install_signal_handlers(tx.clone())?;
    spawn_listener(listener, tx.clone());

    let mut state = DaemonState::new(config, config_path, pack, profile)?;
    prefetch_thumbnails(&state);

    let result = run_loop(&mut state, &rx);

    // Best-effort cleanup of the socket file.
    let _ = fs::remove_file(&sock_path);
    if let Err(e) = &result {
        error!(error = %e, "daemon exited with error");
    }
    result
}

// ---- Socket binding -------------------------------------------------------

fn bind_socket(path: &Path) -> Result<UnixListener> {
    if path.exists() {
        match UnixStream::connect(path) {
            Ok(_) => {
                anyhow::bail!(
                    "another sweetpapers daemon is already running at {}",
                    path.display()
                );
            }
            Err(_) => {
                debug!(socket = %path.display(), "removing stale socket");
                fs::remove_file(path)
                    .with_context(|| format!("removing stale socket {}", path.display()))?;
            }
        }
    }
    UnixListener::bind(path).with_context(|| format!("binding socket {}", path.display()))
}

// ---- Signal handling ------------------------------------------------------

fn install_signal_handlers(tx: Sender<LoopEvent>) -> Result<()> {
    let mut signals =
        Signals::new([SIGINT, SIGTERM, SIGHUP]).context("registering signal handlers")?;
    thread::spawn(move || {
        if let Some(sig) = signals.forever().next() {
            info!(signal = sig, "shutdown signal received");
            let _ = tx.send(LoopEvent::Shutdown);
        }
    });
    Ok(())
}

// ---- Listener / connection handlers --------------------------------------

fn spawn_listener(listener: UnixListener, tx: Sender<LoopEvent>) {
    thread::spawn(move || {
        for stream in listener.incoming() {
            match stream {
                Ok(s) => {
                    let tx = tx.clone();
                    thread::spawn(move || {
                        if let Err(e) = handle_connection(s, tx) {
                            warn!(error = %e, "connection handler failed");
                        }
                    });
                }
                Err(e) => {
                    // EBADF on shutdown is the normal exit path.
                    debug!(error = %e, "accept failed; listener stopping");
                    break;
                }
            }
        }
    });
}

fn handle_connection(stream: UnixStream, tx: Sender<LoopEvent>) -> Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(5))).ok();
    let read_stream = stream.try_clone().context("cloning socket stream")?;
    let mut reader = BufReader::new(read_stream);
    let mut write_stream = stream;

    let req: Option<Request> = proto::read_json_line(&mut reader)?;
    let req = match req {
        Some(r) => r,
        None => return Ok(()),
    };

    let (rtx, rrx) = mpsc::channel::<Response>();
    if tx.send(LoopEvent::Command { req, reply: rtx }).is_err() {
        return proto::write_json_line(&mut write_stream, &Response::err("daemon shutting down"));
    }

    let resp = rrx
        .recv_timeout(Duration::from_secs(30))
        .unwrap_or_else(|_| Response::err("timed out waiting for daemon"));
    proto::write_json_line(&mut write_stream, &resp)
}

// ---- Main loop ------------------------------------------------------------

fn run_loop(state: &mut DaemonState, rx: &Receiver<LoopEvent>) -> Result<()> {
    let mut next_swap_at = Instant::now(); // swap immediately on startup

    loop {
        if state.paused {
            match rx.recv() {
                Ok(LoopEvent::Command { req, reply }) => {
                    let resp = dispatch(state, req);
                    let _ = reply.send(resp);
                    // If we just got resumed, schedule the next swap.
                    if !state.paused {
                        next_swap_at =
                            Instant::now() + Duration::from_secs(state.config.transition.interval);
                    }
                }
                Ok(LoopEvent::Shutdown) | Err(_) => return Ok(()),
            }
            continue;
        }

        let wait = next_swap_at.saturating_duration_since(Instant::now());
        match rx.recv_timeout(wait) {
            Ok(LoopEvent::Command { req, reply }) => {
                // Commands that change what's on screen now, or change the
                // cadence, restart the interval clock from this moment.
                let reset_timer = match &req {
                    Request::Next | Request::Prev | Request::Interval { .. } => true,
                    Request::Pack { .. } => state.config.defaults.swap_on_pack_change,
                    _ => false,
                };
                let resp = dispatch(state, req);
                let _ = reply.send(resp);
                if reset_timer {
                    next_swap_at =
                        Instant::now() + Duration::from_secs(state.config.transition.interval);
                }
            }
            Ok(LoopEvent::Shutdown) => return Ok(()),
            Err(RecvTimeoutError::Timeout) => {
                if let Err(e) = run_swap(state) {
                    warn!(error = %e, "swap failed");
                }
                next_swap_at =
                    Instant::now() + Duration::from_secs(state.config.transition.interval);
            }
            Err(RecvTimeoutError::Disconnected) => return Ok(()),
        }
    }
}

// ---- Command dispatch -----------------------------------------------------

fn dispatch(state: &mut DaemonState, req: Request) -> Response {
    match req {
        Request::Status => match build_status(state) {
            Ok(s) => Response::ok_with(&s).unwrap_or_else(|e| Response::err(e.to_string())),
            Err(e) => Response::err(e.to_string()),
        },
        Request::List => match build_list(state) {
            Ok(l) => Response::ok_with(&l).unwrap_or_else(|e| Response::err(e.to_string())),
            Err(e) => Response::err(e.to_string()),
        },
        Request::Pack { name } => match cmd_pack(state, &name) {
            Ok(()) => Response::ok_empty(),
            Err(e) => Response::err(e.to_string()),
        },
        Request::Next => match cmd_step(state, 1) {
            Ok(()) => Response::ok_empty(),
            Err(e) => Response::err(e.to_string()),
        },
        Request::Prev => match cmd_step(state, -1) {
            Ok(()) => Response::ok_empty(),
            Err(e) => Response::err(e.to_string()),
        },
        Request::Pause => {
            state.paused = true;
            info!("paused");
            Response::ok_empty()
        }
        Request::Resume => {
            state.paused = false;
            info!("resumed");
            Response::ok_empty()
        }
        Request::Reload => match cmd_reload(state) {
            Ok(()) => Response::ok_empty(),
            Err(e) => Response::err(e.to_string()),
        },
        Request::Interval { secs } => {
            state.config.transition.interval = secs;
            info!(secs, "interval updated");
            Response::ok_empty()
        }
        Request::Thumbnail { name, force } => match cmd_thumbnail(state, &name, force) {
            Ok(data) => Response::ok_with(&data).unwrap_or_else(|e| Response::err(e.to_string())),
            Err(e) => Response::err(e.to_string()),
        },
    }
}

fn build_status(state: &DaemonState) -> Result<StatusData> {
    Ok(StatusData {
        pack: state.profile.clone(),
        paused: state.paused,
        interval_secs: state.config.transition.interval,
        current_directory: state.current_directory.clone(),
        monitors: state.monitors.clone(),
    })
}

fn build_list(state: &DaemonState) -> Result<ListData> {
    let names = pack::discover_packs(&state.config.packs_root())?;
    let packs = names
        .into_iter()
        .map(|name| {
            let pack_dir = state.config.pack_dir(&name);
            let thumbnail = match state.thumbnails.ensure(&name, &pack_dir) {
                Ok(opt) => opt,
                Err(e) => {
                    warn!(pack = %name, error = %e, "thumbnail generation failed");
                    None
                }
            };
            PackEntry { name, thumbnail }
        })
        .collect();
    Ok(ListData { packs })
}

fn cmd_pack(state: &mut DaemonState, name: &str) -> Result<()> {
    let dir = state.config.pack_dir(name);
    let pack = Pack::load(name, dir)?;
    state.pack = pack;
    state.profile = name.to_string();
    state.directory_index = 0;
    state.current_directory = None;
    // Keep last_images so the next pick avoids repeats where possible.
    info!(pack = name, "switched pack");
    if state.config.defaults.swap_on_pack_change {
        run_swap(state)?;
    }
    Ok(())
}

fn cmd_step(state: &mut DaemonState, delta: i32) -> Result<()> {
    let n = state.pack.directories.len() as i32;
    if n == 0 {
        anyhow::bail!("pack has no directories");
    }
    let new = ((state.directory_index as i32 + delta) % n + n) % n;
    state.directory_index = new as usize;
    run_swap(state)
}

fn cmd_reload(state: &mut DaemonState) -> Result<()> {
    let cfg = Config::load(&state.config_path)?;
    state.config = cfg;
    state.refresh_monitor_list();
    state.orientation_cache.invalidate_all();
    info!("config reloaded");
    Ok(())
}

fn cmd_thumbnail(state: &DaemonState, name: &str, force: bool) -> Result<ThumbnailData> {
    let dir = state.config.pack_dir(name);
    let path = if force {
        state.thumbnails.force(name, &dir)?
    } else {
        state
            .thumbnails
            .ensure(name, &dir)?
            .with_context(|| format!("no images in pack '{}'", name))?
    };
    Ok(ThumbnailData {
        name: name.to_string(),
        path,
    })
}

// ---- Swap execution -------------------------------------------------------

fn run_swap(state: &mut DaemonState) -> Result<()> {
    let dir = state
        .pack
        .directory_at(state.config.transition.next, state.directory_index)
        .to_path_buf();
    state.current_directory = dir.file_name().and_then(|s| s.to_str()).map(str::to_string);

    let selection: Selection = if state.config.defaults.auto {
        let buckets = state.orientation_cache.buckets_for(&dir)?.clone();
        pick_by_orientation(&buckets, &state.config, &state.last_images)?
    } else {
        pick_by_alphapack(&dir, &state.config)?
    };

    let order = state.config.ordered_screen_keys();
    let by_screen: std::collections::HashMap<String, PathBuf> = selection.into_iter().collect();

    let sequence = state.config.defaults.sequence;
    let mut applied: Vec<PathBuf> = Vec::with_capacity(state.monitors.len());

    for screen_key in &order {
        let img = match by_screen.get(screen_key) {
            Some(p) => p,
            None => {
                debug!(
                    screen = screen_key,
                    "no image selected for screen this round"
                );
                continue;
            }
        };
        let monitor_name = match state.config.screens.get(screen_key) {
            Some(s) => s.name.clone(),
            None => continue,
        };

        if let Err(e) = awww::paint(&state.config.transition, &monitor_name, img) {
            warn!(monitor = %monitor_name, error = %e, "paint failed");
            continue;
        }

        // Update monitor state.
        for m in &mut state.monitors {
            if m.screen == *screen_key {
                m.image = Some(img.clone());
            }
        }
        applied.push(img.clone());

        if sequence {
            thread::sleep(Duration::from_secs(state.config.transition.interval));
        }
    }

    state.last_images = applied;

    // Advance directory index for the next ordered swap.
    if !state.pack.directories.is_empty() {
        state.directory_index = (state.directory_index + 1) % state.pack.directories.len();
    }
    Ok(())
}

// ---- Thumbnail prefetch ---------------------------------------------------

fn prefetch_thumbnails(state: &DaemonState) {
    let names = match pack::discover_packs(&state.config.packs_root()) {
        Ok(n) => n,
        Err(e) => {
            warn!(error = %e, "could not enumerate packs for thumbnail prefetch");
            return;
        }
    };
    for name in names {
        let dir = state.config.pack_dir(&name);
        if let Err(e) = state.thumbnails.ensure(&name, &dir) {
            debug!(pack = %name, error = %e, "thumbnail prefetch skipped");
        }
    }
}
