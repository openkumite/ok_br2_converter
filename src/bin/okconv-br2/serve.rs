//! `okconv-br2 serve DIR`: a static file server for the viewer with HTTP Range requests (seeking in video and
//! audio).

use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};

fn content_type(p: &Path) -> &'static str {
    match p.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase().as_str() {
        "html" | "htm" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript",
        "css" => "text/css",
        "json" => "application/json",
        "txt" => "text/plain; charset=utf-8",
        "glb" => "model/gltf-binary",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "mp4" => "video/mp4",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "m4a" => "audio/mp4",
        "wasm" => "application/wasm",
        _ => "application/octet-stream",
    }
}

fn percent_decode(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            let h = std::str::from_utf8(&b[i + 1..i + 3]).ok()?;
            out.push(u8::from_str_radix(h, 16).ok()?);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// The file a URL path names under `root` (None for anything that leaves it).
fn resolve(root: &Path, url: &str) -> Option<PathBuf> {
    let path = url.split(['?', '#']).next()?;
    let path = percent_decode(path)?;
    let mut p = root.to_path_buf();
    for c in path.split('/') {
        match c {
            "" | "." => {}
            ".." => return None,
            c if c.contains('\\') || c.contains('\0') => return None,
            c => p.push(c),
        }
    }
    if p.is_dir() {
        p.push("index.html");
    }
    Some(p)
}

/// `bytes=a-b`, `bytes=a-`, `bytes=-n` → (start, end inclusive); Err(()) = not satisfiable.
fn parse_range(h: &str, size: u64) -> Option<Result<(u64, u64), ()>> {
    let r = h.trim().strip_prefix("bytes=")?;
    let r = r.split(',').next()?.trim();
    let (a, b) = r.split_once('-')?;
    let (start, end) = if a.is_empty() {
        let n: u64 = b.parse().ok()?;
        (size.saturating_sub(n), size.saturating_sub(1))
    } else {
        let s: u64 = a.parse().ok()?;
        let e: u64 = if b.is_empty() { size.saturating_sub(1) } else { b.parse().ok()? };
        (s, e.min(size.saturating_sub(1)))
    };
    if size == 0 || start > end { Some(Err(())) } else { Some(Ok((start, end))) }
}

fn reply(s: &mut TcpStream, code: u16, reason: &str, headers: &[(String, String)], body: &[u8]) -> std::io::Result<()> {
    let mut h = format!("HTTP/1.1 {code} {reason}\r\nConnection: close\r\nAccept-Ranges: bytes\r\n");
    for (k, v) in headers {
        h.push_str(&format!("{k}: {v}\r\n"));
    }
    h.push_str("\r\n");
    s.write_all(h.as_bytes())?;
    s.write_all(body)
}

fn handle(root: &Path, mut s: TcpStream) -> std::io::Result<()> {
    let mut rd = BufReader::new(s.try_clone()?);
    let mut line = String::new();
    rd.read_line(&mut line)?;
    let mut parts = line.split_whitespace();
    let (method, url) = (parts.next().unwrap_or(""), parts.next().unwrap_or("/"));
    let mut range = None;
    loop {
        let mut h = String::new();
        if rd.read_line(&mut h)? == 0 || h.trim().is_empty() {
            break;
        }
        if let Some((k, v)) = h.split_once(':')
            && k.trim().eq_ignore_ascii_case("range")
        {
            range = Some(v.trim().to_string());
        }
    }
    if method != "GET" && method != "HEAD" {
        return reply(&mut s, 405, "Method Not Allowed", &[("Content-Length".into(), "0".into())], b"");
    }
    let Some(path) = resolve(root, url).filter(|p| p.is_file()) else {
        let body = b"404 not found";
        return reply(&mut s, 404, "Not Found", &[("Content-Type".into(), "text/plain".into()), ("Content-Length".into(), body.len().to_string())], body);
    };
    let mut f = std::fs::File::open(&path)?;
    let size = f.metadata()?.len();
    let ct = content_type(&path).to_string();
    let (code, reason, start, len, mut headers) = match range.as_deref().and_then(|r| parse_range(r, size)) {
        Some(Ok((a, b))) => (206, "Partial Content", a, b - a + 1, vec![("Content-Range".to_string(), format!("bytes {a}-{b}/{size}"))]),
        Some(Err(())) => {
            return reply(
                &mut s,
                416,
                "Requested Range Not Satisfiable",
                &[("Content-Range".into(), format!("bytes */{size}")), ("Content-Length".into(), "0".into())],
                b"",
            );
        }
        None => (200, "OK", 0, size, Vec::new()),
    };
    headers.push(("Content-Type".into(), ct));
    headers.push(("Content-Length".into(), len.to_string()));
    headers.push(("Cache-Control".into(), "no-cache".into()));
    reply(&mut s, code, reason, &headers, b"")?;
    if method == "HEAD" {
        return Ok(());
    }
    f.seek(SeekFrom::Start(start))?;
    let mut left = len;
    let mut buf = vec![0u8; 1 << 16];
    while left > 0 {
        let want = left.min(buf.len() as u64) as usize;
        let n = f.read(&mut buf[..want])?;
        if n == 0 {
            break;
        }
        // the browser may cancel a request while seeking: stop quietly
        if s.write_all(&buf[..n]).is_err() {
            break;
        }
        left -= n as u64;
    }
    Ok(())
}

pub fn serve(args: &[String]) -> Result<(), String> {
    let dir = args.iter().find(|a| !a.starts_with("--")).map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    let mut port = 8000u16;
    if let Some(i) = args.iter().position(|a| a == "--port") {
        port = args.get(i + 1).and_then(|p| p.parse().ok()).ok_or("--port needs a number")?;
    }
    let root = dir.canonicalize().map_err(|e| format!("{}: {e}", dir.display()))?;
    let l = TcpListener::bind(("127.0.0.1", port)).map_err(|e| format!("port {port}: {e}"))?;
    eprintln!("BR2 viewer: http://localhost:{port}  (Ctrl+C to stop)");
    for s in l.incoming() {
        let Ok(s) = s else { continue };
        let root = root.clone();
        std::thread::spawn(move || {
            let _ = handle(&root, s);
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges_and_paths() {
        assert_eq!(parse_range("bytes=0-99", 1000), Some(Ok((0, 99))));
        assert_eq!(parse_range("bytes=900-", 1000), Some(Ok((900, 999))));
        assert_eq!(parse_range("bytes=-100", 1000), Some(Ok((900, 999))));
        assert_eq!(parse_range("bytes=500-2000", 1000), Some(Ok((500, 999))));
        assert_eq!(parse_range("bytes=1000-", 1000), Some(Err(())));
        assert_eq!(parse_range("items=1-2", 1000), None);
        let root = Path::new("/srv");
        assert_eq!(resolve(root, "/a%20b/c.glb?v=3"), Some(PathBuf::from("/srv/a b/c.glb")));
        assert_eq!(resolve(root, "/../etc/passwd"), None);
    }
}
