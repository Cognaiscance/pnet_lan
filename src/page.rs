//! The address page on the record server.
//!
//! It lists this user's devices, then each contact's devices. A row can be
//! given an IPv4 or IPv6 address, or cleared. Saving writes the list and
//! bumps `updated_at`, which is what makes the push run. The page listens on
//! loopback only.

use std::io::{Read, Write};
use std::net::{IpAddr, TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::directory::Directory;
use crate::store::{Store, StoreError};

pub const DEFAULT_HTTP_PORT: u16 = 8794;

pub const PORTAL_SLUG: &str = "lan";
pub const PORTAL_TITLE: &str = "pNet LAN";

pub fn portal_base() -> String {
    match std::env::var("PNET_PORTAL") {
        Ok(value) if !value.is_empty() => value.trim_end_matches('/').to_string(),
        _ => "http://127.0.0.1:8777".to_string(),
    }
}

/// Register this page on the local portal. The same slug replaces an older mount.
pub fn register_portal(port: u16) -> Result<String, String> {
    let body = format!(
        "slug={PORTAL_SLUG}&port={port}&title={}",
        form_encode(PORTAL_TITLE)
    );
    let reply = post_portal("/api/app-web/register", &body)?;
    if !reply.starts_with("ok ") {
        return Err(reply);
    }
    Ok(format!(
        "Portal mount /apps/{PORTAL_SLUG}/ on {}",
        portal_base()
    ))
}

fn post_portal(path: &str, body: &str) -> Result<String, String> {
    let base = portal_base();
    let rest = base
        .strip_prefix("http://")
        .ok_or_else(|| format!("PNET_PORTAL '{base}' must be http"))?;
    let (host, port) = rest
        .rsplit_once(':')
        .ok_or_else(|| format!("PNET_PORTAL '{base}' has no port"))?;
    let port: u16 = port
        .parse()
        .map_err(|_| format!("PNET_PORTAL '{base}' has a bad port"))?;
    let mut last = "portal did not answer".to_string();
    for attempt in 0..5 {
        if attempt > 0 {
            std::thread::sleep(Duration::from_millis(200));
        }
        match post_once(host, port, path, body) {
            Ok(reply) => return Ok(reply),
            Err(err) => last = err,
        }
    }
    Err(last)
}

fn post_once(host: &str, port: u16, path: &str, body: &str) -> Result<String, String> {
    let mut stream =
        TcpStream::connect((host, port)).map_err(|err| format!("connect {host}:{port}: {err}"))?;
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));
    let request = format!(
        "POST {path} HTTP/1.1\r\nHost: {host}:{port}\r\nContent-Type: application/x-www-form-urlencoded\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream
        .write_all(request.as_bytes())
        .map_err(|err| format!("portal write: {err}"))?;
    let mut buf = String::new();
    stream
        .read_to_string(&mut buf)
        .map_err(|err| format!("portal read: {err}"))?;
    let Some((head, rest)) = buf.split_once("\r\n\r\n") else {
        return Err("portal reply had no header".to_string());
    };
    let status = head.lines().next().unwrap_or("");
    let code = status.split_whitespace().nth(1).unwrap_or("");
    if code != "200" {
        return Err(format!("portal {code}: {}", rest.trim()));
    }
    Ok(rest.trim().to_string())
}

