//! Wire protocol between the daemon and CLI / external clients.
//!
//! One JSON object per line over a Unix socket. The request is tagged by
//! `"cmd"`; the response carries a boolean `"ok"` plus an optional `"data"`
//! payload (success) or `"error"` string (failure).

use std::io::{BufRead, Write};
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "cmd", rename_all = "lowercase")]
pub enum Request {
    Status,
    List,
    Pack {
        name: String,
    },
    Next,
    Prev,
    Pause,
    Resume,
    Reload,
    Interval {
        secs: u64,
    },
    Thumbnail {
        name: String,
        #[serde(default)]
        force: bool,
    },
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Response {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub data: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub error: Option<String>,
}

impl Response {
    pub fn ok_empty() -> Self {
        Self {
            ok: true,
            data: None,
            error: None,
        }
    }

    pub fn ok_with<T: Serialize>(data: &T) -> Result<Self> {
        Ok(Self {
            ok: true,
            data: Some(serde_json::to_value(data)?),
            error: None,
        })
    }

    pub fn err(msg: impl Into<String>) -> Self {
        Self {
            ok: false,
            data: None,
            error: Some(msg.into()),
        }
    }
}

// ---- Typed payload shapes -------------------------------------------------

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct StatusData {
    pub pack: String,
    pub paused: bool,
    pub interval_secs: u64,
    pub current_directory: Option<String>,
    pub monitors: Vec<MonitorState>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MonitorState {
    pub screen: String,
    pub name: String,
    pub image: Option<PathBuf>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ListData {
    pub packs: Vec<PackEntry>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PackEntry {
    pub name: String,
    pub thumbnail: Option<PathBuf>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ThumbnailData {
    pub name: String,
    pub path: PathBuf,
}

// ---- Newline-delimited JSON helpers ---------------------------------------

pub fn write_json_line<W: Write, T: Serialize>(w: &mut W, value: &T) -> Result<()> {
    let mut buf = serde_json::to_vec(value).context("serializing message")?;
    buf.push(b'\n');
    w.write_all(&buf).context("writing message")?;
    w.flush().context("flushing message")?;
    Ok(())
}

/// Read one line of JSON. Returns `Ok(None)` on clean EOF.
pub fn read_json_line<R: BufRead, T: for<'de> Deserialize<'de>>(r: &mut R) -> Result<Option<T>> {
    let mut line = String::new();
    let n = r.read_line(&mut line).context("reading message")?;
    if n == 0 {
        return Ok(None);
    }
    let value = serde_json::from_str(line.trim_end())
        .with_context(|| format!("parsing message: {}", line.trim_end()))?;
    Ok(Some(value))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_round_trips_with_tag() {
        let req = Request::Pack {
            name: "Background1".into(),
        };
        let s = serde_json::to_string(&req).unwrap();
        assert!(s.contains(r#""cmd":"pack""#));
        assert!(s.contains(r#""name":"Background1""#));
        let back: Request = serde_json::from_str(&s).unwrap();
        match back {
            Request::Pack { name } => assert_eq!(name, "Background1"),
            other => panic!("wrong variant: {:?}", other),
        }
    }

    #[test]
    fn thumbnail_force_defaults_to_false() {
        let req: Request = serde_json::from_str(r#"{"cmd":"thumbnail","name":"X"}"#).unwrap();
        match req {
            Request::Thumbnail { name, force } => {
                assert_eq!(name, "X");
                assert!(!force);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn success_response_omits_error_field() {
        let r = Response::ok_with(&ListData {
            packs: vec![PackEntry {
                name: "A".into(),
                thumbnail: None,
            }],
        })
        .unwrap();
        let s = serde_json::to_string(&r).unwrap();
        assert!(s.contains(r#""ok":true"#));
        assert!(!s.contains("error"));
    }

    #[test]
    fn error_response_omits_data_field() {
        let r = Response::err("boom");
        let s = serde_json::to_string(&r).unwrap();
        assert!(s.contains(r#""ok":false"#));
        assert!(s.contains(r#""error":"boom""#));
        assert!(!s.contains("data"));
    }

    #[test]
    fn json_line_round_trip() {
        let mut buf: Vec<u8> = Vec::new();
        let original = Request::Next;
        write_json_line(&mut buf, &original).unwrap();
        assert!(buf.ends_with(b"\n"));
        let mut reader = std::io::Cursor::new(buf);
        let back: Option<Request> = read_json_line(&mut reader).unwrap();
        assert!(matches!(back, Some(Request::Next)));
        // Second read returns None (EOF).
        let eof: Option<Request> = read_json_line(&mut reader).unwrap();
        assert!(eof.is_none());
    }
}
