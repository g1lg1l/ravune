//! OSC in (ported from Velo): a UDP port (9000 by default; off until Preferences → MIDI) with
//! an address space under /ravune, for TouchOSC on a phone, a hardware controller's editor or
//! any tool in any language. Messages are stamped as they arrive on their own thread and
//! applied on the UI thread once a frame, through the same paths as MIDI learn (faders coalesce
//! to their last value, every trigger counts).
//!
//! ```text
//! /ravune/play                         play or stop
//! /ravune/stop                         stop, if playing
//! /ravune/record                       record what plays
//! /ravune/tempo/bpm            f       the tempo
//! /ravune/macro/<n>            f 0..1  macro n (1…)
//! /ravune/mixer/<n>/volume     f 0..1  mixer track n (0 is the master), along its fader
//! /ravune/mixer/<n>/pan        f 0..1  left to right
//! /ravune/channel/<n>/volume   f 0..1  rack channel n (1…)
//! /ravune/pattern              i       pattern n (1…)
//! /ravune/op/<operator>        [s]     any operator, its props as JSON
//! ```

use std::net::UdpSocket;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

use eframe::egui::{self, RichText, Ui};
use ravune_core::Target;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::theme::Theme;
use crate::{App, icons, kit};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    pub enabled: bool,
    pub port: u16,
}