fn form_encode(text: &str) -> String {
    let mut out = String::new();
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' => out.push(byte as char),
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

pub fn http_port() -> Result<u16, String> {
    let text = match std::env::var("PNET_LAN_HTTP_PORT") {
        Ok(value) if !value.is_empty() => value,
        _ => DEFAULT_HTTP_PORT.to_string(),
    };
    let port: u16 = text
        .parse()
        .map_err(|_| format!("PNET_LAN_HTTP_PORT '{text}' is not a port"))?;
    if port == 0 {
        return Err("PNET_LAN_HTTP_PORT must not be 0".to_string());
    }
    Ok(port)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub device: [u8; 16],
    pub heading: String,
    pub alias: String,
    pub address: Option<IpAddr>,
}

/// Own devices first, then each contact's devices. The address is the one
/// stored for that device, if any.
pub fn rows(dir: &Directory, store: &Store) -> Vec<Row> {
    let mut out = Vec::new();
    for device in &dir.own_devices {
        out.push(Row {
            device: device.id,
            heading: "Your devices".to_string(),
            alias: device.alias.clone(),
            address: stored(store, &device.id),
        });
    }
    for contact in &dir.contacts {
        for device in &contact.devices {
            out.push(Row {
                device: device.id,
                heading: contact.alias.clone(),
                alias: device.alias.clone(),
                address: stored(store, &device.id),
            });
        }
    }
    out
}

fn stored(store: &Store, device: &[u8; 16]) -> Option<IpAddr> {
    store
        .assignments()
        .iter()
        .find(|row| row.device == *device)
        .and_then(|row| row.address)
}

pub fn render(updated_at: u64, listed: &[Row], error: Option<&str>) -> String {
    let mut body = String::from(
        "<!DOCTYPE html>\n<html lang=\"en\" data-theme=\"dark\"><head><meta charset=\"utf-8\">\
         <meta name=\"theme-color\" content=\"#070605\"><title>pNet LAN</title>\n\
         <script>\
         (function () {\
           var theme = \"dark\";\
           try {\
             var saved = localStorage.getItem(\"pnet-theme\");\
             if (saved === \"light\" || saved === \"dark\") theme = saved;\
           } catch (e) {}\
           document.documentElement.setAttribute(\"data-theme\", theme);\
           var meta = document.querySelector('meta[name=\"theme-color\"]');\
           if (meta) meta.setAttribute(\"content\", theme === \"light\" ? \"#f3eee4\" : \"#070605\");\
         })();\
         </script>\n<style>",
    );
    body.push_str(PAGE_CSS);
    body.push_str(
        "</style></head><body>\n\
         <button class=\"theme-toggle\" type=\"button\">Light</button>\n\
         <main>\n<h1>Addresses</h1>\n\
         <p class=\"lead\">A device with no address is disabled. Enable it to assign an address.</p>\n\
         <p class=\"lead\">Updated <time data-ms=\"",
    );
    body.push_str(&updated_at.to_string());
    body.push_str("\">");
    body.push_str(&updated_at.to_string());
    body.push_str("</time>.</p>\n");
    if let Some(error) = error {
        body.push_str("<p class=\"error\">");
        body.push_str(&escape(error));
        body.push_str("</p>\n");
    }
    // Each post goes back to this page, including when the portal serves it
    // under /apps/lan/. A post names only the device being changed.
    let mut heading = None;
    for row in listed {
        if heading.as_deref() != Some(row.heading.as_str()) {
            if heading.is_some() {
                body.push_str("</tbody></table></section></section>\n");
            }
            body.push_str("<section class=\"person\"><h2>");
            body.push_str(&escape(&row.heading));
            body.push_str(
                "</h2>\n<section class=\"card\"><table><thead><tr>\
                 <th>Device</th><th>Address</th><th></th></tr></thead><tbody>\n",
            );
            heading = Some(row.heading.clone());
        }
        let id = hex_id(&row.device);
        let alias = escape(&row.alias);
        body.push_str("<tr><td>");
        body.push_str(&alias);
        body.push_str("</td>");
        match row.address {
            None => {
                body.push_str(
                    "<td class=\"off\">Disabled</td><td class=\"actions\">\
                     <button type=\"button\" class=\"enable\" data-device=\"",
                );
                body.push_str(&id);
                body.push_str("\" data-alias=\"");
                body.push_str(&alias);
                body.push_str("\">Enable</button></td>");
            }
            Some(address) => {
                let value = escape(&address.to_string());
                body.push_str("<td class=\"addr\">");
                body.push_str(&value);
                body.push_str(
                    "</td><td class=\"actions\">\
                     <button type=\"button\" class=\"edit\" data-device=\"",
                );
                body.push_str(&id);
                body.push_str("\" data-alias=\"");
                body.push_str(&alias);
                body.push_str("\" data-address=\"");
                body.push_str(&value);
                body.push_str(
                    "\">Edit</button>\
                     <form method=\"post\" action=\"\">\
                     <input type=\"hidden\" name=\"",
                );
                body.push_str(&id);
                body.push_str(
                    "\" value=\"\">\
                     <button class=\"ghost\" type=\"submit\">Disable</button></form></td>",
                );
            }
        }
        body.push_str("</tr>\n");
    }
    if heading.is_some() {
        body.push_str("</tbody></table></section></section>\n");
    }
    body.push_str(
        "<dialog id=\"address-dialog\">\
         <form method=\"post\" action=\"\">\
         <h2 id=\"dialog-title\">Address</h2>\
         <label for=\"dialog-address\">Address</label>\
         <input id=\"dialog-address\" name=\"\" value=\"\" autocomplete=\"off\" required>\
         <div class=\"dialog-actions\">\
         <button class=\"save\" type=\"submit\">Save</button>\
         <button class=\"ghost\" type=\"button\" id=\"dialog-cancel\">Cancel</button>\
         </div></form></dialog>\n\
         <p><a href=\"/\">Portal home</a></p>\n</main>\n<script>\
         (function () {\
           var dialog = document.getElementById(\"address-dialog\");\
           var title = document.getElementById(\"dialog-title\");\
           var input = document.getElementById(\"dialog-address\");\
           var cancel = document.getElementById(\"dialog-cancel\");\
           document.addEventListener(\"click\", function (ev) {\
             var btn = ev.target.closest && ev.target.closest(\"button.enable, button.edit\");\
             if (!btn || !dialog || !input) return;\
             var alias = btn.getAttribute(\"data-alias\") || \"device\";\
             var enabling = btn.classList.contains(\"enable\");\
             if (title) title.textContent = (enabling ? \"Enable \" : \"Edit \") + alias;\
             input.name = btn.getAttribute(\"data-device\") || \"\";\
             input.value = btn.getAttribute(\"data-address\") || \"\";\
             if (dialog.showModal) dialog.showModal();\
             input.focus();\
           });\
           if (cancel && dialog) cancel.addEventListener(\"click\", function () { dialog.close(); });\
         })();\
         (function () {\
           var key = \"pnet-theme\";\
           function current() {\
             return document.documentElement.getAttribute(\"data-theme\") === \"light\" ? \"light\" : \"dark\";\
           }\
           function paint(theme) {\
             document.documentElement.setAttribute(\"data-theme\", theme);\
             var meta = document.querySelector('meta[name=\"theme-color\"]');\
             if (meta) meta.setAttribute(\"content\", theme === \"light\" ? \"#f3eee4\" : \"#070605\");\
             var next = theme === \"dark\" ? \"light\" : \"dark\";\
             document.querySelectorAll(\".theme-toggle\").forEach(function (btn) {\
               btn.textContent = theme === \"dark\" ? \"Light\" : \"Dark\";\
               btn.setAttribute(\"aria-label\", \"Switch to \" + next + \" mode\");\
             });\
           }\
           paint(current());\
           document.addEventListener(\"click\", function (ev) {\
             var btn = ev.target.closest && ev.target.closest(\".theme-toggle\");\
             if (!btn) return;\
             var next = current() === \"dark\" ? \"light\" : \"dark\";\
             try { localStorage.setItem(key, next); } catch (e) {}\
             paint(next);\
           });\
           window.addEventListener(\"storage\", function (ev) {\
             if (ev.key === key && (ev.newValue === \"light\" || ev.newValue === \"dark\")) paint(ev.newValue);\
           });\
         })();\
         (function () {\
           var fmt = new Intl.DateTimeFormat(undefined, {\
             year: \"numeric\", month: \"2-digit\", day: \"2-digit\",\
             hour: \"2-digit\", minute: \"2-digit\", second: \"2-digit\",\
             hourCycle: \"h23\", timeZoneName: \"short\"\
           });\
           document.querySelectorAll(\"time[data-ms]\").forEach(function (el) {\
             var ms = Number(el.getAttribute(\"data-ms\"));\
             if (!Number.isFinite(ms)) return;\
             el.textContent = fmt.format(new Date(ms));\
           });\
         })();\
         </script>\n</body></html>\n",
    );
    body
}

const PAGE_CSS: &str = r#"
:root, html[data-theme="dark"] {
  color-scheme: dark;
  --bg: #070605; --bg-glow: #2a2218; --bg-elev: #14110d;
  --ink: #f4efe6; --muted: #b7aa96; --gold: #d4b15a; --gold-hover: #e4c56e;
  --gold-ink: #1a1408; --line: rgba(244, 239, 230, 0.12);
  --card-line: rgba(212, 177, 90, 0.32); --line-strong: rgba(212, 177, 90, 0.45);
  --shadow: 0 16px 36px rgba(0, 0, 0, 0.45);
  --bad: #e07a6a; --link: #e4c97a;
}
html[data-theme="light"] {
  color-scheme: light;
  --bg: #f3eee4; --bg-glow: #f0e0be; --bg-elev: #fffdf8;
  --ink: #241c14; --muted: #6b5844; --gold: #8d6818; --gold-hover: #6f5010;
  --gold-ink: #fffdf8; --line: rgba(36, 28, 20, 0.1);
  --card-line: rgba(141, 104, 24, 0.28); --line-strong: rgba(141, 104, 24, 0.38);
  --shadow: 0 12px 32px rgba(70, 48, 16, 0.08);
  --bad: #a3261c; --link: #8a6412;
}
* { box-sizing: border-box; }
html, body { margin: 0; min-height: 100%; }
body {
  background: radial-gradient(1100px 520px at 50% -12%, var(--bg-glow) 0%, transparent 58%), var(--bg);
  color: var(--ink);
  font-family: "Segoe UI", system-ui, -apple-system, sans-serif;
  line-height: 1.5;
}
h1, h2 { font-family: Georgia, "Iowan Old Style", Palatino, serif; font-weight: 600; }
h1 { margin: 0 0 0.4rem; font-size: 1.9rem; }
.person { margin: 1.6rem 0 0.2rem; }
.person > h2 { margin: 0; font-size: 1.45rem; }
a { color: var(--link); }
a:hover { color: var(--gold-hover); }
main { max-width: 56rem; margin: 0 auto; padding: 2rem 1.2rem 3rem; }
.lead { color: var(--muted); }
.error { color: var(--bad); font-weight: 600; }
.card {
  background: var(--bg-elev); border: 1px solid var(--card-line); border-radius: 14px;
  padding: 1.2rem 1.4rem; box-shadow: var(--shadow); margin: 1rem 0;
}
table { border-collapse: collapse; width: 100%; }
th, td { text-align: left; padding: .45rem .6rem; border-bottom: 1px solid var(--line); vertical-align: middle; }
th { color: var(--muted); font-weight: 600; font-size: .85rem; }
.off { color: var(--muted); }
.addr { font-variant-numeric: tabular-nums; }
td.actions { text-align: right; white-space: nowrap; }
td.actions form { display: inline; }
input {
  display: block; background: var(--bg); color: var(--ink); border: 1px solid var(--line-strong);
  border-radius: 8px; padding: .45rem .65rem; width: 100%; margin: .4rem 0 .8rem; font: inherit;
}
input:focus { outline: 2px solid var(--gold); border-color: var(--gold); }
button {
  background: transparent; color: var(--ink); border: 1px solid var(--line-strong);
  border-radius: 8px; font-weight: 600; padding: .28rem .7rem; cursor: pointer;
}
button:hover { color: var(--gold); border-color: var(--gold); }
button.theme-toggle { position: fixed; top: 1rem; right: 1rem; z-index: 2; }
button.save {
  background: var(--gold); color: var(--gold-ink); border: 0;
}
button.save:hover { background: var(--gold-hover); color: var(--gold-ink); }
dialog {
  background: var(--bg-elev); color: var(--ink); border: 1px solid var(--card-line);
  border-radius: 14px; box-shadow: var(--shadow); padding: 1.2rem 1.4rem; width: min(24rem, 92vw);
}
dialog::backdrop { background: rgba(0, 0, 0, 0.55); }
dialog h2 { margin: 0 0 .8rem; font-size: 1.35rem; }
.dialog-actions { display: flex; gap: .5rem; justify-content: flex-end; }
"#;

/// Apply a saved form. A bad address writes nothing. An unknown device is
/// ignored. An empty field clears a stored address.
pub fn apply_form(
    store: &mut Store,
    dir: &Directory,
    fields: &[(String, String)],
    now_ms: u64,
) -> Result<(), StoreError> {
    let known = rows(dir, store);
    let mut changes = Vec::new();
    for (name, value) in fields {
        let Some(device) = parse_id(name) else {
            continue;
        };
        if !known.iter().any(|row| row.device == device) {
            continue;
        }
        let address = if value.is_empty() {
            None
        } else {
            let Ok(address) = value.parse::<IpAddr>() else {
                return Err(StoreError::Corrupt("bad address"));
            };
            Some(address)
        };
        let current = stored(store, &device);
        if current != address {
            changes.push((device, address));
        }
    }
    for (device, address) in changes {
        store.assign(device, address, now_ms)?;
    }
    Ok(())
}

pub fn form_fields(body: &str) -> Vec<(String, String)> {
    let mut fields = Vec::new();
    if body.is_empty() {
        return fields;
    }
    for pair in body.split('&') {
        let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
        fields.push((percent_decode(name), percent_decode(value)));
    }
    fields
}

/// One HTTP request against the shared list. `GET /` renders it. `POST /`
/// saves it. A query string is ignored, so the portal iframe's `/?raw=1`
/// is the same page. Anything else is 404.
pub fn handle(
    method: &str,
    path: &str,
    body: &str,
    store: &mut Store,
    dir: &Directory,
    now_ms: u64,
) -> (u16, String) {
    let path = path.split_once('?').map(|(path, _)| path).unwrap_or(path);
    if path != "/" {
        return (404, "Not found".to_string());
    }
    match method {
        "GET" => (200, render(store.updated_at(), &rows(dir, store), None)),
        "POST" => match apply_form(store, dir, &form_fields(body), now_ms) {
            Ok(()) => (200, render(store.updated_at(), &rows(dir, store), None)),
            Err(_) => (
                400,
                render(
                    store.updated_at(),
                    &rows(dir, store),
                    Some("That address is not an IP address."),
                ),
            ),
        },
        _ => (405, "Method not allowed".to_string()),
    }
}

pub fn serve(
    listener: TcpListener,
    store: Arc<Mutex<Store>>,
    dir: Arc<Mutex<Directory>>,
) -> Result<(), String> {
    for stream in listener.incoming() {
        let mut stream = match stream {
            Ok(stream) => stream,
            Err(err) => return Err(format!("page accept: {err}")),
        };
        let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
        let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));
        if let Ok((method, path, body)) = read_request(&mut stream) {
            let now = unix_ms();
            let (status, page) = {
                let mut store = store.lock().map_err(|_| "address list lock".to_string())?;
                let dir = dir.lock().map_err(|_| "directory lock".to_string())?;
                handle(&method, &path, &body, &mut store, &dir, now)
            };
            let _ = write_response(&mut stream, status, &page);
        }
    }
    Ok(())
}

