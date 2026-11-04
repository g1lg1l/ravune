//! The app's own load, for the status bar (ported from Velo): the CPU time and memory the
//! system says it uses, and on Windows how busy it keeps the GPU and the video memory it holds,
//! read a second apart, with the last half minute of each for its sparkline; and the chip that
//! shows one. No crate: one call or one file per platform.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use eframe::egui::{self, Mesh, Painter, Pos2, Rect, Response, Sense, Shape, Stroke, StrokeKind, Ui, pos2, vec2};

use crate::kit;
use crate::theme::Theme;

/// How often the numbers change: the bar shouldn't flicker.
const EVERY: Duration = Duration::from_secs(1);
/// How many readings a trail keeps: half a minute.
pub const TRAIL: usize = 30;

pub struct Usage {
    last: Option<(Instant, f64)>,
    /// Of the whole machine, 0..100.
    pub cpu: Option<f32>,
    /// Resident memory, bytes, and all the machine has.
    pub memory: Option<u64>,
    pub memory_total: Option<u64>,
    /// The GPU's 3D engine, as Task Manager counts it for this process, 0..100 (Windows).
    pub gpu: Option<f32>,
    /// Video memory held, bytes, and what the system lets this process hold (Windows).
    pub vram: Option<u64>,
    pub vram_budget: Option<u64>,
    /// Each reading's last half minute, oldest first, as shares of their whole (0..1).
    pub trails: Trails,
    #[cfg(windows)]
    counters: Option<gpu::Counters>,
}

#[derive(Default)]
pub struct Trails {
    pub cpu: VecDeque<f32>,
    pub memory: VecDeque<f32>,
    pub gpu: VecDeque<f32>,
    pub vram: VecDeque<f32>,
}

fn push(trail: &mut VecDeque<f32>, value: Option<f32>) {
    let Some(value) = value else { return };
    if trail.len() == TRAIL {
        trail.pop_front();
    }
    trail.push_back(value);
}

/// `part` of `whole`, 0..1.
fn share(part: Option<u64>, whole: Option<u64>) -> Option<f32> {
    Some((part? as f64 / whole.filter(|w| *w > 0)? as f64).clamp(0.0, 1.0) as f32)
}

impl Usage {
    pub fn new() -> Self {
        Self {
            last: None,
            cpu: None,
            memory: None,
            memory_total: total_memory(),
            gpu: None,
            vram: None,
            vram_budget: None,
            trails: Trails::default(),
            #[cfg(windows)]
            counters: gpu::Counters::open(),
        }
    }

    /// Reads the system again if a second has gone by.
    pub fn sample(&mut self) {
        let now = Instant::now();
        if self.last.is_some_and(|(at, _)| now - at < EVERY) {
            return;
        }
        let Some((cpu_s, memory)) = read() else { return };
        if let Some((at, before)) = self.last {
            let cores = std::thread::available_parallelism().map_or(1, |n| n.get()) as f64;
            self.cpu = Some(((cpu_s - before) / (now - at).as_secs_f64() / cores * 100.0).clamp(0.0, 100.0) as f32);
        }
        self.memory = memory;
        self.last = Some((now, cpu_s));
        #[cfg(windows)]
        {
            self.gpu = self.counters.as_ref().and_then(gpu::Counters::busy);
            (self.vram, self.vram_budget) = gpu::vram().unzip();
        }
        let t = &mut self.trails;
        push(&mut t.cpu, self.cpu.map(|c| c / 100.0));
        push(&mut t.memory, share(self.memory, self.memory_total));
        push(&mut t.gpu, self.gpu.map(|g| g / 100.0));
        push(&mut t.vram, share(self.vram, self.vram_budget));
    }
}

/// One load as the status bar shows it: what, its value, how near its limit it runs.
pub struct Load {
    pub name: &'static str,
    /// The value now, as `format` writes it; `room` is as wide as it gets, so the chips beside
    /// it don't move as it counts.
    pub value: f32,
    pub format: fn(f32) -> String,
    pub room: &'static str,
    /// How near its limit it runs (0..1): amber past `warn`, red past `high`, as a meter.
    pub strain: f32,
    pub warn: f32,
    pub high: f32,
    /// Its last half minute (0..1, oldest first) for the sparkline.
    pub trail: Vec<f32>,
}