impl Default for Prefs {
    fn default() -> Self {
        Self { enabled: false, port: 9000 }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Arg {
    Int(i64),
    Float(f64),
    Str(String),
    Bool(bool),
    /// Nil, impulse, a blob: there, but no value.
    Other,
}

impl Arg {
    fn number(&self) -> Option<f64> {
        match self {
            Arg::Int(i) => Some(*i as f64),
            Arg::Float(f) => Some(*f),
            Arg::Bool(b) => Some(f64::from(u8::from(*b))),
            Arg::Str(s) => s.trim().parse().ok(),
            Arg::Other => None,
        }
    }
}

/// One message as it came.
#[derive(Clone, Debug, PartialEq)]
pub struct Message {
    pub address: String,
    pub args: Vec<Arg>,
    pub at: Instant,
}

/// A padded OSC string at `*at`, moving past it.
fn string(b: &[u8], at: &mut usize) -> Option<String> {
    let len = b.get(*at..)?.iter().position(|&c| c == 0)?;
    let s = String::from_utf8_lossy(&b[*at..*at + len]).into_owned();
    *at += (len + 4) & !3;
    Some(s)
}

fn word<const N: usize>(b: &[u8], at: &mut usize) -> Option<[u8; N]> {
    let w = b.get(*at..*at + N)?.try_into().ok()?;
    *at += N;
    Some(w)
}

/// A packet's messages: one, or a bundle's (nested bundles too). What doesn't parse is dropped.
pub fn decode(b: &[u8]) -> Vec<(String, Vec<Arg>)> {
    let mut out = Vec::new();
    decode_into(b, &mut out, 0);
    out
}

fn decode_into(b: &[u8], out: &mut Vec<(String, Vec<Arg>)>, depth: usize) {
    if b.starts_with(b"#bundle\0") {
        // the time tag is ignored: everything applies on the next frame
        let mut at = 16;
        while depth < 8
            && let Some(size) = word::<4>(b, &mut at).map(|w| u32::from_be_bytes(w) as usize)
        {
            let Some(element) = b.get(at..at + size) else { break };
            decode_into(element, out, depth + 1);
            at += size;
        }
        return;
    }
    let mut at = 0;
    let Some(address) = string(b, &mut at).filter(|a| a.starts_with('/')) else { return };
    let tags = if b.get(at) == Some(&b',') { string(b, &mut at).unwrap_or_default() } else { String::new() };
    let mut args = Vec::new();
    for tag in tags.chars().skip(1) {
        let arg = match tag {
            'i' => word::<4>(b, &mut at).map(|w| Arg::Int(i64::from(i32::from_be_bytes(w)))),
            'f' => word::<4>(b, &mut at).map(|w| Arg::Float(f64::from(f32::from_be_bytes(w)))),
            'h' => word::<8>(b, &mut at).map(|w| Arg::Int(i64::from_be_bytes(w))),
            'd' => word::<8>(b, &mut at).map(|w| Arg::Float(f64::from_be_bytes(w))),
            's' | 'S' => string(b, &mut at).map(Arg::Str),
            'T' => Some(Arg::Bool(true)),
            'F' => Some(Arg::Bool(false)),
            'b' => word::<4>(b, &mut at).map(|w| {
                at += (u32::from_be_bytes(w) as usize + 3) & !3;
                Arg::Other
            }),
            't' => word::<8>(b, &mut at).map(|_| Arg::Other),
            'c' | 'r' | 'm' => word::<4>(b, &mut at).map(|_| Arg::Other),
            _ => Some(Arg::Other),
        };
        match arg {
            Some(a) => args.push(a),
            None => return, // cut short
        }
    }
    out.push((address, args));
}

/// The port's thread and what it heard.
pub struct Osc {
    port: u16,
    rx: Receiver<Message>,
    stop: Arc<AtomicBool>,
    /// The last address and when it came (Preferences shows it), and whether Ravune knew it.
    last: Option<(String, Instant, bool)>,
}

impl Drop for Osc {
    fn drop(&mut self) {
        self.stop.store(true, Relaxed);
    }
}

impl Osc {
    /// Listens on `port` on every interface (a phone on the same network sends to it).
    fn start(port: u16, ctx: &egui::Context) -> Result<Self, String> {
        let socket = UdpSocket::bind(("0.0.0.0", port)).map_err(|e| e.to_string())?;
        socket.set_read_timeout(Some(Duration::from_millis(200))).map_err(|e| e.to_string())?;
        let (tx, rx) = channel();
        let stop = Arc::new(AtomicBool::new(false));
        let (flag, ctx) = (stop.clone(), ctx.clone());
        std::thread::Builder::new()
            .name("osc".into())
            .spawn(move || {
                let mut buf = vec![0u8; 65_536];
                while !flag.load(Relaxed) {
                    let Ok((n, _)) = socket.recv_from(&mut buf) else { continue };
                    let at = Instant::now();
                    for (address, args) in decode(&buf[..n]) {
                        if tx.send(Message { address, args, at }).is_err() {
                            return;
                        }
                    }
                    ctx.request_repaint();
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(Self { port, rx, stop, last: None })
    }
}

/// This computer's address on the local network (what a phone sends to), when it has one.
fn local_ip() -> Option<String> {
    // connecting a UDP socket sends nothing; it only picks the interface a packet would leave by
    let s = UdpSocket::bind(("0.0.0.0", 0)).ok()?;
    s.connect(("192.0.2.1", 9)).ok()?;
    s.local_addr().ok().map(|a| a.ip().to_string()).filter(|ip| ip != "0.0.0.0")
}

/// A continuous address (its last value in a frame is all that matters).
fn continuous(address: &str) -> bool {
    ["/ravune/macro/", "/ravune/mixer/", "/ravune/channel/"].iter().any(|p| address.starts_with(p)) || address == "/ravune/tempo/bpm"
}

/// The messages of one frame worth applying: a fader's last position only, every trigger.
fn worth(messages: &[Message]) -> Vec<bool> {
    messages.iter().enumerate().map(|(i, m)| !continuous(&m.address) || !messages[i + 1..].iter().any(|n| n.address == m.address)).collect()
}

impl App {
    /// Keeps the port in step with the preferences; applies what came since the last frame.
    pub fn poll_osc(&mut self, ctx: &egui::Context) {
        let prefs = self.settings.osc.clone();
        if self.osc.as_ref().is_some_and(|o| !prefs.enabled || o.port != prefs.port) {
            self.osc = None;
        }
        if prefs.enabled && self.osc.is_none() {
            match Osc::start(prefs.port, ctx) {
                Ok(osc) => self.osc = Some(osc),
                Err(e) => {
                    self.settings.osc.enabled = false;
                    self.say(tr!("OSC can't listen on port {}: {}", prefs.port, e));
                }
            }
        }
        let Some(osc) = &self.osc else { return };
        let messages: Vec<Message> = osc.rx.try_iter().collect();
        let keep = worth(&messages);
        let mut last = None;
        for (m, keep) in messages.into_iter().zip(keep) {
            if keep {
                let known = self.apply_osc(&m);
                last = Some((m.address, m.at, known));
            }
        }
        if let (Some(last), Some(osc)) = (last, &mut self.osc) {
            osc.last = Some(last);
        }
    }

    /// One message: true when the address means something to Ravune.
    fn apply_osc(&mut self, m: &Message) -> bool {
        let parts: Vec<&str> = m.address.trim_matches('/').split('/').collect();
        let number = m.args.first().and_then(Arg::number);
        let text = m.args.first().and_then(|a| if let Arg::Str(s) = a { Some(s.clone()) } else { None });
        let norm = number.map(|k| k.clamp(0.0, 1.0) as f32);
        let p = &self.session.project;
        // a fader or knob, as MIDI learn moves it
        let target = match parts.as_slice() {
            ["ravune", "macro", n] => n.parse::<usize>().ok().filter(|n| (1..=p.macros.len()).contains(n)).map(|n| Target::Macro { index: (n - 1) as u8 }),
            ["ravune", "mixer", n, "volume"] => n.parse::<usize>().ok().filter(|n| *n < p.mixer.len()).map(|track| Target::TrackGain { track }),
            ["ravune", "mixer", n, "pan"] => n.parse::<usize>().ok().filter(|n| *n < p.mixer.len()).map(|track| Target::TrackPan { track }),
            ["ravune", "channel", n, "volume"] => n.parse::<usize>().ok().and_then(|n| p.channels.get(n.checked_sub(1)?)).map(|c| Target::ChannelGain { slot: c.slot }),
            _ => None,
        };
        if let Some(target) = target {
            let Some(k) = norm else { return false };
            self.set_target(target, k, &format!("osc {}", m.address));
            return true;
        }
        match parts.as_slice() {
            ["ravune", "play"] => self.command("transport.play_toggle"),
            ["ravune", "stop"] if self.session.playing => self.command("transport.play_toggle"),
            ["ravune", "stop"] => {}
            ["ravune", "record"] => self.command("app.record"),
            ["ravune", "tempo", "bpm"] => match number {
                Some(bpm) => self.run_merge("transport.set_tempo", json!({ "bpm": bpm }), "osc tempo"),
                None => return false,
            },
            ["ravune", "pattern"] => match number {
                Some(n) if n >= 1.0 && (n as usize) <= self.session.project.patterns.len() => self.run("pattern.select", json!({ "pattern": n as usize - 1 })),
                _ => return false,
            },
            ["ravune", "op", id] => {
                if ravune_core::find(id).is_none() {
                    return false;
                }
                let props = text.and_then(|t| serde_json::from_str::<Value>(&t).ok()).unwrap_or(json!({}));
                self.run(id, props);
            }
            _ => return false,
        }
        true
    }

    /// Preferences → MIDI's OSC part.
    pub fn prefs_osc(&mut self, ui: &mut Ui, t: &Theme) {
        kit::section(ui, t, tr!("OSC"), false, |ui| {
            let o = &mut self.settings.osc;
            ui.checkbox(&mut o.enabled, tr!("Listen for OSC (TouchOSC, controller editors, any tool that sends it)"));
            ui.add_enabled_ui(o.enabled, |ui| {
                ui.horizontal(|ui| {
                    ui.label(tr!("Port"));
                    ui.add(egui::DragValue::new(&mut o.port).range(1024..=65535));
                });
            });
            let status = match (&self.osc, local_ip()) {
                (Some(osc), ip) => {
                    let to = ip.map_or_else(|| format!("port {}", osc.port), |ip| format!("{ip}:{}", osc.port));
                    match &osc.last {
                        Some((address, at, known)) => tr!("Send to {} · last: {} {} s ago{}", to, address, at.elapsed().as_secs(), if *known { "" } else { " (?)" }),
                        None => tr!("Send to {} · nothing yet", to),
                    }
                }
                (None, _) => tr!("Off").to_string(),
            };
            ui.label(RichText::new(format!("{}  {status}", icons::GLOBE)).color(if self.osc.is_some() { t.select } else { t.fg_muted }));
        });
        kit::section(ui, t, tr!("OSC ADDRESSES"), false, |ui| {
            let addresses = [
                ("/ravune/play", tr!("Plays or stops")),
                ("/ravune/stop", tr!("Stops")),
                ("/ravune/record", tr!("Records what plays")),
                ("/ravune/tempo/bpm f", tr!("Sets the tempo")),
                ("/ravune/macro/1 f", tr!("A macro, 0 to 1")),
                ("/ravune/mixer/1/volume f", tr!("A mixer track's fader, 0 to 1 (0 is the master)")),
                ("/ravune/mixer/1/pan f", tr!("A mixer track's pan, 0 left to 1 right")),
                ("/ravune/channel/1/volume f", tr!("A rack channel's volume, 0 to 1")),
                ("/ravune/pattern i", tr!("Picks a pattern, by number")),
                ("/ravune/op/<operator> s", tr!("Runs an operator, its settings as JSON")),
            ];
            egui::Grid::new("osc addresses").num_columns(2).spacing([t.pad * 2.0, t.gap + 2.0]).show(ui, |ui| {
                for (address, what) in addresses {
                    ui.add(egui::Label::new(RichText::new(address).font(Theme::mono(Theme::TEXT_XS)).color(t.fg_text)).wrap_mode(egui::TextWrapMode::Extend));
                    ui.label(RichText::new(what).small().color(t.fg_muted));
                    ui.end_row();
                }
            });
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An OSC message as TouchOSC sends it.
    fn message(address: &str, args: &[Arg]) -> Vec<u8> {
        let pad = |b: &mut Vec<u8>, s: &[u8]| {
            b.extend_from_slice(s);
            b.push(0);
            while !b.len().is_multiple_of(4) {
                b.push(0);
            }
        };
        let mut b = Vec::new();
        pad(&mut b, address.as_bytes());
        let tags: String = std::iter::once(',')
            .chain(args.iter().map(|a| match a {
                Arg::Int(_) => 'i',
                Arg::Float(_) => 'f',
                Arg::Str(_) => 's',
                Arg::Bool(true) => 'T',
                Arg::Bool(false) => 'F',
                Arg::Other => 'N',
            }))
            .collect();
        pad(&mut b, tags.as_bytes());
        for a in args {
            match a {
                Arg::Int(i) => b.extend_from_slice(&(*i as i32).to_be_bytes()),
                Arg::Float(f) => b.extend_from_slice(&(*f as f32).to_be_bytes()),
                Arg::Str(s) => pad(&mut b, s.as_bytes()),
                _ => {}
            }
        }
        b
    }

    #[test]
    fn messages_and_bundles_decode() {
        let one = message("/ravune/macro/2", &[Arg::Float(0.5)]);
        assert_eq!(decode(&one), [("/ravune/macro/2".to_string(), vec![Arg::Float(0.5)])]);
        let two = message("/ravune/op/pattern.add", &[Arg::Str("{}".into()), Arg::Int(3), Arg::Bool(true)]);
        assert_eq!(decode(&two)[0].1, [Arg::Str("{}".into()), Arg::Int(3), Arg::Bool(true)]);
        // a bundle of both
        let mut bundle = b"#bundle\0".to_vec();
        bundle.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 1]);
        for m in [&one, &two] {
            bundle.extend_from_slice(&(m.len() as u32).to_be_bytes());
            bundle.extend_from_slice(m);
        }
        assert_eq!(decode(&bundle).len(), 2);
        // no type tags (old senders), cut short, not OSC
        assert_eq!(decode(&message("/ravune/play", &[])), [("/ravune/play".to_string(), vec![])]);
        assert!(decode(&one[..one.len() - 2]).is_empty());
        assert!(decode(b"hello").is_empty());
    }

    #[test]
    fn faders_keep_their_last_value_triggers_all_count() {
        let at = Instant::now();
        let m = |a: &str| Message { address: a.into(), args: vec![], at };
        let frame = [m("/ravune/macro/1"), m("/ravune/play"), m("/ravune/macro/1"), m("/ravune/play"), m("/ravune/mixer/0/volume")];
        assert_eq!(worth(&frame), [false, true, true, true, true]);
    }
}