fn read_request(stream: &mut TcpStream) -> std::io::Result<(String, String, String)> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 1024];
    loop {
        let n = stream.read(&mut chunk)?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
        if buf.windows(4).any(|window| window == b"\r\n\r\n") || buf.len() > 65_536 {
            break;
        }
    }
    let text = String::from_utf8_lossy(&buf);
    let Some((head, rest)) = text.split_once("\r\n\r\n") else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "no header",
        ));
    };
    let mut lines = head.lines();
    let request = lines.next().unwrap_or("");
    let mut parts = request.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let path = parts.next().unwrap_or("").to_string();
    let length = lines
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            if name.eq_ignore_ascii_case("content-length") {
                value.trim().parse::<usize>().ok()
            } else {
                None
            }
        })
        .unwrap_or(0)
        .min(65_536);
    let mut body = rest.to_string();
    while body.len() < length {
        let n = stream.read(&mut chunk)?;
        if n == 0 {
            break;
        }
        body.push_str(&String::from_utf8_lossy(&chunk[..n]));
    }
    body.truncate(length);
    Ok((method, path, body))
}

fn write_response(stream: &mut TcpStream, status: u16, body: &str) -> std::io::Result<()> {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        405 => "Method Not Allowed",
        _ => "Error",
    };
    let bytes = body.as_bytes();
    write!(
        stream,
        "HTTP/1.1 {status} {reason}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        bytes.len()
    )?;
    stream.write_all(bytes)?;
    Ok(())
}

