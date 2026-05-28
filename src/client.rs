//! CLI client: opens the daemon socket, sends one request, prints the reply.
//!
//! Output:
//! - Default: human-readable text on stdout.
//! - `--json`: the raw response object verbatim.
//! - Exit code: 0 on `"ok": true`, 1 on `"ok": false` or transport error.

use std::io::{BufReader, Write};
use std::os::unix::net::UnixStream;
use std::process::ExitCode;
use std::time::Duration;

use anyhow::{Context, Result};

use crate::paths::socket_path;
use crate::proto::{
    self, ListData, PackEntry, Request, Response, StatusData, ThumbnailData,
};

pub fn dispatch(req: Request, json_out: bool) -> ExitCode {
    match send(req.clone()) {
        Ok(resp) => {
            if json_out {
                println!(
                    "{}",
                    serde_json::to_string(&resp).unwrap_or_else(|_| "{}".into())
                );
            } else {
                print_human(&req, &resp);
            }
            if resp.ok {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        Err(e) => {
            eprintln!("error: {:#}", e);
            ExitCode::FAILURE
        }
    }
}

fn send(req: Request) -> Result<Response> {
    let path = socket_path();
    let stream = UnixStream::connect(&path).with_context(|| {
        format!(
            "could not connect to sweetpapers daemon at {} (is the daemon running?)",
            path.display()
        )
    })?;
    stream.set_read_timeout(Some(Duration::from_secs(30))).ok();
    stream.set_write_timeout(Some(Duration::from_secs(5))).ok();

    let read_stream = stream.try_clone().context("cloning socket")?;
    let mut writer = stream;
    proto::write_json_line(&mut writer, &req)?;
    writer.flush().ok();
    let mut reader = BufReader::new(read_stream);
    let resp: Option<Response> = proto::read_json_line(&mut reader)?;
    resp.context("daemon closed connection without replying")
}

fn print_human(req: &Request, resp: &Response) {
    if !resp.ok {
        eprintln!(
            "error: {}",
            resp.error.as_deref().unwrap_or("(no error message)")
        );
        return;
    }
    match req {
        Request::Status => print_status(resp),
        Request::List => print_list(resp),
        Request::Thumbnail { .. } => print_thumbnail(resp),
        Request::Pack { .. }
        | Request::Next
        | Request::Prev
        | Request::Pause
        | Request::Resume
        | Request::Reload
        | Request::Interval { .. } => println!("ok"),
    }
}

fn print_status(resp: &Response) {
    let data = match decode::<StatusData>(resp) {
        Some(d) => d,
        None => return,
    };
    println!("pack:     {}", data.pack);
    println!(
        "state:    {}",
        if data.paused { "paused" } else { "running" }
    );
    println!("interval: {}s", data.interval_secs);
    if let Some(dir) = &data.current_directory {
        println!("dir:      {}", dir);
    }
    println!("monitors:");
    for m in &data.monitors {
        let img = m
            .image
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "(none)".into());
        println!("  [{}] {:<10} {}", m.screen, m.name, img);
    }
}

fn print_list(resp: &Response) {
    let data = match decode::<ListData>(resp) {
        Some(d) => d,
        None => return,
    };
    for PackEntry { name, thumbnail } in &data.packs {
        match thumbnail {
            Some(p) => println!("{}\t{}", name, p.display()),
            None => println!("{}", name),
        }
    }
}

fn print_thumbnail(resp: &Response) {
    if let Some(d) = decode::<ThumbnailData>(resp) {
        println!("{}", d.path.display());
    }
}

fn decode<T: serde::de::DeserializeOwned>(resp: &Response) -> Option<T> {
    let data = resp.data.as_ref()?;
    match serde_json::from_value::<T>(data.clone()) {
        Ok(v) => Some(v),
        Err(e) => {
            eprintln!("error: malformed response from daemon: {}", e);
            None
        }
    }
}
