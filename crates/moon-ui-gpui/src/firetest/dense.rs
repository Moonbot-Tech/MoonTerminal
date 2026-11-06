//! A candle chart and a dense hybrid sharing generated history through the normal chart consumer.

use std::time::{Duration, Instant};

use gpui::*;
use moon_ui::Root;

use super::{logging::firetest_info, plan::Phase};
use crate::{Backend, chartdx::ChartDataHandle};

/// Real windows and consumers retained throughout a generated FireTest run.
pub(super) struct DenseRun {
    core: moon_core::session::CoreId,
    pub(super) window: WindowHandle<Root>,
    pub(super) hybrid: ChartDataHandle,
    pub(super) candle: ChartDataHandle,
    last_batch: Instant,
    /// Generations before each explicit reset; two passes catch state consumed by the first recovery.
    recovery: Option<RecoveryCheck>,
    autosave: Option<crate::persistence::coordinator::firetest::AutosaveProbe>,
}

/// Diagnostic-only state: wait for stable native pixels before either of the two device resets.
#[derive(Clone)]
enum RecoveryCheck {
    #[cfg(windows)]
    Baseline(Option<[std::sync::Arc<crate::chartdx::RecoveryImage>; 2]>),
    AfterReset {
        completed: u8,
        before: [(usize, u64); 2],
        #[cfg(windows)]
        baseline: [std::sync::Arc<crate::chartdx::RecoveryImage>; 2],
    },
}

/// The generated fixture is explicit; a live-core run keeps its existing data source.
pub(crate) fn selected() -> bool {
    moon_core::fixture::active().is_some_and(|fixture| fixture.dense_history().is_some())
}

/// Configure the existing Main stack only after its normal open request has reached the host.
pub(crate) fn install_main(
    main: &Entity<crate::chart_tabs::MainChartStack>,
    backend: &Entity<Backend>,
    group: &str,
    window: &Window,
    cx: &mut App,
) {
    if !selected()
        || backend.read(cx).firetest.as_ref().is_none_or(|runtime| {
            runtime.dense.is_some() || runtime.opened_group.as_deref() != Some(group)
        })
    {
        return;
    }
    let Some((core, _)) = main.read(cx).active_target(cx) else {
        return;
    };
    let [candle, hybrid] = main.update(cx, |main, cx| main.firetest_dense_pair(core, cx));
    let run = DenseRun {
        core,
        window: window
            .window_handle()
            .downcast::<Root>()
            .expect("Main window has a Root"),
        hybrid,
        candle,
        last_batch: Instant::now(),
        recovery: None,
        autosave: None,
    };
    backend.update(cx, |backend, _| {
        if let Some(runtime) = &mut backend.firetest {
            runtime.dense = Some(run);
        }
    });
    firetest_info(&format!(
        "[firetest] dense seed=1 generated={} retained={} host=main-stack panels=candles,hybrid",
        moon_core::fixture::dense::CAPACITY,
        moon_core::fixture::dense::CAPACITY
    ));
}
impl DenseRun {
    /// Start the slow disk case during the first existing mouse phase, without extending the plan.
    pub(super) fn start_autosave(&mut self, backend: &mut Backend) -> Result<(), String> {
        if self.autosave.is_none() {
            self.autosave = Some(
                crate::persistence::coordinator::firetest::AutosaveProbe::start(
                    backend, self.core,
                )?,
            );
        }
        Ok(())
    }

    /// Fail the first storm if the actual dispatcher lost progress or the latest drawing edit.
    pub(super) fn verify_autosave(&self) -> Result<(), String> {
        let ticks = self
            .autosave
            .as_ref()
            .ok_or("autosave case did not start")?
            .verify()?;
        firetest_info(&format!(
            "[firetest] autosave slow_failure_min_ms=600 blocked_ui_ticks={ticks} frames/input=true latest_persisted=true"
        ));
        Ok(())
    }