/// A load as a chip, square as the meters: a light in the meter's colors, its name, the last
/// half minute as a sparkline, its value.
pub fn chip(ui: &mut Ui, t: &Theme, load: &Load) -> Response {
    let ctx = ui.ctx().clone();
    let id = egui::Id::unique(("load", load.name));
    let value = ctx.animate_value_with_time(id.with("value"), load.value, 0.6);
    let last = ctx.animate_value_with_time(id.with("last"), load.trail.last().copied().unwrap_or(0.0), 0.6);
    let level = if load.strain >= load.high {
        2.0
    } else if load.strain >= load.warn {
        1.0
    } else {
        0.0
    };
    let k = ctx.animate_value_with_time(id.with("level"), level, 0.3);
    let color = if k <= 1.0 { Theme::mix(t.meter_ok, t.meter_warn, k) } else { Theme::mix(t.meter_warn, t.meter_hot, k - 1.0) };
    let painter = ui.painter();
    let name = painter.layout_no_wrap(load.name.into(), Theme::mono(Theme::TEXT_XS), t.fg_muted);
    let font = Theme::mono(Theme::TEXT_XS);
    let text = painter.layout_no_wrap((load.format)(value), font.clone(), Theme::mix(t.fg_text, color, k.min(1.0)));
    let room = painter.layout_no_wrap(load.room.into(), font, t.fg_text).size().x.max(text.size().x);
    let (pad, gap, spark) = (6.0, 5.0, vec2(30.0, 10.0));
    let width = pad + 5.0 + gap + name.size().x + gap + spark.x + gap + room + pad;
    let (rect, resp) = ui.allocate_exact_size(vec2(width, 18.0), Sense::hover());
    let h = kit::hover(ui, t, &resp);
    let painter = ui.painter();
    let fill = Theme::mix(Theme::mix(t.bg_panel, t.fg_text, 0.05 * h), color, 0.08 * k);
    painter.rect(rect, t.radius_sm, fill, Stroke::new(1.0, Theme::mix(t.line, color, 0.45 * k.min(1.0))), StrokeKind::Inside);
    // the light, blinking while it's red
    let y = rect.center().y;
    let light = Rect::from_center_size(pos2(rect.left() + pad + 2.5, y), vec2(5.0, 5.0));
    let blink = level >= 2.0 && t.motion > 0.0 && (ui.input(|i| i.time) * 3.0).fract() > 0.5;
    if level >= 2.0 && t.motion > 0.0 {
        ctx.request_repaint_after(Duration::from_millis(100));
    }
    painter.rect_filled(light, 0, if blink { color.gamma_multiply(0.35) } else { color });
    let x = light.right() + gap;
    painter.galley(pos2(x, y - name.size().y / 2.0), name.clone(), t.fg_muted);
    let x = x + name.size().x + gap;
    sparkline(painter, t, Rect::from_min_size(pos2(x, y - spark.y / 2.0), spark), &load.trail, last, Theme::mix(t.fg_muted, color, 0.35 + 0.65 * k.min(1.0)));
    // the value, right-aligned in its room
    painter.galley(pos2(rect.right() - pad - text.size().x, y - text.size().y / 2.0), text, t.fg_text);
    resp
}

/// A trail of readings (0..1, oldest first, the newest at `last` as it eases there) as a line
/// with a fading area under it, from the right edge.
fn sparkline(painter: &Painter, t: &Theme, rect: Rect, trail: &[f32], last: f32, color: egui::Color32) {
    painter.rect_filled(rect.expand2(vec2(2.0, 1.0)), 0, t.bg_field);
    let step = rect.width() / (TRAIL - 1) as f32;
    let n = trail.len();
    let points: Vec<Pos2> = trail
        .iter()
        .enumerate()
        .map(|(i, v)| {
            let v = if i + 1 == n { last } else { *v };
            pos2(rect.right() - (n - 1 - i) as f32 * step, rect.bottom() - v.clamp(0.0, 1.0) * rect.height())
        })
        .collect();
    let Some(&end) = points.last() else { return };
    let mut mesh = Mesh::default();
    for (i, p) in points.iter().enumerate() {
        mesh.colored_vertex(*p, color.gamma_multiply(0.3));
        mesh.colored_vertex(pos2(p.x, rect.bottom()), color.gamma_multiply(0.02));
        if i > 0 {
            let k = 2 * i as u32;
            mesh.add_triangle(k - 2, k - 1, k);
            mesh.add_triangle(k - 1, k + 1, k);
        }
    }
    painter.add(Shape::mesh(mesh));
    painter.add(Shape::line(points, Stroke::new(1.25, color)));
    painter.rect_filled(Rect::from_center_size(end, vec2(3.0, 3.0)), 0, color);
}

