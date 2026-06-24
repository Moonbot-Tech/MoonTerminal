//! Диагностические метрики процесса и системы для статус-бара: CPU
//! (процесс/система), RAM процесса и её рост за окно. На Windows дополнительно
//! снимаем GPU Engine utilisation текущего процесса через PDH. sysinfo/PDH
//! обновление дорогое, поэтому реально опрашиваем не чаще REFRESH_EVERY, между
//! сэмплами отдаём кэш.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use sysinfo::{Pid, ProcessesToUpdate, System};

/// Как часто реально опрашиваем sysinfo.
const REFRESH_EVERY: Duration = Duration::from_millis(1000);
/// Окно, на котором считаем прирост памяти (растёт/падает).
const MEM_WINDOW: Duration = Duration::from_secs(5);

/// Снимок метрик — Copy, дёшево прокидывается в каждый `WindowHost::render`.
#[derive(Clone, Copy, Default)]
pub struct MetricsSnapshot {
    /// CPU процесса, % всей машины (как в Task Manager: 100% = все ядра заняты).
    pub cpu_process: f32,
    /// CPU всей системы, %.
    pub cpu_system: f32,
    /// RAM процесса (resident), МБ.
    pub mem_mb: f32,
    /// Прирост RAM за MEM_WINDOW, МБ (>0 — растёт; стабильный плюс → утечка).
    pub mem_delta_mb: f32,
    /// GPU текущего процесса, % по Windows GPU Engine counters. На не-Windows 0.
    pub gpu_process: f32,
}

pub struct Metrics {
    sys: System,
    pid: Pid,
    ncpu: f32,
    last_refresh: Option<Instant>,
    snap: MetricsSnapshot,
    gpu: GpuProcessSampler,
    /// (время, RAM МБ) для расчёта прироста за MEM_WINDOW.
    mem_hist: VecDeque<(Instant, f32)>,
}

impl Metrics {
    pub fn new() -> Self {
        let mut sys = System::new();
        sys.refresh_cpu_usage();
        let ncpu = sys.cpus().len().max(1) as f32;
        let pid = sysinfo::get_current_pid().unwrap_or(Pid::from(0));
        Self {
            sys,
            pid,
            ncpu,
            last_refresh: None,
            snap: MetricsSnapshot::default(),
            gpu: GpuProcessSampler::new(pid_as_u32(pid)),
            mem_hist: VecDeque::new(),
        }
    }

    /// Актуальный снимок; реально опрашивает систему не чаще REFRESH_EVERY.
    pub fn sample(&mut self, now: Instant) -> MetricsSnapshot {
        let due = self
            .last_refresh
            .is_none_or(|t| now.duration_since(t) >= REFRESH_EVERY);
        if !due {
            return self.snap;
        }
        self.last_refresh = Some(now);

        self.sys.refresh_cpu_usage();
        self.sys.refresh_memory();
        self.sys
            .refresh_processes(ProcessesToUpdate::Some(&[self.pid]), true);

        let cpu_system = self.sys.global_cpu_usage();
        let (cpu_process, mem_mb) = match self.sys.process(self.pid) {
            // cpu_usage(): 100% = одно ядро → делим на число ядер (как Task Manager).
            Some(p) => (
                p.cpu_usage() / self.ncpu,
                p.memory() as f32 / (1024.0 * 1024.0),
            ),
            None => (0.0, 0.0),
        };

        self.mem_hist.push_back((now, mem_mb));
        while self
            .mem_hist
            .front()
            .is_some_and(|(t, _)| now.duration_since(*t) > MEM_WINDOW)
        {
            self.mem_hist.pop_front();
        }
        let mem_delta_mb = self
            .mem_hist
            .front()
            .map(|(_, m0)| mem_mb - *m0)
            .unwrap_or(0.0);
        let gpu_process = self.gpu.sample().unwrap_or(self.snap.gpu_process);

        self.snap = MetricsSnapshot {
            cpu_process,
            cpu_system,
            mem_mb,
            mem_delta_mb,
            gpu_process,
        };
        self.snap
    }
}

impl Default for Metrics {
    fn default() -> Self {
        Self::new()
    }
}

