//! Summary chart axis helpers.

use super::*;

/// Which buckets keep a value label once the bars are denser than the labels are wide.
///
/// The LAST bucket is seeded before anything else, so the period's own end always carries its
/// number — including a zero one, which ordering by magnitude ranks dead last and which the
/// reader most wants to see, because "where did this period finish" is the question the right
/// edge of the chart answers. Every other candidate then has to clear that label rather than the
/// other way round, so the guarantee costs a neighbour rather than an overlap.
///
/// The rest is greedy by descending |value|: the biggest day is labelled next, and every later
/// candidate is kept only if its column sits at least one label-width from every column already
/// kept. That is what replaced the old all-or-nothing `days.len() <= 45` cutoff, which drew a
/// slab of colliding digits just under the limit and NOTHING at all just over it — so on «Все»
/// the chart said nothing about its own extremes.
///
/// Selecting by magnitude rather than by every N-th bucket is deliberate: the days worth naming
/// on a profit chart are the big ones, and an every-N-th rule names whichever days the stride
/// happens to land on.
///
/// Args:
///     vals: Per-bucket profit, ordered, one entry per bar.
///     plot_w: Width the bars are laid out across, in pixels.
///     label_w: Width of one label, in pixels.
///
/// Returns:
///     Indices into `vals` to label, ascending. Always contains the last index.
pub(in crate::analytics::summary) fn thinned_labels(
    vals: &[f64],
    plot_w: f32,
    label_w: f32,
) -> Vec<usize> {
    let n = vals.len();
    if n == 0 {
        return Vec::new();
    }
    let last = n - 1;
    // Bars are equal flex cells, so bucket `i` is centred at (i+0.5)/n of the width — the same
    // mapping the hover popup uses. The last one is the exception: its label is right-aligned to
    // the plot's edge instead of centred on its column (`daily_bars`), because a centred label
    // there would hang half its width outside the card. Its centre therefore sits half a label
    // in from that edge, and the separation test has to use THAT, or the neighbour it clears on
    // paper still collides on screen.
    let x = |i: usize| {
        if i == last {
            plot_w - label_w / 2.0
        } else {
            (i as f32 + 0.5) / n as f32 * plot_w
        }
    };
    let mut order: Vec<usize> = (0..n).collect();
    // Index as the tie-break, so two equal days never swap between frames.
    order.sort_by(|&a, &b| vals[b].abs().total_cmp(&vals[a].abs()).then(a.cmp(&b)));
    let mut kept: Vec<usize> = vec![last];
    for i in order {
        if i == last {
            continue;
        }
        if kept.iter().all(|&j| (x(i) - x(j)).abs() >= label_w) {
            kept.push(i);
        }
    }
    kept.sort_unstable();
    kept
}

/// FALLBACK color for a core's series (cycled from the palette) — used when
/// the server has no color in its settings (e.g. the core is already gone from
/// the config). The primary source is `ServerConfig.color` (see core_colors in
/// summary.rs).
pub(in crate::analytics::summary) fn fallback_core_color(p: MoonPalette, i: usize) -> u32 {
    [
        p.blue,
        p.green,
        p.orange,
        p.amber,
        p.red,
        p.yellow,
        p.accent,
        p.text_soft,
    ][i % 8]
}

/// Minimum hue separation, as a fraction of the wheel, before two core colours read as one.
///
/// `picker_palette` steps a full twelfth (0.083) between hues, so this admits every one of its
/// own swatches while rejecting a pair a user picked a few degrees apart.
const MIN_HUE_SEP: f32 = 0.04;
/// Minimum lightness separation that rescues an otherwise too-close hue pair. A dark blue beside
/// a pale blue IS two lines a reader can follow; two mid blues are not.
const MIN_L_SEP: f32 = 0.14;
/// Below this saturation a colour reads as grey and its hue carries no information, so greys are
/// separated by lightness alone.
const GREY_S: f32 = 0.15;
/// Saturation gap that separates two colours on its own. HSL puts a neutral grey and a saturated
/// red at the SAME hue (zero), so a hue test alone calls them identical when they are the easiest
/// pair on the chart to tell apart.
const MIN_S_SEP: f32 = 0.35;

/// Whether two core colours are too close to tell apart on a one-pixel line.
///
/// Deliberately a COARSE rule, not a perceptual distance: it decides only whether to keep a
/// user's own configured colour or to substitute a palette swatch, and a threshold nobody can
/// state is worse than one that is occasionally generous.
///
/// Args:
///     a: One already-taken colour.
///     b: The candidate colour.
///
/// Returns:
///     `true` when a reader would see one line where there are two.
pub(super) fn too_close(a: Hsla, b: Hsla) -> bool {
    if (b.l - a.l).abs() >= MIN_L_SEP {
        return false;
    }
    if (b.s - a.s).abs() >= MIN_S_SEP {
        return false;
    }
    if a.s < GREY_S && b.s < GREY_S {
        return true;
    }
    let d = (a.h - b.h).abs();
    d.min(1.0 - d) < MIN_HUE_SEP
}