    /// Validate the actual camera workload before each mouse phase, not just allocated capacity.
    pub(super) fn verify_visible(&self) -> Result<(), String> {
        let candle = self
            .candle
            .firetest_visible_history()
            .ok_or("candle scene has no plot")?;
        let hybrid = self
            .hybrid
            .firetest_visible_history()
            .ok_or("hybrid scene has no plot")?;
        firetest_info(&format!(
            "[firetest] dense visible candle={candle:?} hybrid={hybrid:?} (trades,candles,columns/16)"
        ));
        if candle.0 != 0 || candle.1 < 200 || hybrid.0 < 25_000 || hybrid.1 < 200 || hybrid.2 < 6 {
            return Err(format!(
                "dense scene is not visibly dense: candle={candle:?}, hybrid={hybrid:?}"
            ));
        }
        Ok(())
    }

    /// Freeze the producer, recreate the shared native device twice, and check both consumers.
    pub(super) fn recover(&mut self, cx: &mut Context<Backend>) -> Result<bool, String> {
        let probes = [
            self.candle.firetest_history_probe(),
            self.hybrid.firetest_history_probe(),
        ];
        let [Some(candle), Some(hybrid)] = probes else {
            return Err("dense consumer was dropped".into());
        };
        let current = [candle, hybrid];
        if candle.0 != 0 || hybrid.0 != moon_core::fixture::dense::CAPACITY {
            return Err(format!(
                "dense consumer counts: candle={} hybrid={}, expected 0/{}",
                candle.0,
                hybrid.0,
                moon_core::fixture::dense::CAPACITY
            ));
        }
        match self.recovery.clone() {
            None => {
                let history = moon_core::fixture::active()
                    .and_then(|fixture| fixture.dense_history())
                    .ok_or("dense history missing")?;
                let mut history = history.lock().unwrap_or_else(|error| error.into_inner());
                history.set_frozen(true);
                firetest_info(&format!(
                    "[firetest] dense frozen generated={} retained={} hybrid_consumed={}",
                    history.produced(),
                    history.retained(),
                    hybrid.0
                ));
                drop(history);
                #[cfg(windows)]
                {
                    self.candle.firetest_park_recovery_camera()?;
                    self.hybrid.firetest_park_recovery_camera()?;
                    self.request_images()?;
                    self.recovery = Some(RecoveryCheck::Baseline(None));
                }
                #[cfg(not(windows))]
                {
                    firetest_info(
                        "[firetest] gpu_recovery ownpass_image=UNSUPPORTED backend_readback=d3d11-only; retained-data check continues",
                    );
                    self.reset_device(cx)?;
                    self.recovery = Some(RecoveryCheck::AfterReset {
                        completed: 0,
                        before: current,
                    });
                }
            }
            #[cfg(windows)]
            Some(RecoveryCheck::Baseline(previous)) => {
                let Some(images) = self.images()? else {
                    return Ok(false);
                };
                for image in &images {
                    image.validate()?;
                }
                // Let the existing book bake/Y easing settle by observing equal completed images,
                // not by adding another sleep or a stage duration.
                let stable = previous.as_ref().is_some_and(|before| {
                    before
                        .iter()
                        .zip(&images)
                        .all(|(before, after)| before.compare(after).is_ok())
                });
                if !stable {
                    self.recovery = Some(RecoveryCheck::Baseline(Some(images)));
                    self.request_images()?;
                    return Ok(false);
                }
                firetest_info(&format!(
                    "[firetest] gpu_recovery ownpass_baseline={}x{} / {}x{} frozen=true camera=parked",
                    images[0].width, images[0].height, images[1].width, images[1].height
                ));
                firetest_info(&format!(
                    "[firetest] gpu_recovery prepared_rows(candles,side,hvol)={:?} / {:?}; ownpass_only=true gpui_text=separate",
                    self.candle.firetest_recovery_layer_counts(),
                    self.hybrid.firetest_recovery_layer_counts()
                ));
                for chart in [&self.candle, &self.hybrid] {
                    if !chart
                        .firetest_recovery_layer_counts()
                        .is_some_and(|(candles, side, hvol)| candles != 0 && side != 0 && hvol != 0)
                    {
                        return Err("GPU recovery requires populated candle, side-volume and profile layers".into());
                    }
                }
                self.reset_device(cx)?;
                self.recovery = Some(RecoveryCheck::AfterReset {
                    completed: 0,
                    before: current,
                    baseline: images,
                });
            }
            Some(RecoveryCheck::AfterReset {
                completed,
                before,
                #[cfg(windows)]
                baseline,
            }) => {
                if current
                    .iter()
                    .zip(before)
                    .any(|(after, before)| after.1 == before.1)
                {
                    return Ok(false);
                }
                #[cfg(windows)]
                {
                    let Some(images) = self.images()? else {
                        return Ok(false);
                    };
                    if images
                        .iter()
                        .zip(current)
                        .any(|(image, probe)| image.generation != probe.1)
                    {
                        self.request_images()?;
                        return Ok(false);
                    }
                    for (ix, (before, after)) in baseline.iter().zip(&images).enumerate() {
                        before
                            .compare(after)
                            .map_err(|error| format!("chart {ix}: {error}"))?;
                    }
                    firetest_info(&format!(
                        "[firetest] gpu_recovery ownpass_image=PASS round={} charts=2 exact_pixels=true",
                        completed + 1
                    ));
                }
                firetest_info(&format!(
                    "[firetest] gpu_recovery round={} generations={}->{} / {}->{} retained={}/{}",
                    completed + 1,
                    before[0].1,
                    current[0].1,
                    before[1].1,
                    current[1].1,
                    candle.0,
                    hybrid.0
                ));
                if completed == 1 {
                    return Ok(true);
                }
                self.reset_device(cx)?;
                self.recovery = Some(RecoveryCheck::AfterReset {
                    completed: completed + 1,
                    before: current,
                    #[cfg(windows)]
                    baseline,
                });
            }
        }
        Ok(false)
    }

