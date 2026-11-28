//! The command-line choice of GPU and present mode, and the terminal's `--help`.
//!
//! On Linux the renderer picks a GPU and a present mode by itself (MoonUI's `gpu_selection`).
//! A machine where that pick is wrong — several GPUs, a monitor on another card's output — gets
//! three options here: `--list-gpus` to see the GPUs and their ids, `--gpu <id>` and
//! `--present-mode <mode>` to choose. An option left out means automatic. They apply to this
//! launch only and never touch the saved settings. An explicit choice is never swapped for
//! another: an unknown id, a GPU that cannot draw to the window or a mode its surface lacks ends
//! the launch with a message and exit code 2.
//!
//! Parsed in `main`, before the updater and before anything else reads a setting, because
//! `--help` and `--list-gpus` must answer without a window, a database or a core connection.

use std::sync::OnceLock;

/// The text `--help` prints.
pub(crate) const USAGE: &str = "\
MoonTerminal — trading terminal for MoonBot cores

Usage: moonterminal [options]

GPU options (Linux only; each one left out is chosen automatically):
  --list-gpus              List the GPUs MoonTerminal can render with, with the ID each one
                           is chosen by, and exit. Opens no window and connects to no core.
  --gpu <auto|ID>          Render with the GPU of this ID from --list-gpus. Default: auto,
                           which prefers the GPU the display server runs on.
  --present-mode <auto|fifo|mailbox|immediate>
                           How finished frames reach the screen. fifo waits for the monitor's
                           refresh (no tearing), mailbox replaces a waiting frame with a newer
                           one, immediate shows each frame at once and may tear.
                           Default: auto, which is fifo on X11 and mailbox on Wayland.

  -h, --help               Show this help and exit.

The options apply to this launch only and change no saved setting. A GPU or mode that cannot
be used when MoonTerminal starts — or a mode MESA_VK_WSI_PRESENT_MODE would replace — is
reported with the choices available, and MoonTerminal exits with code 2 instead of picking
another. If MoonTerminal is already running, close it first: the options take effect only when
it starts, and this launch exits with code 2.

Examples:
  moonterminal --list-gpus
  moonterminal --gpu pci-0000:01:00.0
  moonterminal --present-mode fifo
  moonterminal --gpu pci-0000:01:00.0 --present-mode mailbox
";

/// What `moonterminal --gpu`/`--present-mode` was told, or the launch is refused with when the
/// running instance cannot take the options.
pub(crate) const ALREADY_RUNNING: &str = "MoonTerminal is already running. --gpu and \
--present-mode take effect only when it starts: close the running MoonTerminal and launch it \
again with these options.";

/// The present modes `--present-mode` takes besides `auto`, as MoonUI spells them.
const PRESENT_MODES: [&str; 3] = ["fifo", "mailbox", "immediate"];

/// The GPU options of this launch.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct GpuArgs {
    /// `--help` or `-h`.
    pub(crate) help: bool,
    /// `--list-gpus`.
    pub(crate) list_gpus: bool,
    /// The id given to `--gpu`; `None` is automatic.
    pub(crate) gpu: Option<String>,
    /// The mode given to `--present-mode`, lower-case, one of [`PRESENT_MODES`]; `None` is
    /// automatic.
    pub(crate) present_mode: Option<String>,
}

impl GpuArgs {
    /// Whether the launch names a GPU or a present mode rather than leaving both automatic.
    pub(crate) fn explicit(&self) -> bool {
        self.gpu.is_some() || self.present_mode.is_some()
    }
}

