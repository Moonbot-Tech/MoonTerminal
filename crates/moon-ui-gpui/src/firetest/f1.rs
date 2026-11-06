//! Native frame-debt regression inside the existing Assets window, without synthetic draw calls.

use super::logging::firetest_info;
use crate::{Backend, panels::AssetsView};
use gpui::{prelude::*, *};
use moon_ui::{
    MoonInput, MoonInputState, MoonResizableState, moon_h_resizable, moon_resizable_panel,
};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    time::Duration,
};

/// Distinct strings must be found in painted text, never read back from InputState.
const BEFORE: &str = "F1 before";
/// Final content must occur in the platform window's completed painted frame.
const AFTER: &str = "F1 painted";

/// One real form retained by Assets only during an explicitly requested diagnostic run.
pub(crate) struct Form {
    input: Entity<MoonInputState>,
    panels: Entity<MoonResizableState>,
    renders: u32,
    width: Rc<Cell<f32>>,
    /// 0 = baseline; 1/2 = first mutation prepaint/paint; 3 = applied; 4/5 = repeat; 6 = repeated.
    step: Rc<Cell<u8>>,
}

impl Form {
    /// Build ordinary components; their existing notifications own all follow-up frames.
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self {
            input: cx.new(|cx| MoonInputState::new(window, cx).default_value(BEFORE)),
            panels: cx.new(|_| MoonResizableState::default()),
            renders: 0,
            width: Rc::new(Cell::new(0.0)),
            step: Rc::new(Cell::new(0)),
        }
    }

    /// Request just the triggering frame; after this, no rescue invalidation is issued.
    fn arm(&self, repeat: bool, cx: &mut Context<Self>) {
        self.step.set(if repeat { 4 } else { 1 });
        cx.notify();
    }
}

impl Render for Form {
    /// Resize after layout and edit after Input painted, so the result requires another frame.
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.renders += 1;
        self.input.update(cx, |input, cx| {
            input.set_placeholder("F1", window, cx);
            input.set_loading(false, window, cx);
        });
        let width = self.width.clone();
        let pre_step = self.step.clone();
        let paint_step = self.step.clone();
        let panels = self.panels.clone();
        let input = self.input.clone();
        div()
            .id("firetest-f1-form")
            .w(px(480.0))
            .h(px(64.0))
            .flex_none()
            .child(
                moon_h_resizable("firetest-f1-panels")
                    .with_state(&self.panels)
                    .child(
                        moon_resizable_panel().size(px(240.0)).child(
                            div()
                                .size_full()
                                .child(MoonInput::new("firetest-f1-input").state(&self.input))
                                .child(
                                    canvas(
                                        |_, _, _| (),
                                        move |bounds, _, _, _| width.set(bounds.size.width.into()),
                                    )
                                    .w_full()
                                    .h(px(1.0)),
                                ),
                        ),
                    )
                    .child(moon_resizable_panel().child("F1 neighbor")),
            )
            .child(
                canvas(
                    move |_, window, cx| {
                        let step = pre_step.get();
                        if step == 1 || step == 4 {
                            panels.update(cx, |panels, cx| {
                                panels.resize_panel_silently(0, px(160.0), window, cx)
                            });
                            pre_step.set(step + 1);
                        }
                    },
                    move |_, _, window, cx| {
                        let step = paint_step.get();
                        if step == 2 || step == 5 {
                            input.update(cx, |input, cx| input.set_value(AFTER, window, cx));
                            paint_step.set(step + 1);
                        }
                    },
                )
                .w_full()
                .h(px(1.0)),
            )
    }
}

/// The asynchronous result is checked by the existing cooldown verdict, including missing completion.
#[derive(Clone, Default)]
pub(super) struct Probe(Rc<RefCell<Option<Result<(), String>>>>);

impl Probe {
    /// Do not mistake an unfinished task for a successful native check.
    pub(super) fn verify(&self) -> Result<(), String> {
        self.0
            .borrow()
            .clone()
            .unwrap_or_else(|| Err("F1 native probe did not finish".into()))
    }
}

/// Read the completed frame's text and independently measured panel geometry without invalidation.
fn painted(window: &Window, width: f32, expected: &str, expected_width: f32) -> Result<(), String> {
    let found = window.inspector_texts().iter().any(|run| {
        run.text.as_ref() == expected && !run.bounds.intersect(&run.content_mask).is_empty()
    });
    if !found || (width - expected_width).abs() > 2.0 {
        return Err(format!(
            "F1 painted oracle: text={expected:?} found={found}, panel_width={width:.1}, expected={expected_width:.1}"
        ));
    }
    Ok(())
}

