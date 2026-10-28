//! Background preparation of complete bubble snapshots, shared by every native backend.

use std::sync::{
    Arc, Mutex, OnceLock,
    atomic::{AtomicBool, Ordering},
    mpsc,
};

use moon_chart::clusters::{self, DiscRadii, Print, PrintDisc, PrintScale};
use moon_chart::layers::{MARKER_SHAPE_KNOT, MarkerInstance};

use super::{FoldKey, LayoutKey};
use crate::chartdx::types::{ChartCross, MarkerGpu, mk_of};

/// Immutable inputs whose equality makes a completed layout safe to publish.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::chartdx) struct Request {
    pub fold: FoldKey,
    pub layout: LayoutKey,
    pub range: (f64, f64),
    pub epoch: f64,
    pub generation: u64,
}

impl Request {
    /// A pan inside the captured reach uses the same prints; other input changes reject it.
    pub fn matches(self, current: Self, visible: (f64, f64)) -> bool {
        self.fold == current.fold
            && self.layout == current.layout
            && self.epoch == current.epoch
            && self.generation == current.generation
            && visible.0 >= self.range.0
            && visible.1 <= self.range.1
    }
}

/// Retained fold reused by zoom, density and colour changes without copying the print buffer.
pub(in crate::chartdx) struct Folded {
    pub(super) prints: Vec<Print>,
    pub(super) order: Vec<u32>,
    pub(super) scale: Option<PrintScale>,
}

/// A new tape snapshot or an already-published immutable fold for a layout-only request.
pub(super) enum Source {
    Tape(Vec<ChartCross>, Option<PrintScale>),
    Fold(Arc<Folded>),
}

/// All CPU outputs of one request, published together only after validating its inputs.
pub(super) struct Prepared {
    pub request: Request,
    pub folded: Arc<Folded>,
    pub discs: Vec<PrintDisc>,
    pub markers: Vec<MarkerGpu>,
}

/// One in-flight request per pane; the frame path polls only its atomic completion edge.
pub(in crate::chartdx) struct Pending {
    pub(super) ready: Arc<AtomicBool>,
    pub(super) receiver: mpsc::Receiver<Result<Prepared, String>>,
}

impl Pending {
    /// Whether a complete result or explicit failure is waiting without a UI-thread lock.
    pub(super) fn is_ready(&self) -> bool {
        self.ready.load(Ordering::Acquire)
    }

    /// Take an already-ready result without ever blocking the frame path.
    pub(super) fn take(self) -> Result<Prepared, String> {
        self.receiver.try_recv().map_err(|err| err.to_string())?
    }

    /// Enqueue one immutable snapshot on the bounded-concurrency background executor.
    pub(super) fn spawn(request: Request, source: Source) -> Result<Self, String> {
        let (sender, receiver) = mpsc::channel();
        let ready = Arc::new(AtomicBool::new(false));
        let done = ready.clone();
        executor()
            .as_ref()
            .map_err(Clone::clone)?
            .send(Box::new(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    prepare(request, source)
                }))
                .map_err(|_| "bubble preparation panicked".to_owned());
                let _ = sender.send(result);
                done.store(true, Ordering::Release);
            }))
            .map_err(|err| err.to_string())?;
        Ok(Self { ready, receiver })
    }
}

/// Background work is capped at two jobs; panes enqueue at most one request each.
type Job = Box<dyn FnOnce() + Send>;

/// Release superseded bubble buffers on the executor so publication never frees a large fold.
pub(super) fn release_later<T: Send + 'static>(value: T) {
    let result = executor()
        .as_ref()
        .map_err(Clone::clone)
        .and_then(|executor| {
            executor
                .send(Box::new(move || drop(value)))
                .map_err(|err| err.to_string())
        });
    if let Err(err) = result {
        log::error!("cannot retire bubble buffers: {err}");
    }
}

/// Lazily start the process-wide bubble executor without creating a thread on every refold.
fn executor() -> &'static Result<mpsc::Sender<Job>, String> {
    static EXECUTOR: OnceLock<Result<mpsc::Sender<Job>, String>> = OnceLock::new();
    EXECUTOR.get_or_init(|| {
        let (sender, receiver) = mpsc::channel::<Job>();
        let receiver = Arc::new(Mutex::new(receiver));
        for index in 0..2 {
            let receiver = receiver.clone();
            std::thread::Builder::new()
                .name(format!("chart-bubbles-{index}"))
                .spawn(move || {
                    loop {
                        let job = match receiver.lock() {
                            Ok(queue) => queue.recv(),
                            Err(err) => {
                                log::error!("bubble executor queue failed: {err}");
                                return;
                            }
                        };
                        match job {
                            Ok(job) => job(),
                            Err(_) => return,
                        }
                    }
                })
                .map_err(|err| err.to_string())?;
        }
        Ok(sender)
    })
}

/// Compute the original fold, hysteresis, stable painter order and native marker conversion.
pub(super) fn prepare(request: Request, source: Source) -> Prepared {
    let folded = match source {
        Source::Fold(folded) => folded,
        Source::Tape(samples, previous) => {
            let (mut prints, mut order, mut scratch) = (Vec::new(), Vec::new(), Vec::new());
            let samples = samples.iter().filter(|c| c.side < 2).map(|c| {
                (
                    request.epoch + f64::from(c.time_rel),
                    c.price,
                    c.qty,
                    c.side == 0,
                )
            });
            let tick =
                (request.fold.tick_bits != 0).then(|| f64::from_bits(request.fold.tick_bits));
            clusters::aggregate_prints(
                samples,
                tick,
                f64::from(request.fold.merge_ms),
                &mut prints,
            );
            clusters::price_time_order(&prints, &mut order);
            let fresh = clusters::print_scale(&prints, request.fold.small_pct, &mut scratch);
            let scale = match (previous, fresh) {
                (Some(old), Some(new)) if old.holds_against(&new) => Some(PrintScale {
                    small: old.small.min(new.small),
                    ..old
                }),
                (_, fresh) => fresh,
            };
            Arc::new(Folded {
                prints,
                order,
                scale,
            })
        }
    };
    let mut discs = Vec::new();
    if let Some(scale) = folded.scale {
        let radii = DiscRadii::at(request.layout.ppp);
        clusters::merge_overlapping(
            &folded.prints,
            &folded.order,
            f64::from(request.layout.time_to_px),
            |v| clusters::print_radius(v, scale, radii),
            &mut discs,
        );
    }
    let [buy, sell] = request.layout.colors.map(|c| [c[0], c[1], c[2], 0.85]);
    let mut markers: Vec<_> = discs
        .iter()
        .map(|d| {
            MarkerInstance::at_price(
                (d.time_ms - request.epoch) as f32,
                d.price,
                d.radius,
                0.0,
                MARKER_SHAPE_KNOT,
                if d.buy >= d.sell { buy } else { sell },
            )
        })
        .collect();
    markers.sort_by(|a, b| a.size.total_cmp(&b.size));
    let markers = markers.iter().map(mk_of).collect();
    Prepared {
        request,
        folded,
        discs,
        markers,
    }
}