/// Parse the GPU options out of `args` (the program name left out). Every other argument is
/// someone else's — `--fixture`, `--debug-script`, the updater's — and is passed over.
///
/// `--gpu X` and `--gpu=X` are the same, likewise for `--present-mode`; `auto` is the same as
/// leaving the option out. An option given twice, without a value or with a value it does not
/// take is an error, never a guess.
pub(crate) fn parse<I>(args: I) -> Result<GpuArgs, String>
where
    I: IntoIterator<Item = String>,
{
    let mut parsed = GpuArgs::default();
    let mut gpu_seen = false;
    let mut mode_seen = false;
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        let (name, inline) = match arg.split_once('=') {
            Some((name, value)) if name.starts_with("--") => (name.to_string(), Some(value)),
            _ => (arg.clone(), None),
        };
        match name.as_str() {
            "--help" | "-h" => parsed.help = true,
            "--list-gpus" => parsed.list_gpus = true,
            "--gpu" | "--present-mode" => {
                let value = match inline {
                    Some(value) => value.to_string(),
                    None => match args.next() {
                        Some(value) if !value.starts_with("--") => value,
                        _ => return Err(format!("{name} needs a value")),
                    },
                };
                let value = value.trim().to_string();
                if value.is_empty() {
                    return Err(format!("{name} needs a value"));
                }
                let seen = if name == "--gpu" {
                    &mut gpu_seen
                } else {
                    &mut mode_seen
                };
                if std::mem::replace(seen, true) {
                    return Err(format!("{name} is given more than once"));
                }
                let auto = value.eq_ignore_ascii_case("auto");
                if name == "--gpu" {
                    parsed.gpu = (!auto).then_some(value);
                } else if !auto {
                    let mode = value.to_ascii_lowercase();
                    if !PRESENT_MODES.contains(&mode.as_str()) {
                        return Err(format!(
                            "--present-mode {value}: unknown mode; use auto, {}",
                            PRESENT_MODES.join(", ")
                        ));
                    }
                    parsed.present_mode = Some(mode);
                }
            }
            _ => {}
        }
    }
    Ok(parsed)
}

static ARGS: OnceLock<GpuArgs> = OnceLock::new();

/// The GPU options of this launch, all automatic before [`handle_early`] ran.
pub(crate) fn args() -> &'static GpuArgs {
    ARGS.get_or_init(GpuArgs::default)
}

/// Answer `--help` and `--list-gpus`, refuse options this build cannot honour, and keep the rest
/// for [`install`]. Returns the exit code when the launch ends here.
pub(crate) fn handle_early() -> Option<i32> {
    let parsed = match parse(
        std::env::args_os()
            .skip(1)
            .map(|a| a.to_string_lossy().into_owned()),
    ) {
        Ok(parsed) => parsed,
        Err(message) => {
            eprintln!("moonterminal: {message}\nRun `moonterminal --help` for the options.");
            return Some(2);
        }
    };
    if parsed.help {
        write_stdout(USAGE);
        return Some(0);
    }
    if !GPU_OPTIONS_SUPPORTED && (parsed.list_gpus || parsed.explicit()) {
        eprintln!("moonterminal: --list-gpus, --gpu and --present-mode exist only on Linux");
        return Some(2);
    }
    if parsed.list_gpus {
        return Some(list_gpus());
    }
    let _ = ARGS.set(parsed);
    None
}

/// Write `text` to stdout and pass over a failure: `print!` panics on a closed pipe
/// (`moonterminal --list-gpus | head -1`), and this runs before the panic hook exists.
fn write_stdout(text: &str) {
    use std::io::Write as _;
    let mut out = std::io::stdout().lock();
    let _ = out.write_all(text.as_bytes()).and_then(|()| out.flush());
}

/// Whether this build renders through MoonUI's wgpu renderer, the one the GPU options steer.
const GPU_OPTIONS_SUPPORTED: bool = cfg!(all(
    any(target_os = "linux", target_os = "freebsd"),
    any(feature = "linux-x11", feature = "linux-wayland")
));