fn pid_as_u32(pid: Pid) -> u32 {
    let text = pid.to_string();
    text.parse().unwrap_or(0)
}

#[cfg(windows)]
struct GpuProcessSampler {
    pid_pattern: String,
    query: windows_sys::Win32::System::Performance::PDH_HQUERY,
    counter: windows_sys::Win32::System::Performance::PDH_HCOUNTER,
    available: bool,
}

#[cfg(windows)]
impl GpuProcessSampler {
    fn new(pid: u32) -> Self {
        use windows_sys::Win32::System::Performance::{
            PdhAddEnglishCounterW, PdhCollectQueryData, PdhOpenQueryW,
        };

        let mut query = std::ptr::null_mut();
        let mut counter = std::ptr::null_mut();
        let mut available = false;
        let path = to_wide("\\GPU Engine(*)\\Utilization Percentage");
        unsafe {
            if PdhOpenQueryW(std::ptr::null(), 0, &mut query) == 0
                && PdhAddEnglishCounterW(query, path.as_ptr(), 0, &mut counter) == 0
            {
                let _ = PdhCollectQueryData(query);
                available = true;
            }
        }
        if !available && !query.is_null() {
            unsafe {
                windows_sys::Win32::System::Performance::PdhCloseQuery(query);
            }
            query = std::ptr::null_mut();
            counter = std::ptr::null_mut();
        }
        Self {
            pid_pattern: format!("pid_{pid}_"),
            query,
            counter,
            available,
        }
    }

    fn sample(&mut self) -> Option<f32> {
        use windows_sys::Win32::System::Performance::{
            PdhCollectQueryData, PdhGetFormattedCounterArrayW, PDH_FMT_COUNTERVALUE_ITEM_W,
            PDH_FMT_DOUBLE, PDH_MORE_DATA,
        };

        if !self.available {
            return None;
        }
        unsafe {
            if PdhCollectQueryData(self.query) != 0 {
                self.available = false;
                return None;
            }

            let mut bytes = 0_u32;
            let mut count = 0_u32;
            let status = PdhGetFormattedCounterArrayW(
                self.counter,
                PDH_FMT_DOUBLE,
                &mut bytes,
                &mut count,
                std::ptr::null_mut(),
            );
            if status != PDH_MORE_DATA || bytes == 0 {
                return None;
            }

            let item_size = std::mem::size_of::<PDH_FMT_COUNTERVALUE_ITEM_W>() as u32;
            let item_count = count.max(bytes.div_ceil(item_size)).max(1);
            let mut items = vec![PDH_FMT_COUNTERVALUE_ITEM_W::default(); item_count as usize];
            let status = PdhGetFormattedCounterArrayW(
                self.counter,
                PDH_FMT_DOUBLE,
                &mut bytes,
                &mut count,
                items.as_mut_ptr(),
            );
            if status != 0 {
                return None;
            }

            let mut total = 0.0_f64;
            let mut matched = false;
            for item in items.iter().take(count as usize) {
                if item.szName.is_null() || item.FmtValue.CStatus != 0 {
                    continue;
                }
                let name = wide_ptr_to_string(item.szName);
                if !name.contains(&self.pid_pattern) {
                    continue;
                }
                let value = item.FmtValue.Anonymous.doubleValue;
                if value.is_finite() && value > 0.0 {
                    total += value;
                    matched = true;
                }
            }
            matched.then_some(total.clamp(0.0, 100.0) as f32)
        }
    }
}

#[cfg(windows)]
impl Drop for GpuProcessSampler {
    fn drop(&mut self) {
        if !self.query.is_null() {
            unsafe {
                windows_sys::Win32::System::Performance::PdhCloseQuery(self.query);
            }
        }
    }
}

#[cfg(windows)]
fn to_wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(windows)]
unsafe fn wide_ptr_to_string(ptr: *const u16) -> String {
    let mut len = 0_usize;
    while unsafe { *ptr.add(len) } != 0 {
        len += 1;
    }
    String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(ptr, len) })
}

#[cfg(not(windows))]
struct GpuProcessSampler;

#[cfg(not(windows))]
impl GpuProcessSampler {
    fn new(_pid: u32) -> Self {
        Self
    }

    fn sample(&mut self) -> Option<f32> {
        Some(0.0)
    }
}