    /// Reset the shared device through the existing native recovery entry point.
    fn reset_device(&self, cx: &mut Context<Backend>) -> Result<(), String> {
        let requested = self
            .window
            .update(cx, |_, window, cx| window.request_gpu_device_reset(cx))
            .map_err(|error| error.to_string())?;
        if !requested {
            return Err("native device recovery is unsupported on this platform".into());
        }
        Ok(())
    }

    /// Arm both real Main charts; a normal frame performs each readback once.
    #[cfg(windows)]
    fn request_images(&self) -> Result<(), String> {
        self.candle.firetest_request_recovery_image()?;
        self.hybrid.firetest_request_recovery_image()
    }

    /// Poll only completed native image captures, retaining their pixels through Arc.
    #[cfg(windows)]
    fn images(&self) -> Result<Option<[std::sync::Arc<crate::chartdx::RecoveryImage>; 2]>, String> {
        Ok(
            match [
                self.candle.firetest_recovery_image()?,
                self.hybrid.firetest_recovery_image()?,
            ] {
                [Some(candle), Some(hybrid)] => Some([candle, hybrid]),
                _ => None,
            },
        )
    }

    /// Identical logical-time batches in baseline and mouse phases keep their comparison fair.
    pub(super) fn tick(&mut self, phase: Option<Phase>, backend: &mut Backend) {
        if let Some(probe) = &mut self.autosave {
            probe.tick(backend);
        }
        let advancing = matches!(
            phase,
            Some(
                Phase::Baseline
                    | Phase::Storm
                    | Phase::FlashStorm
                    | Phase::StaticTextWarmup
                    | Phase::StaticTextStorm
            )
        );
        if !advancing || self.last_batch.elapsed() < Duration::from_millis(100) {
            return;
        }
        self.last_batch = Instant::now();
        let Some(history) =
            moon_core::fixture::active().and_then(|fixture| fixture.dense_history())
        else {
            return;
        };
        history
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .advance(250, 100);
    }
}