/// Require a native frame callback to finish without rendering the form, independent of FPS.
/// The first callback queues only a clock callback, not dirty state. The second runs before the
/// following frame's UI predicate; a perpetual paint notification must have rendered between them.
async fn quiet(
    window: WindowHandle<moon_ui::Root>,
    form: Entity<Form>,
    cx: &mut AsyncApp,
) -> Result<(), String> {
    let completion = Rc::new(RefCell::new((None, None::<std::task::Waker>)));
    let sender = completion.clone();
    window
        .update(cx, |_, window, _| {
            window.on_next_frame(move |window, cx| {
                let before = form.read(cx).renders;
                window.on_next_frame(move |_, cx| {
                    let after = form.read(cx).renders;
                    let result = if after == before {
                        Ok(())
                    } else {
                        Err(format!(
                            "F1 unchanged form rendered between native callbacks: {} extra renders",
                            after - before
                        ))
                    };
                    let mut completion = sender.borrow_mut();
                    completion.0 = Some(result);
                    if let Some(waker) = completion.1.take() {
                        waker.wake();
                    }
                });
            });
        })
        .map_err(|error| error.to_string())?;
    std::future::poll_fn(move |cx| {
        let mut completion = completion.borrow_mut();
        if let Some(result) = completion.0.take() {
            std::task::Poll::Ready(result)
        } else {
            completion.1 = Some(cx.waker().clone());
            std::task::Poll::Pending
        }
    })
    .await
}

/// Use the existing 1200-ms quiet tail; timers only inspect state and never draw or notify.
pub(super) fn start(backend: &Backend, cx: &mut Context<Backend>) -> Probe {
    let result = Probe::default();
    let completion = result.clone();
    let window = backend.assets_window;
    cx.defer(move |cx| {
        let Some(window) = window else {
            *completion.0.borrow_mut() = Some(Err("F1 needs the existing Assets window".into()));
            return;
        };
        let installed = window.update(cx, |root, window, cx| {
            let assets = root.view().clone().downcast::<AssetsView>()
                .map_err(|_| "F1 Assets root has the wrong view")?;
            cx.set_inspector_panel_width(Some(px(0.0)));
            window.toggle_inspector(cx);
            let form = cx.new(|cx| Form::new(window, cx));
            assets.update(cx, |assets, cx| { assets.firetest_f1 = Some(form.clone()); cx.notify(); });
            Ok::<_, &str>(form)
        });
        let form = match installed {
            Ok(Ok(form)) => form,
            _ => { *completion.0.borrow_mut() = Some(Err("F1 could not install in Assets".into())); return; }
        };
        cx.spawn(async move |cx| {
            let check = async {
                cx.background_executor().timer(Duration::from_millis(150)).await;
                let initial = window.update(cx, |_, window, cx| {
                    let state = form.read(cx);
                    painted(window, state.width.get(), BEFORE, 240.0)?;
                    Ok::<_, String>(state.renders)
                }).map_err(|error| error.to_string())??;
                firetest_info(&format!("[firetest] f1_native baseline painted_text=PASS panel_width=240 renders={initial}"));
                window.update(cx, |_, _, cx| form.update(cx, |form, cx| form.arm(false, cx)))
                    .map_err(|error| error.to_string())?;
                for repeat in [false, true] {
                    cx.background_executor().timer(Duration::from_millis(150)).await;
                    window.update(cx, |_, window, cx| {
                        let state = form.read(cx);
                        painted(window, state.width.get(), AFTER, 160.0)
                    }).map_err(|error| error.to_string())??;
                    quiet(window, form.clone(), cx).await?;
                    firetest_info(&format!("[firetest] f1_native repeat={repeat} painted_text=PASS panel_width=160 quiet_native_callbacks=2 quiet_renders=0"));
                    if !repeat {
                        window.update(cx, |_, _, cx| form.update(cx, |form, cx| form.arm(true, cx)))
                            .map_err(|error| error.to_string())?;
                    }
                }
                Ok(())
            }.await;
            *completion.0.borrow_mut() = Some(check);
        }).detach();
    });
    result
}