/// Give every drawn core a colour a reader can actually tell from its neighbours.
///
/// The cumulative chart draws up to twelve thin per-core lines inside the total's area, and they
/// were all one blue: a core's colour comes from its own `ServerConfig.color`, and a user who
/// gives a whole exchange one colour gets one colour on the chart. So a configured colour is
/// KEPT — it is the identity the core selector and every popup dot already use — unless it
/// collides with one already taken, and only then is it replaced from `design::picker_palette`,
/// the very palette that colour was chosen from.
///
/// Walked in ascending `uid` order rather than in the caller's order, which is by PROFIT: a core
/// must not change colour because it had a better week.
///
/// Args:
///     configured: One entry per drawn core, in the caller's order — its uid and the RGB its
///         server config carries, or `None` when no config names it.
///     p: Active palette, for the last-resort cycle when the picker palette runs out.
///
/// Returns:
///     One colour per input entry in INPUT order. Colours stay distinguishable until the
///     separated picker swatches run out, then repeat deterministically by uid rank.
pub(in crate::analytics::summary) fn distinct_core_colors(
    configured: &[(u64, Option<[u8; 3]>)],
    p: MoonPalette,
) -> Vec<Hsla> {
    let rgb = |c: [u8; 3]| {
        Hsla::from(Rgba {
            r: f32::from(c[0]) / 255.0,
            g: f32::from(c[1]) / 255.0,
            b: f32::from(c[2]) / 255.0,
            a: 1.0,
        })
    };
    let mut order: Vec<usize> = (0..configured.len()).collect();
    order.sort_by_key(|&i| configured[i].0);
    let mut out = vec![None; configured.len()];
    let mut taken: Vec<Hsla> = Vec::with_capacity(configured.len());
    for &i in &order {
        if let Some(c) = configured[i].1.map(rgb)
            && !taken.iter().any(|&t| too_close(t, c))
        {
            taken.push(c);
            out[i] = Some(c);
        }
    }
    // Mid-shade first (index 2 of `SHADES`, the saturated one the picker leads with), then the
    // LIGHTEST and the DARKEST — never the neighbours. `SHADES` steps lightness by 0.12 between
    // adjacent entries, which is below `MIN_L_SEP`, so shades 1 and 3 would offer swatches
    // `too_close` rejects on sight and the usable pool would be twelve. 2 -> 0 -> 4 keeps every
    // step at 0.22 or more and gives thirty-six.
    let palette = design::picker_palette();
    let spare: Vec<Hsla> = [2usize, 0, 4]
        .into_iter()
        .flat_map(|shade| (0..12).map(move |hue| hue * 5 + shade))
        .filter_map(|ix| palette.get(ix).copied())
        .collect();
    let mut cursor = 0usize;
    for (rank, &i) in order.iter().enumerate() {
        if out[i].is_some() {
            continue;
        }
        let mut pick = None;
        while cursor < spare.len() {
            let candidate = spare[cursor];
            cursor += 1;
            if !taken.iter().any(|&t| too_close(t, candidate)) {
                pick = Some(candidate);
                break;
            }
        }
        let pick = pick.unwrap_or_else(|| {
            // Every separated swatch is spent — more than thirty-six cores need one. Colours
            // repeat from here, but by the core's RANK IN UID ORDER, so a core still keeps the
            // same colour between reloads; keying the repeat on the caller's profit-ordered
            // index would reshuffle the whole chart whenever the ranking moved.
            spare
                .get(rank % spare.len().max(1))
                .copied()
                .unwrap_or_else(|| moon(fallback_core_color(p, rank)))
        });
        taken.push(pick);
        out[i] = Some(pick);
    }
    out.into_iter().flatten().collect()
}

/// Label of one bucket: the HOUR when the grid is finer than a day (a single-day period),
/// the date otherwise. Without this every hourly bucket would be titled with the same date.
///
/// Args:
///     secs: Bucket start as UTC Unix seconds.
///     bucket: Civil bucket width in seconds.
///     zone: Selected IANA display zone.
///
/// Returns:
///     `HH:MM` for hourly grids or `DD.MM` for wider grids.
pub(in crate::analytics::summary) fn bucket_label(
    secs: i64,
    bucket: i64,
    zone: chrono_tz::Tz,
) -> String {
    if bucket < 86_400 {
        let s = moon_core::util::display_time::format_minute(secs, zone);
        // "YYYY-MM-DD HH:MM" → "HH:MM"
        if s.len() >= 16 {
            s[11..16].to_string()
        } else {
            s
        }
    } else {
        super::dm(secs, zone)
    }
}

/// A core's colour looked up by uid — the per-type popup knows uids, not series indices.
pub(in crate::analytics::summary) fn core_color_by_uid(
    cores: &[CoreSeries],
    colors: &[Hsla],
    uid: u64,
    p: MoonPalette,
) -> Hsla {
    match cores.iter().position(|c| c.uid == uid) {
        Some(ci) => core_color(colors, ci, p),
        None => moon(fallback_core_color(p, uid as usize)),
    }
}

/// Format UTC Unix seconds as selected-zone `DD.MM` for axis labels.
///
/// Args:
///     secs: Absolute UTC Unix seconds.
///     zone: Selected IANA display zone.
///
/// Returns:
///     Civil date label, or the shared formatter's fallback text.
pub(in crate::analytics::summary) fn dm(secs: i64, zone: chrono_tz::Tz) -> String {
    let s = moon_core::util::display_time::format_minute(secs, zone);
    if s.len() >= 10 {
        format!("{}.{}", &s[8..10], &s[5..7])
    } else {
        s
    }
}