/// Hand the explicit choices of this launch to the renderer, before the first window.
pub(crate) fn install() {
    #[cfg(all(
        any(target_os = "linux", target_os = "freebsd"),
        any(feature = "linux-x11", feature = "linux-wayland")
    ))]
    {
        use gpui_platform::gpu_selection::{
            AdapterChoice, GpuSelection, PresentModeChoice, parse_present_mode, set_gpu_selection,
        };
        let args = args();
        if !args.explicit() {
            return;
        }
        let adapter = match &args.gpu {
            Some(id) => AdapterChoice::Id(id.clone()),
            None => AdapterChoice::Auto,
        };
        let present_mode = match args.present_mode.as_deref().and_then(parse_present_mode) {
            Some(mode) => PresentModeChoice::Explicit(mode),
            None => PresentModeChoice::Auto,
        };
        set_gpu_selection(
            GpuSelection {
                adapter,
                present_mode,
            },
            unusable,
        );
    }
}

/// An explicit GPU option cannot be honoured: say why on stderr and in the log, and end the
/// launch with exit code 2 instead of rendering with something the user did not choose.
#[cfg(all(
    any(target_os = "linux", target_os = "freebsd"),
    any(feature = "linux-x11", feature = "linux-wayland")
))]
fn unusable(message: String) -> ! {
    log::error!("GPU option cannot be honoured: {message}");
    log::logger().flush();
    eprintln!(
        "moonterminal: {message}\nRun `moonterminal --list-gpus` for the GPUs and their IDs, \
         `moonterminal --help` for the options."
    );
    std::process::exit(2)
}

/// Print the GPUs `--gpu` can name and the environment variables that change them. Exit code 0,
/// or 1 when there is none — the Vulkan driver of the GPU is missing.
#[cfg(all(
    any(target_os = "linux", target_os = "freebsd"),
    any(feature = "linux-x11", feature = "linux-wayland")
))]
fn list_gpus() -> i32 {
    use gpui_platform::gpu_selection::{env_overrides, list_gpus};
    use std::fmt::Write as _;
    let gpus = list_gpus();
    let mut overrides = String::new();
    for o in env_overrides() {
        let _ = writeln!(overrides, "  {}={}  ({})", o.name, o.value, o.effect);
    }
    if !overrides.is_empty() {
        overrides.insert_str(
            0,
            "\nEnvironment variables that change the GPUs found, the automatic choice or the \
             present mode:\n",
        );
    }
    if gpus.is_empty() {
        eprintln!(
            "No Vulkan GPU found. MoonTerminal renders through Vulkan: install the Vulkan driver \
             of your GPU, see https://github.com/Moonbot-Tech/MoonTerminal/blob/main/docs/\
             MAC_LINUX_BUILD.md#gpu-and-vulkan-driver"
        );
        write_stdout(&overrides);
        return 1;
    }
    let id_width = gpus.iter().map(|g| g.id.len()).max().unwrap_or(2).max(2);
    let name_width = gpus.iter().map(|g| g.name.len()).max().unwrap_or(4).max(4);
    let mut out = format!(
        "{:<id_width$}  {:<name_width$}  {:<13}  DRIVER\n",
        "ID", "NAME", "TYPE"
    );
    for gpu in &gpus {
        let kind = format!("{:?}", gpu.device_type);
        let _ = writeln!(
            out,
            "{:<id_width$}  {:<name_width$}  {:<13}  {} {} ({:?})",
            gpu.id, gpu.name, kind, gpu.driver, gpu.driver_info, gpu.backend
        );
    }
    out.push_str(&overrides);
    let _ = writeln!(
        out,
        "\nChoose one with --gpu <ID>, e.g. moonterminal --gpu {}. Whether a GPU can draw to the \
         window is checked when MoonTerminal starts.",
        gpus[0].id
    );
    write_stdout(&out);
    0
}

/// No wgpu renderer in this build: [`handle_early`] refuses `--list-gpus` before this is reached.
#[cfg(not(all(
    any(target_os = "linux", target_os = "freebsd"),
    any(feature = "linux-x11", feature = "linux-wayland")
)))]
fn list_gpus() -> i32 {
    2
}

#[cfg(test)]
mod tests;
