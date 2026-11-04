//! Test bench for closed-menu widths: lays real triggers out in every language and font step.
//!
//! The surfaces this guards hold a `Backend` no test can build, so the bench draws the SAME
//! `MoonDropdown` builder each surface draws — same id, size and width policy — inside a host of
//! the surface's own width, and reads the result back through `debug_bounds`.

use gpui::{
    AnyElement, Bounds, Context, IntoElement, ParentElement, Pixels, Render, Styled,
    VisualTestContext, Window, px,
};
use moon_core::config::{Language, UiThemeMode};
use moon_ui::MoonTheme;

/// Builds the probed element for one frame.
type Build = Box<dyn Fn(&mut Window, &mut gpui::App) -> AnyElement>;

/// Host view rendering whatever its builder returns.
struct Probe(Build);

impl Render for Probe {
    /// Draw the builder's element on a plain root.
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        gpui::div().size_full().child((self.0)(window, cx))
    }
}

/// One laid-out frame: where each instrumented element landed, plus the app to measure with.
pub(crate) struct Frame<'a> {
    visual: &'a mut VisualTestContext,
    /// Language, theme and font step the frame was drawn at, for assertion messages.
    pub(crate) at: String,
}

impl Frame<'_> {
    /// Bounds of the element tagged `selector`, panicking when it was not laid out.
    pub(crate) fn bounds(&mut self, selector: &str) -> Bounds<Pixels> {
        let leaked: &'static str = Box::leak(selector.to_string().into_boxed_str());
        self.visual
            .debug_bounds(leaked)
            .unwrap_or_else(|| panic!("{selector} not laid out: {}", self.at))
    }

    /// Run `f` against the frame's app, for measurements in the active theme.
    pub(crate) fn measure<R>(&mut self, f: impl FnOnce(&gpui::App) -> R) -> R {
        self.visual.update(|_, app| f(app))
    }

    /// Assert the trigger tagged `selector` is wide enough for `label` plus its caret and padding.
    ///
    /// The oracle is the label measured on its own at the trigger's mono 14-pixel design size,
    /// caret appended, plus the two 7-pixel paddings — independent of the fit under test.
    pub(crate) fn assert_holds(&mut self, selector: &str, label: &str) {
        let cell = self.bounds(selector);
        let need = self.measure(|app| {
            crate::design::ui_text_width(app, &format!("{label} \u{25be}"), 14.0, 400.0, true)
        });
        assert!(
            cell.size.width + px(0.5) >= px(need + 14.0),
            "closed menu cuts {label:?} ({:?} < {need}+14): {}",
            cell.size.width,
            self.at
        );
    }

    /// Assert the element tagged `inner` stays horizontally inside the element tagged `outer`.
    pub(crate) fn assert_inside(&mut self, inner: &str, outer: &str) {
        let a = self.bounds(inner);
        let b = self.bounds(outer);
        assert!(
            a.origin.x + px(0.5) >= b.origin.x && a.right() <= b.right() + px(0.5),
            "{inner} {a:?} leaves {outer} {b:?}: {}",
            self.at
        );
    }
}

/// Draw `build` once per shipped language, at the standard and the shipped font step, and hand
/// each laid-out frame to `check`.
///
/// Args:
///     cx: The gpui test context.
///     build: Builds the probed element for a frame; called on every frame.
///     check: Inspects each laid-out frame.
pub(crate) fn every_locale(
    cx: &mut gpui::TestAppContext,
    build: impl Fn(&mut Window, &mut gpui::App) -> AnyElement + Clone + 'static,
    mut check: impl FnMut(&mut Frame<'_>),
) {
    crate::test_locale::warm();
    cx.update(moon_ui::init);
    for delta in [0.0, crate::design::DESIGN_FONT_DELTA] {
        cx.update(|app| {
            MoonTheme::install_config(
                crate::startup::moon_theme_config_for_presentation(UiThemeMode::Light, 1.0)
                    .with_font_delta(delta),
                app,
            )
        });
        for language in Language::ALL {
            let _locale = crate::test_locale::force(language.code());
            let build = build.clone();
            let (_, visual) =
                cx.add_window_view(move |_, _| Probe(Box::new(move |w, a| build(w, a))));
            visual.simulate_resize(gpui::size(px(1600.0), px(900.0)));
            for _ in 0..2 {
                visual.update(|window, app| {
                    let _ = window.draw(app);
                });
            }
            let mut frame = Frame {
                visual,
                at: format!("{} font+{delta}", language.code()),
            };
            check(&mut frame);
            frame.visual.update(|window, _| window.remove_window());
        }
    }
}
