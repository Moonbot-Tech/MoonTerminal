//! Cached editor-state factories for strategy field inputs, memos, and colors.

use super::*;

impl StrategiesView {
    pub(super) fn field_input_state(
        &mut self,
        id: String,
        value: String,
        keys: Arc<Vec<Key>>,
        field: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<MoonInputState> {
        if let Some(state) = self.field_inputs.get(&id) {
            let cur = state.read(cx).value().to_string();
            if cur == value {
                return state.clone();
            }
            // The cache may lag a server echo or an edit made through another selection while
            // `value` already includes the current draft. Synchronize silently; `sync_value` does
            // not emit Change and therefore cannot loop staged edits.
            let state = state.clone();
            state.update(cx, |s, cx| s.sync_value(value.clone(), cx));
            return state;
        }
        let state = cx.new(|cx| MoonInputState::new(window, cx).default_value(value));
        cx.subscribe(&state, move |this, state, ev: &MoonInputEvent, cx| {
            if matches!(ev, MoonInputEvent::Change) {
                let value = state.read(cx).value().to_string();
                this.stage_field_value(keys.as_ref(), &field, value, cx);
            }
        })
        .detach();
        self.field_inputs.insert(id, state.clone());
        state
    }

    pub(super) fn field_memo_state(
        &mut self,
        id: String,
        value: String,
        keys: Arc<Vec<Key>>,
        field: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<MoonTextAreaState> {
        if let Some(state) = self.field_memos.get(&id) {
            let cur = state.read(cx).value().to_string();
            if cur == value {
                return state.clone();
            }
            // As in `field_input_state`, synchronize silently to the current value.
            let state = state.clone();
            state.update(cx, |s, cx| s.sync_value(value.clone(), cx));
            return state;
        }
        let state = cx.new(|cx| MoonTextAreaState::new(window, cx).default_value(value));
        cx.subscribe(&state, move |this, state, ev: &MoonTextAreaEvent, cx| {
            if matches!(ev, MoonTextAreaEvent::Change) {
                let value = state.read(cx).value().to_string();
                this.stage_field_value(keys.as_ref(), &field, value, cx);
            }
        })
        .detach();
        self.field_memos.insert(id, state.clone());
        state
    }

    /// Build a MoonUI swatch/palette picker for a Color field.
    /// A selection stages a new hexadecimal value while preserving its Moonbot `AARRGGBB` alpha
    /// prefix. States are cached by row id and recreated when the external RGB value changes.
    pub(super) fn field_color_state(
        &mut self,
        id: String,
        hex: &str,
        keys: Arc<Vec<Key>>,
        field: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<MoonColorPickerState> {
        let rgb_val = parse_hex_rgb(hex);
        if let Some((cached, state)) = self.field_colors.get(&id) {
            if *cached == rgb_val {
                return state.clone();
            }
        }
        let init: Hsla = gpui::rgb(design::rgb_to_u32(rgb_val)).into();
        let state = cx.new(|cx| MoonColorPickerState::new(window, cx).default_value(init));
        let prefix = hex_alpha_prefix(hex);
        cx.subscribe(&state, move |this, _st, ev: &MoonColorPickerEvent, cx| {
            let MoonColorPickerEvent::Change(h) = ev;
            let c = design::hsla_to_rgb8(*h);
            let hexv = format!("{prefix}{:02X}{:02X}{:02X}", c[0], c[1], c[2]);
            this.stage_field_value(keys.as_ref(), &field, hexv, cx);
        })
        .detach();
        self.field_colors.insert(id, (rgb_val, state.clone()));
        state
    }

    pub(super) fn append_formula_snippet(
        &mut self,
        field: &str,
        snippet: &str,
        cx: &mut Context<Self>,
    ) {
        let needle = field_id(field);
        let state = self
            .field_memos
            .iter()
            .find_map(|(id, state)| id.contains(&needle).then_some(state.clone()));
        let current = state
            .as_ref()
            .map(|state| state.read(cx).value().to_string())
            .unwrap_or_default();
        let next = append_snippet(&current, snippet);
        if let Some(state) = state {
            state.update(cx, |state, cx| state.sync_value(next, cx));
        }
    }
}