#[cfg(windows)]
mod gpu {
    use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, DXGI_MEMORY_SEGMENT_GROUP_LOCAL, DXGI_QUERY_VIDEO_MEMORY_INFO, IDXGIAdapter3, IDXGIFactory1};
    use windows::Win32::System::Performance::{PDH_FMT_COUNTERVALUE_ITEM_W, PDH_FMT_DOUBLE, PDH_HCOUNTER, PDH_HQUERY, PDH_MORE_DATA, PdhAddEnglishCounterW, PdhCloseQuery, PdhCollectQueryData, PdhGetFormattedCounterArrayW, PdhOpenQueryW};
    use windows::core::{HSTRING, Interface};

    /// The GPU engines' load counters (Task Manager's), one query kept open: each read is the
    /// load since the one before.
    pub struct Counters {
        query: PDH_HQUERY,
        counter: PDH_HCOUNTER,
    }

    impl Counters {
        pub fn open() -> Option<Self> {
            let (mut query, mut counter) = (PDH_HQUERY(std::ptr::null_mut()), PDH_HCOUNTER(std::ptr::null_mut()));
            // SAFETY: out-parameters PDH fills; the query is closed if the counter can't be had
            unsafe {
                if PdhOpenQueryW(None, 0, &mut query) != 0 {
                    return None;
                }
                if PdhAddEnglishCounterW(query, &HSTRING::from("\\GPU Engine(*)\\Utilization Percentage"), 0, &mut counter) != 0 {
                    PdhCloseQuery(query);
                    return None;
                }
                PdhCollectQueryData(query);
            }
            Some(Self { query, counter })
        }

        /// How busy this process keeps the 3D engines, 0..100.
        pub fn busy(&self) -> Option<f32> {
            let pid = format!("pid_{}_", std::process::id());
            // SAFETY: the buffer is as large as PDH asks, 8-aligned, and read within the count it gives
            unsafe {
                PdhCollectQueryData(self.query);
                let (mut size, mut count) = (0u32, 0u32);
                if PdhGetFormattedCounterArrayW(self.counter, PDH_FMT_DOUBLE, &mut size, &mut count, None) != PDH_MORE_DATA {
                    return None;
                }
                let mut buffer = vec![0u64; (size as usize).div_ceil(8)];
                let items = buffer.as_mut_ptr().cast::<PDH_FMT_COUNTERVALUE_ITEM_W>();
                if PdhGetFormattedCounterArrayW(self.counter, PDH_FMT_DOUBLE, &mut size, &mut count, Some(items)) != 0 {
                    return None;
                }
                let busy: f64 = std::slice::from_raw_parts(items, count as usize)
                    .iter()
                    .filter(|i| i.FmtValue.CStatus == 0 && i.szName.to_string().is_ok_and(|n| n.starts_with(&pid) && n.ends_with("engtype_3D")))
                    .map(|i| i.FmtValue.Anonymous.doubleValue)
                    .sum();
                Some(busy.clamp(0.0, 100.0) as f32)
            }
        }
    }

    impl Drop for Counters {
        fn drop(&mut self) {
            // SAFETY: the query this opened
            unsafe {
                PdhCloseQuery(self.query);
            }
        }
    }

    /// The video memory this process holds on every adapter, and the budget the system gives
    /// it on the ones it uses.
    pub fn vram() -> Option<(u64, u64)> {
        // SAFETY: COM calls on interfaces DXGI hands out, an out-parameter of the size asked
        unsafe {
            let factory: IDXGIFactory1 = CreateDXGIFactory1().ok()?;
            let (mut used, mut budget) = (0, 0);
            for i in 0.. {
                let Ok(adapter) = factory.EnumAdapters1(i) else { break };
                let Ok(adapter) = adapter.cast::<IDXGIAdapter3>() else { continue };
                let mut info = DXGI_QUERY_VIDEO_MEMORY_INFO::default();
                if adapter.QueryVideoMemoryInfo(0, DXGI_MEMORY_SEGMENT_GROUP_LOCAL, &mut info).is_ok() && info.CurrentUsage > 0 {
                    used += info.CurrentUsage;
                    budget += info.Budget;
                }
            }
            Some((used, budget))
        }
    }
}

/// Seconds of CPU the process has used, and its resident memory.
#[cfg(windows)]
fn read() -> Option<(f64, Option<u64>)> {
    use windows::Win32::Foundation::FILETIME;
    use windows::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
    use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};
    let seconds = |f: FILETIME| ((u64::from(f.dwHighDateTime) << 32) | u64::from(f.dwLowDateTime)) as f64 / 1e7;
    let (mut created, mut exited, mut kernel, mut user) = Default::default();
    let mut memory = PROCESS_MEMORY_COUNTERS::default();
    // SAFETY: the current process' own handle, and out-parameters of the sizes asked for
    unsafe {
        let process = GetCurrentProcess();
        GetProcessTimes(process, &mut created, &mut exited, &mut kernel, &mut user).ok()?;
        let ok = GetProcessMemoryInfo(process, &mut memory, size_of::<PROCESS_MEMORY_COUNTERS>() as u32).is_ok();
        Some((seconds(kernel) + seconds(user), ok.then_some(memory.WorkingSetSize as u64)))
    }
}

