//! Сигнатура входов чарт-вкладок (`chart_tabs_sig`) — дешёвый хэш состояния `Backend`, что реально
//! меняет полоску вкладок/стек этой группы. `ChartTabs` сравнивает его в backend-observe и
//! пропускает дорогой проход, если ничего не изменилось. Плюс мелкие хелперы группы.

use crate::Backend;
use moon_core::session::CoreId;

pub(super) fn chart_tabs_sig(b: &Backend, group: &str) -> u64 {
    let mut sig = if b
        .open_request
        .as_ref()
        .is_some_and(|(core, _)| core_belongs_to_group(b, group, *core))
    {
        b.open_request_rev
    } else {
        0
    };
    sig = sig
        .wrapping_mul(31)
        .wrapping_add(u64::from(b.config.charts_split_by_core));
    if b.price_scale_group.as_deref() == Some(group) {
        sig = sig.wrapping_mul(31).wrapping_add(b.price_scale_rev);
    }
    for (g, n, bucket) in &b.chart_repin_request {
        if g == group {
            sig = sig
                .wrapping_mul(31)
                .wrapping_add(*n as u64)
                .wrapping_mul(31)
                .wrapping_add(text_sig(&format!("{bucket:?}")));
        }
    }
    #[cfg(any(debug_assertions, moon_profile_debug, feature = "debug-tools"))]
    if b.debug_fill_main_chart_group.as_deref() == Some(group) {
        sig = sig
            .wrapping_mul(31)
            .wrapping_add(b.debug_fill_main_chart_rev);
    }
    let store = b.session.store();
    for s in b.session.sessions().iter().filter(|s| s.group == group) {
        if let Some(d) = store.core(s.id) {
            sig = sig.wrapping_mul(31).wrapping_add(d.detects_rev);
        }
    }
    sig
}

fn text_sig(text: &str) -> u64 {
    let mut sig = 0xcbf29ce484222325u64;
    for byte in text.bytes() {
        sig ^= byte as u64;
        sig = sig.wrapping_mul(0x100000001b3);
    }
    sig
}

pub(super) fn core_belongs_to_group(b: &Backend, group: &str, core: CoreId) -> bool {
    b.session
        .sessions()
        .iter()
        .any(|s| s.id == core && s.group == group)
}