fn escape(text: &str) -> String {
    let mut out = String::new();
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(ch),
        }
    }
    out
}

fn hex_id(id: &[u8; 16]) -> String {
    id.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn parse_id(text: &str) -> Option<[u8; 16]> {
    if text.len() != 32 {
        return None;
    }
    let mut id = [0u8; 16];
    for (index, byte) in id.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&text[index * 2..index * 2 + 2], 16).ok()?;
    }
    Some(id)
}

fn percent_decode(text: &str) -> String {
    let text = text.replace('+', " ");
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            if let Ok(byte) = u8::from_str_radix(
                std::str::from_utf8(&bytes[index + 1..index + 3]).unwrap_or(""),
                16,
            ) {
                out.push(byte);
                index += 3;
                continue;
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::directory::{ContactRecord, DeviceRecord, Grade};
    use crate::push::needs_push;
    use std::net::{Ipv4Addr, Ipv6Addr};

    fn device(id: u8, alias: &str) -> DeviceRecord {
        DeviceRecord {
            id: [id; 16],
            alias: alias.into(),
            grade: Grade::Device,
            sg_rank: 0,
            apps: Vec::new(),
        }
    }

    fn directory() -> Directory {
        Directory {
            local_app_id: [9; 16],
            local_app_alias: "pnet-lan-server".into(),
            local_app_approved: true,
            token: [2; 16],
            local_device: [1; 16],
            owner_alias: "william".into(),
            owner_id: [7; 16],
            own_devices: vec![device(1, "n64"), device(3, "laptop <home>")],
            contacts: vec![ContactRecord {
                alias: "chad".into(),
                id: [6; 16],
                devices: vec![device(4, "phone")],
            }],
        }
    }

    fn open() -> (std::path::PathBuf, Store) {
        let dir = std::env::temp_dir().join(format!(
            "pnet-lan-page-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let store = Store::open(&dir).unwrap();
        (dir, store)
    }

    #[test]
    fn the_page_lists_own_devices_then_each_contact() {
        let (dir, mut store) = open();
        store.note_client([3; 16]).unwrap();
        store.set_received([3; 16], 40).unwrap();
        store
            .assign([3; 16], Some(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2))), 40)
            .unwrap();
        let directory = directory();
        let (status, html) = handle("GET", "/", "", &mut store, &directory, 50);
        assert_eq!(status, 200);
        let own = html.find("Your devices").unwrap();
        let chad = html.find(">chad<").unwrap();
        let laptop = html.find("laptop &lt;home&gt;").unwrap();
        let phone = html.find("phone").unwrap();
        assert!(own < laptop && laptop < chad && chad < phone);
        assert!(html.contains("10.0.0.2"));
        assert!(html.contains(">Edit<"));
        assert!(html.contains(">Disable<"));
        assert!(html.contains(">Enable<"));
        assert!(html.contains("Disabled"));
        assert!(html.contains("address-dialog"));
        assert!(html.contains("action=\"\""));
        assert!(html.contains("pnet-theme"));
        assert!(html.contains("data-theme=\"dark\""));
        assert!(html.contains("theme-toggle"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn saving_an_address_bumps_the_list_and_needs_a_push() {
        let (dir, mut store) = open();
        store.note_client([3; 16]).unwrap();
        store.set_received([3; 16], 40).unwrap();
        let directory = directory();
        let phone = hex_id(&[4; 16]);
        let body = format!("{phone}=fd00%3A%3A2");
        let (status, html) = handle("POST", "/", &body, &mut store, &directory, 90);
        assert_eq!(status, 200);
        assert!(html.contains("fd00::2"));
        assert_eq!(store.updated_at(), 90);
        assert_eq!(
            store.assignments()[0].address,
            Some(IpAddr::V6(Ipv6Addr::new(0xfd00, 0, 0, 0, 0, 0, 0, 2)))
        );
        assert!(needs_push(&store, &[3; 16]));
        let again = format!("{phone}=fd00%3A%3A2");
        let _ = handle("POST", "/", &again, &mut store, &directory, 120);
        assert_eq!(store.updated_at(), 90);
        let clear = format!("{phone}=");
        handle("POST", "/", &clear, &mut store, &directory, 130);
        assert_eq!(store.assignments()[0].address, None);
        assert_eq!(store.updated_at(), 130);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_bad_address_does_not_change_the_list() {
        let (dir, mut store) = open();
        let directory = directory();
        let laptop = hex_id(&[3; 16]);
        let (status, html) = handle(
            "POST",
            "/",
            &format!("{laptop}=not-an-ip"),
            &mut store,
            &directory,
            80,
        );
        assert_eq!(status, 400);
        assert!(html.contains("That address is not an IP address."));
        assert!(store.assignments().is_empty());
        assert_eq!(store.updated_at(), 0);
        let (status, _) = handle("GET", "/nope", "", &mut store, &directory, 80);
        assert_eq!(status, 404);
        let (status, html) = handle("GET", "/?raw=1", "", &mut store, &directory, 80);
        assert_eq!(status, 200);
        assert!(html.contains("<h1>Addresses</h1>"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_loopback_post_saves_the_address() {
        let (dir, store) = open();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let store = Arc::new(Mutex::new(store));
        let directory = Arc::new(Mutex::new(directory()));
        let page_store = Arc::clone(&store);
        let page_dir = Arc::clone(&directory);
        std::thread::spawn(move || {
            let _ = serve(listener, page_store, page_dir);
        });
        let laptop = hex_id(&[3; 16]);
        let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
        write!(
            stream,
            "POST / HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: {}\r\n\r\n{laptop}=10.1.0.8",
            format!("{laptop}=10.1.0.8").len()
        )
        .unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        assert!(response.contains("200"));
        assert!(response.contains("10.1.0.8"));
        let saved = store.lock().unwrap();
        assert_eq!(
            saved.assignments()[0].address,
            Some(IpAddr::V4(Ipv4Addr::new(10, 1, 0, 8)))
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