/// The machine's physical memory, bytes.
#[cfg(windows)]
fn total_memory() -> Option<u64> {
    use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    let mut status = MEMORYSTATUSEX { dwLength: size_of::<MEMORYSTATUSEX>() as u32, ..Default::default() };
    // SAFETY: an out-parameter whose length field says its size
    unsafe { GlobalMemoryStatusEx(&mut status) }.ok().map(|_| status.ullTotalPhys)
}

/// ponytail: ticks taken as 100 a second and pages as 4 KiB, as on nearly every Linux; sysconf
/// through libc if one ever differs
#[cfg(target_os = "linux")]
fn read() -> Option<(f64, Option<u64>)> {
    let stat = std::fs::read_to_string("/proc/self/stat").ok()?;
    // after the command's closing parenthesis: state is the first field, utime the 12th, stime the 13th
    let fields: Vec<&str> = stat.rsplit_once(')')?.1.split_whitespace().collect();
    let ticks: f64 = fields.get(11)?.parse::<f64>().ok()? + fields.get(12)?.parse::<f64>().ok()?;
    let pages = std::fs::read_to_string("/proc/self/statm").ok().and_then(|s| s.split_whitespace().nth(1)?.parse::<u64>().ok());
    Some((ticks / 100.0, pages.map(|p| p * 4096)))
}

#[cfg(target_os = "linux")]
fn total_memory() -> Option<u64> {
    let info = std::fs::read_to_string("/proc/meminfo").ok()?;
    let kb: u64 = info.lines().find_map(|l| l.strip_prefix("MemTotal:"))?.trim().trim_end_matches("kB").trim().parse().ok()?;
    Some(kb * 1024)
}

#[cfg(target_os = "macos")]
fn read() -> Option<(f64, Option<u64>)> {
    // SAFETY: getrusage and proc_pidinfo fill the structs they're given, of the sizes given
    let (usage, task) = unsafe {
        let mut usage = std::mem::zeroed::<libc::rusage>();
        let mut task = std::mem::zeroed::<libc::proc_taskinfo>();
        let size = size_of::<libc::proc_taskinfo>() as libc::c_int;
        let got = libc::proc_pidinfo(std::process::id() as libc::c_int, libc::PROC_PIDTASKINFO, 0, (&raw mut task).cast(), size) == size;
        ((libc::getrusage(libc::RUSAGE_SELF, &mut usage) == 0).then_some(usage)?, got.then_some(task))
    };
    let seconds = |t: libc::timeval| t.tv_sec as f64 + t.tv_usec as f64 / 1e6;
    Some((seconds(usage.ru_utime) + seconds(usage.ru_stime), task.map(|t| t.pti_resident_size)))
}

#[cfg(target_os = "macos")]
fn total_memory() -> Option<u64> {
    let (mut bytes, mut size) = (0u64, size_of::<u64>());
    // SAFETY: hw.memsize is a u64, written into one of the size given
    let ok = unsafe { libc::sysctlbyname(c"hw.memsize".as_ptr(), (&raw mut bytes).cast(), &mut size, std::ptr::null_mut(), 0) } == 0;
    ok.then_some(bytes)
}

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
fn read() -> Option<(f64, Option<u64>)> {
    None
}

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
fn total_memory() -> Option<u64> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_reads_its_own_load_a_second_apart() {
        let mut usage = Usage::new();
        usage.sample();
        assert_eq!(usage.cpu, None, "one reading says nothing about a rate");
        // a little work, and a second gone by
        let start = Instant::now();
        while start.elapsed() < Duration::from_millis(50) {
            std::hint::black_box(start.elapsed());
        }
        usage.last = usage.last.map(|(at, s)| (at - EVERY, s));
        usage.sample();
        assert!(usage.cpu.is_some_and(|c| (0.0..=100.0).contains(&c)), "{:?}", usage.cpu);
        assert_eq!(usage.trails.cpu.len(), 1, "a share once there's a rate");
        if cfg!(any(windows, target_os = "linux", target_os = "macos")) {
            assert!(usage.memory.is_some_and(|m| m > 1 << 20), "{:?}", usage.memory);
            assert!(usage.memory_total > usage.memory, "{:?} of {:?}", usage.memory, usage.memory_total);
            assert!(usage.trails.memory.iter().all(|s| (0.0..=1.0).contains(s)));
        }
    }

    #[test]
    fn a_trail_keeps_half_a_minute() {
        let mut trail = VecDeque::new();
        for i in 0..TRAIL + 5 {
            push(&mut trail, Some(i as f32));
        }
        push(&mut trail, None);
        assert_eq!(trail.len(), TRAIL);
        assert_eq!(trail.front(), Some(&5.0), "the oldest go");
        assert_eq!(share(Some(1), Some(4)), Some(0.25));
        assert_eq!(share(Some(1), Some(0)), None, "no whole, no share");
    }
}
