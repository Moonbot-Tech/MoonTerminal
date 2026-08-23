//! Burning the header strip into the captured picture, with Win32 GDI.
//!
//! Deliberately a SEPARATE pass from `super::win`, and that separation is the design rather than
//! tidiness. `capture_client_rect` is one auditable claim — *read exactly this rectangle off the
//! desktop, nothing else* — and it sits on the privacy path (`super::caption`), where the question
//! a reader must be able to answer quickly is "can anything reach the desktop read before the
//! substituted caption is proven drawn?". Growing that function with fonts, a brush, a
//! measurement loop and a clip rule would triple its length and mix cosmetics into the one place
//! that must stay easy to check.
//!
//! GDI rather than a Rust text stack, and that was a real choice: the code is already inside a
//! memory DC with a bitmap selected, and GDI is the platform's own text engine — it does system
//! font FALLBACK for whatever alphabet an exchange put in a ticker, with no new crate, no shipped
//! font file, and the same ClearType rendering the rest of the application shows. `tiny-skia` plus
//! `cosmic-text` would be two new direct dependencies that then have to be taught to find a font.
//!
//! # What this module owns, and what it deliberately does not
//!
//! It owns the parts that need a device context: creating the three faces, measuring a run,
//! placing a baseline, filling a rectangle, writing the text. It owns NO arithmetic and NO colour.
//! The ranking and the clip order are `super::header`'s, the sizes and the strip's height are
//! `super::resize`'s, and every colour is derived by `super::ink` — all three platform-neutral, so
//! that the parts of this pass a test can actually check are checked on every platform rather than
//! on none.
//!
//! The pass ends in `super::win::read_dib`, the same `GetDIBits` call the capture uses, so the
//! composed picture is a valid `CF_DIB` body by construction rather than by a second agreement.

use anyhow::{Context as _, bail};
use windows::Win32::Foundation::{COLORREF, RECT, SIZE};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CLEARTYPE_QUALITY, CreateCompatibleBitmap,
    CreateCompatibleDC, CreateFontIndirectW, CreateSolidBrush, DEFAULT_CHARSET, DIB_RGB_COLORS,
    DeleteObject, FF_DONTCARE, FONT_WEIGHT, FW_NORMAL, FW_SEMIBOLD, FillRect, GetDC,
    GetTextExtentPoint32W, GetTextMetricsW, HDC, HFONT, HGDIOBJ, LOGFONTW, OUT_DEFAULT_PRECIS,
    SelectObject, SetBkMode, SetDIBitsToDevice, SetTextAlign, SetTextColor, TA_BASELINE, TA_LEFT,
    TEXTMETRICW, TRANSPARENT, TextOutW,
};

use super::header::{Gaps, LeadGap, Measured, RunStyle, ShotStrip, StripField};
use super::ink::Palette;
use super::win::{Bitmap, DibImage, MemoryDc, ScreenDc, read_dib, restore_selection};

/// What the header strip is drawn in: the chart's own colours.
///
/// Read off the panel at capture time rather than themed here, so the burnt-in line cannot end up
/// in colours the chart beneath it never used. These two are the INPUTS to `super::ink`, not the
/// colours drawn: the strip needs a band, a rule and two text registers that all hold against a
/// background the user chose, and deriving those is that module's job.
pub(super) struct ShotStyle {
    /// The chart's background, which the strip's own band is lifted off.
    pub(super) bg: [u8; 3],
    /// The chart's supporting-text colour.
    ///
    /// Deliberately NOT a hard-coded dark: the intent is "the chart's own background, no
    /// decoration", and `ChartTheme.bg` is user-configurable and dark by default, so a fixed dark
    /// text would be invisible for most users. Taking the colour the chart already writes its axis
    /// labels in satisfies the intent in every theme, light one included.
    pub(super) text: [u8; 3],
}

/// Compose `body` with a header strip above it.
///
/// Args:
///     body: The captured chart, already normalized to its final size. Not resampled here — the
///         strip is drawn at final resolution precisely so its text is never resampled.
///     header: The header's fields, grouped by clip priority.
///     style: The chart's own colours.
///
/// Returns:
///     A taller picture in DIB layout, or an error naming the GDI step that failed.
pub(super) fn draw_strips(
    body: &DibImage,
    header: &ShotStrip,
    style: &ShotStyle,
) -> anyhow::Result<DibImage> {
    if body.width == 0 || body.height == 0 {
        bail!("nothing to compose: the captured picture has no area");
    }
    // Every one of these comes from `super::resize`, which is where the SIZE RULE lives and
    // therefore the one module that has to know how tall this strip will be: it shortens the
    // body's box by exactly `resize::HEADER_RESERVE_PX` so the composed picture still fits the
    // messenger's bound. A second copy of any of this arithmetic here is what would let the two
    // silently disagree, and the symptom would be a recompressed picture rather than a crash.
    let base_px = super::resize::font_px(body.width);
    let lead_px = super::resize::lead_px(base_px);
    let pad = super::resize::strip_pad(base_px);
    let strip_h = super::resize::strip_height(base_px);
    let gaps = Gaps {
        field: super::resize::field_gap(base_px) as i32,
        group: super::resize::group_gap(base_px) as i32,
    };

    let total_h = body
        .height
        .checked_add(strip_h)
        .context("composed height overflow")?;
    let width = i32::try_from(body.width).context("composed width does not fit in i32")?;
    let height = i32::try_from(total_h).context("composed height does not fit in i32")?;

    let screen = unsafe { GetDC(None) };
    if screen.is_invalid() {
        bail!("GetDC(None) returned no desktop DC");
    }
    let screen = ScreenDc(screen);

    let memory = unsafe { CreateCompatibleDC(Some(screen.0)) };
    if memory.is_invalid() {
        bail!("CreateCompatibleDC failed");
    }
    let memory = MemoryDc(memory);

    // Compatible with the SCREEN DC, never the memory DC, for the same reason the capture is: a
    // fresh memory DC holds a 1x1 MONOCHROME bitmap, and asking it for a compatible one yields a
    // black-and-white composition.
    let bitmap = unsafe { CreateCompatibleBitmap(screen.0, width, height) };
    if bitmap.is_invalid() {
        bail!("CreateCompatibleBitmap failed for {width}x{height}");
    }
    let bitmap = Bitmap(bitmap);
    let previous_bitmap = unsafe { SelectObject(memory.0, bitmap.0.into()) };
    // Checked, unlike every other selection in this pass, because this ONE failure is SILENT. A
    // fresh memory DC starts with a 1x1 monochrome bitmap selected; if the composition bitmap is
    // refused, `SelectObject` answers NULL, drawing continues into that 1x1 default, and the read
    // below comes back with the untouched composition bitmap. The picture then reaches the
    // clipboard and the user is told the shot succeeded. Failing loudly is the only honest
    // outcome, and it is also the file's own convention: every other GDI call here is checked.
    if previous_bitmap.is_invalid() {
        bail!("SelectObject refused the {width}x{height} composition bitmap");
    }

    let composed = compose(
        memory.0,
        body,
        header,
        style,
        Layout {
            width,
            height,
            strip_h: strip_h as i32,
            hairline: super::resize::HAIRLINE_PX as i32,
            base_px: base_px as i32,
            lead_px: lead_px as i32,
            pad: pad as i32,
            gaps,
        },
    );

    // Deselected BEFORE the read, and on the error path too: `GetDIBits` documents that its bitmap
    // may not be selected into any DC, and a still-selected one returns zero rows rather than an
    // error.
    restore_selection(memory.0, previous_bitmap);
    composed?;
    read_dib(screen.0, bitmap.0, body.width, total_h)
}

/// Pixel geometry of one composition, resolved once so the drawing steps do not each recompute it.
struct Layout {
    width: i32,
    height: i32,
    strip_h: i32,
    hairline: i32,
    base_px: i32,
    lead_px: i32,
    pad: i32,
    gaps: Gaps,
}

/// One GDI font, released when it goes out of scope.
///
/// RAII rather than a hand-rolled delete because the strip now needs THREE faces and the old
/// single-font code released its one on exactly two paths. Three faces and four error paths is
/// where a leak stops being reviewable by eye — and a leaked `HFONT` is a process-lifetime GDI
/// handle, not memory a drop of the picture reclaims.
struct Font(HFONT);

impl Font {
    /// Ask GDI for one face.
    ///
    /// Args:
    ///     px: Character height in pixels.
    ///     weight: The stroke weight.
    ///
    /// Returns:
    ///     The face, or an error naming the failure.
    fn create(px: i32, weight: FONT_WEIGHT) -> anyhow::Result<Self> {
        let font = unsafe { CreateFontIndirectW(&logfont(px, weight)) };
        if font.is_invalid() {
            bail!("CreateFontIndirectW failed for {px}px weight {}", weight.0);
        }
        Ok(Self(font))
    }
}

impl Drop for Font {
    fn drop(&mut self) {
        // Must not still be selected into a DC when this runs: `DeleteObject` on a selected font
        // is a documented no-op, and the handle would leak silently.
        //
        // What guarantees that, and what does NOT: `compose` binds `write_strip`'s result instead
        // of `?`-ing it, so the `restore_selection` below runs before these faces drop on every
        // path that RETURNS — success and error alike. It does not run on an UNWIND. That gap is
        // real and currently unreachable: the only panic between the selection and the restore is
        // `place_body`'s `expect` on `size_of::<BITMAPINFOHEADER>()`, a compile-time constant of
        // 40. Anyone adding a fallible or panicking call inside that window owes it a guard type
        // that restores the selection in its own `Drop`; a bare `?` is what this design already
        // avoids and a `panic!` is what it does not yet cover.
        let _ = unsafe { DeleteObject(HGDIOBJ(self.0.0)) }.ok();
    }
}

/// The three faces the strip is set in.
///
/// Two SIZES and two WEIGHTS, which is the whole type scale: the coin larger and semibold, the
/// figures a reader scans for semibold at the base size, everything else regular at the base size.
/// A fourth face was not added on purpose — hierarchy comes from contrast between few registers,
/// and a header with five distinctions has none.
struct Faces {
    lead: Font,
    primary: Font,
    secondary: Font,
}

impl Faces {
    /// Create all three.
    ///
    /// Args:
    ///     base_px: Character height for everything but the coin.
    ///     lead_px: Character height for the coin.
    ///
    /// Returns:
    ///     The faces, or an error naming the one that failed.
    fn create(base_px: i32, lead_px: i32) -> anyhow::Result<Self> {
        Ok(Self {
            lead: Font::create(lead_px, FW_SEMIBOLD)?,
            primary: Font::create(base_px, FW_SEMIBOLD)?,
            secondary: Font::create(base_px, FW_NORMAL)?,
        })
    }

    /// The face one role is set in.
    ///
    /// Args:
    ///     style: The run's role.
    ///
    /// Returns:
    ///     The face to select before measuring or drawing it.
    fn for_style(&self, style: RunStyle) -> HFONT {
        match style {
            RunStyle::Lead => self.lead.0,
            RunStyle::Primary => self.primary.0,
            RunStyle::Secondary => self.secondary.0,
        }
    }
}

/// Fill the background, place the body, band the strip, and write the header into `dc`.
///
/// Split from [`draw_strips`] so the bitmap deselection above runs on every path out of here,
/// including the error ones.
///
/// Args:
///     dc: Memory DC with the composition bitmap selected.
///     body: The captured chart in DIB layout.
///     header: The header's fields.
///     style: The chart's own colours.
///     layout: Resolved pixel geometry.
///
/// Returns:
///     `Ok(())` once everything has been drawn.
fn compose(
    dc: HDC,
    body: &DibImage,
    header: &ShotStrip,
    style: &ShotStyle,
    layout: Layout,
) -> anyhow::Result<()> {
    let ink = super::ink::palette(style.bg, style.text);

    fill_rect(dc, style.bg, whole(layout.width, layout.height))?;
    place_body(dc, body, layout.strip_h)?;
    // The band goes down only as far as the rule, and the rule owns the strip's last row. Drawn
    // AFTER the body so a one-pixel disagreement between the two shows up as a visible seam rather
    // than as a row of chart quietly painted over.
    fill_rect(
        dc,
        ink.band,
        band_rect(layout.width, layout.strip_h - layout.hairline),
    )?;
    fill_rect(
        dc,
        ink.hairline,
        hairline_rect(layout.width, layout.strip_h, layout.hairline),
    )?;

    let faces = Faces::create(layout.base_px, layout.lead_px)?;
    let previous_font = unsafe { SelectObject(dc, faces.lead.0.into()) };
    unsafe {
        SetBkMode(dc, TRANSPARENT);
        // Every run is drawn at ONE baseline, so mixed sizes sit on a common line instead of on
        // their own boxes' tops. This REDEFINES the `y` handed to every `TextOutW` below — the
        // old top-relative centring expression is gone for exactly that reason.
        SetTextAlign(dc, TA_BASELINE | TA_LEFT);
    }

    // BOUND rather than `?`-ed: the DC's previous selection must be restored on the failing path
    // too, and an early return here would drop `faces` while one of them is still selected, which
    // makes their `DeleteObject` a silent no-op and leaks the handle.
    let header_result = write_strip(dc, header, &faces, &ink, 0, &layout);

    restore_selection(dc, previous_font);

    header_result
}

/// The whole composition.
///
/// Args:
///     width: Composition width in pixels.
///     height: Composition height in pixels.
///
/// Returns:
///     The rectangle.
fn whole(width: i32, height: i32) -> RECT {
    RECT {
        left: 0,
        top: 0,
        right: width,
        bottom: height,
    }
}

/// The strip's band, down to but not including the rule.
///
/// Args:
///     width: Composition width in pixels.
///     bottom: Where the band stops.
///
/// Returns:
///     The rectangle.
fn band_rect(width: i32, bottom: i32) -> RECT {
    RECT {
        left: 0,
        top: 0,
        right: width,
        bottom,
    }
}

/// The rule along the strip's BOTTOM edge, and nowhere else.
///
/// One edge, because the strip sits at the picture's top: a rule above it would draw on the
/// picture's own border, and a box around the caption is decoration rather than hierarchy.
///
/// Args:
///     width: Composition width in pixels.
///     strip_h: The strip's height.
///     hairline: The rule's thickness.
///
/// Returns:
///     The rectangle.
fn hairline_rect(width: i32, strip_h: i32, hairline: i32) -> RECT {
    RECT {
        left: 0,
        top: strip_h - hairline,
        right: width,
        bottom: strip_h,
    }
}

/// Paint one rectangle in one colour.
///
/// Args:
///     dc: Memory DC with the composition bitmap selected.
///     color: The colour to fill with.
///     rect: The area to fill.
///
/// Returns:
///     `Ok(())` once the fill has been applied.
fn fill_rect(dc: HDC, color: [u8; 3], rect: RECT) -> anyhow::Result<()> {
    let brush = unsafe { CreateSolidBrush(colorref(color)) };
    if brush.is_invalid() {
        bail!("CreateSolidBrush failed");
    }
    let filled = unsafe { FillRect(dc, &rect, brush) };
    let _ = unsafe { DeleteObject(HGDIOBJ(brush.0)) }.ok();
    if filled == 0 {
        bail!("FillRect failed");
    }
    Ok(())
}

/// Copy the captured chart into the band below the strip, at 1:1.
///
/// Never stretched: the normalizer (`super::resize`) has already put the picture at its final size
/// with a filter chosen for the job, and a second resample here would undo exactly that.
///
/// Args:
///     dc: Memory DC with the composition bitmap selected.
///     body: The captured chart in DIB layout.
///     top: Where the body starts, i.e. the header strip's height.
///
/// Returns:
///     `Ok(())` once the pixels are in place.
fn place_body(dc: HDC, body: &DibImage, top: i32) -> anyhow::Result<()> {
    let width = i32::try_from(body.width).context("body width does not fit in i32")?;
    let height = i32::try_from(body.height).context("body height does not fit in i32")?;
    // Checked immediately before the raw pointer crosses into GDI. `SetDIBitsToDevice` sizes its
    // read from the `BITMAPINFO` below, which is derived from the SAME width and height, so a
    // buffer shorter than they describe is an out-of-bounds read inside the OS rather than a Rust
    // panic. Nothing in `DibImage`'s type prevents that, and this pass is one of two producers.
    if !body.is_consistent() {
        bail!(
            "refusing to compose a {}x{} body whose pixel array is {} bytes",
            body.width,
            body.height,
            body.rows.len()
        );
    }
    let info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: u32::try_from(std::mem::size_of::<BITMAPINFOHEADER>())
                .expect("BITMAPINFOHEADER fits in u32"),
            biWidth: width,
            // POSITIVE, matching how `super::win` asked for these rows: bottom-up.
            biHeight: height,
            biPlanes: 1,
            biBitCount: 24,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let copied = unsafe {
        SetDIBitsToDevice(
            dc,
            0,
            top,
            body.width,
            body.height,
            0,
            0,
            0,
            body.height,
            body.rows.as_ptr().cast(),
            &info,
            DIB_RGB_COLORS,
        )
    };
    if copied != height {
        bail!("SetDIBitsToDevice placed {copied} of {height} scan lines");
    }
    Ok(())
}

/// Write the strip's surviving fields, CENTRED, across the line at `top`.
///
/// Two steps, in this order and never the other one. **Fit**: the head never drops, and the tail
/// fills what is left, whole fields only, cut from the right — its order IS the priority, decided
/// in `super::header`, and each field is charged its own leading gap so a group boundary really
/// does cost more room. **Then centre**: the run that actually survived is placed by
/// `super::header::centred_start_x`, so the text is centred around the width it occupies rather
/// than the width it would have occupied unclipped.
///
/// The arithmetic is in `super::header` rather than here because this module is `#[cfg(windows)]`
/// — a formula placed here is a formula no test on another platform ever runs. Here there is only
/// the measuring, which needs a device context, and the drawing.
///
/// A run too wide even after clipping — reachable only when the head alone overflows a very narrow
/// pane — starts at the inset and is clipped by GDI at the bitmap edge. Nothing corrupts, and
/// because the coin is drawn FIRST it is the field that survives.
///
/// Args:
///     dc: Memory DC with the composition bitmap and the lead face selected.
///     strip: The fields to draw.
///     faces: The three faces the runs are set in.
///     ink: The strip's derived colours.
///     top: The strip's top edge in composition pixels.
///     layout: Resolved pixel geometry.
///
/// Returns:
///     `Ok(())` once every surviving field has been written.
fn write_strip(
    dc: HDC,
    strip: &ShotStrip,
    faces: &Faces,
    ink: &Palette,
    top: i32,
    layout: &Layout,
) -> anyhow::Result<()> {
    let inset = layout.gaps.field;
    let baseline = top + layout.pad + ascent(dc, layout.lead_px);
    let avail = layout.width - 2 * inset;

    let head = measure(dc, faces, &strip.head)?;
    let tail = measure(dc, faces, &strip.tail)?;

    let fitted = super::header::fit_tail(&sized(&head), &sized(&tail), layout.gaps, avail);

    // The run AS DRAWN — head plus the tail fields that survived — is what gets centred. Built
    // once and reused for both the measurement and the drawing, so the two cannot disagree.
    let drawn: Vec<&MeasuredField> = head.iter().chain(tail.iter().take(fitted)).collect();
    let placed: Vec<Measured> = drawn.iter().map(|field| field.sized()).collect();
    let mut x = super::header::centred_start_x(&placed, layout.gaps, layout.width, inset);
    for (index, field) in drawn.iter().enumerate() {
        if index > 0 {
            // The SAME function `fit_tail` and `centred_start_x` charged. Re-typing the match here
            // would let the measurement and the drawing disagree about what a group boundary costs,
            // and the symptom — a line centred around a width it does not occupy — looks like a
            // centring bug rather than a spacing one.
            x += super::header::lead_width(field.lead_gap, layout.gaps);
        }
        for run in &field.runs {
            unsafe {
                SelectObject(dc, faces.for_style(run.style).into());
                SetTextColor(dc, colorref(colour(ink, run.style)));
            }
            draw(dc, x, baseline, &run.text)?;
            x += run.width;
        }
    }
    Ok(())
}

/// Where the baseline sits below the strip's top padding.
///
/// Asked of the platform rather than assumed, because it is what makes two sizes share one line:
/// the smaller runs are placed by the LEAD face's ascent, so their own boxes are irrelevant.
/// A failure falls back to `super::resize::ascent_fallback_px` and warns — a baseline a pixel or
/// two off is a far better outcome than refusing to produce the picture the user pressed for.
///
/// Args:
///     dc: Memory DC with the LEAD face selected.
///     lead_px: The lead character height, for the fallback.
///
/// Returns:
///     The ascent in pixels.
fn ascent(dc: HDC, lead_px: i32) -> i32 {
    let mut metrics = TEXTMETRICW::default();
    if unsafe { GetTextMetricsW(dc, &mut metrics) }.as_bool() {
        return metrics.tmAscent;
    }
    log::warn!("GetTextMetricsW failed for the shot header; placing the baseline by ratio");
    super::resize::ascent_fallback_px(lead_px.max(0) as u32) as i32
}

/// One measured run: its UTF-16 text, its rendered width and the role that chose its face.
struct MeasuredRun {
    text: Vec<u16>,
    width: i32,
    style: RunStyle,
}

/// One measured field: its runs, its total width and the space charged in front of it.
struct MeasuredField {
    runs: Vec<MeasuredRun>,
    width: i32,
    lead_gap: LeadGap,
}

impl MeasuredField {
    /// This field as the layout arithmetic wants it.
    ///
    /// Returns:
    ///     Its width and its leading gap.
    fn sized(&self) -> Measured {
        Measured {
            width: self.width,
            lead_gap: self.lead_gap,
        }
    }
}

/// Reduce measured fields to what `super::header`'s formulas take.
///
/// Args:
///     fields: The measured fields.
///
/// Returns:
///     One [`Measured`] per field, in the same order.
fn sized(fields: &[MeasuredField]) -> Vec<Measured> {
    fields.iter().map(MeasuredField::sized).collect()
}

/// Convert each run to UTF-16 and ask GDI how wide it renders IN ITS OWN FACE.
///
/// The face is selected per run rather than per field: a movement figure is set in a different
/// weight from the window token in front of it, and measuring both in one face would put every
/// later field on the line at the wrong x.
///
/// Args:
///     dc: Memory DC.
///     faces: The three faces.
///     fields: The fields to measure.
///
/// Returns:
///     Each field's runs beside their rendered widths, and the field's total width.
fn measure(dc: HDC, faces: &Faces, fields: &[StripField]) -> anyhow::Result<Vec<MeasuredField>> {
    fields
        .iter()
        .map(|field| {
            let mut runs = Vec::with_capacity(field.runs.len());
            let mut width = 0i32;
            for run in &field.runs {
                let text: Vec<u16> = run.text.encode_utf16().collect();
                let mut size = SIZE::default();
                unsafe { SelectObject(dc, faces.for_style(run.style).into()) };
                let measured = unsafe { GetTextExtentPoint32W(dc, &text, &mut size) };
                if !measured.as_bool() {
                    bail!("GetTextExtentPoint32W failed");
                }
                width = width.saturating_add(size.cx);
                runs.push(MeasuredRun {
                    text,
                    width: size.cx,
                    style: run.style,
                });
            }
            Ok(MeasuredField {
                runs,
                width,
                lead_gap: field.lead_gap,
            })
        })
        .collect()
}

/// The colour one role is written in.
///
/// Args:
///     ink: The strip's derived colours.
///     style: The run's role.
///
/// Returns:
///     The colour. `Lead` and `Primary` share one by design — the coin is distinguished by size
///     and weight, not by a second accent.
fn colour(ink: &Palette, style: RunStyle) -> [u8; 3] {
    match style {
        RunStyle::Lead => ink.lead,
        RunStyle::Primary => ink.primary,
        RunStyle::Secondary => ink.secondary,
    }
}

/// Write one already-measured run.
///
/// Args:
///     dc: Memory DC with that run's face and colour selected.
///     x: Left edge in composition pixels.
///     y: The shared BASELINE, not a top edge — see the `SetTextAlign` call in [`compose`].
///     text: The run's UTF-16 text.
///
/// Returns:
///     `Ok(())` once the text has been written.
fn draw(dc: HDC, x: i32, y: i32, text: &[u16]) -> anyhow::Result<()> {
    if text.is_empty() {
        return Ok(());
    }
    if !unsafe { TextOutW(dc, x, y, text) }.as_bool() {
        bail!("TextOutW failed");
    }
    Ok(())
}

/// One face's description.
///
/// `lfFaceName` is left EMPTY on purpose: that asks GDI for the system's own default UI face and,
/// with it, the system's font-fallback chain. It is the entire reason this module draws with GDI
/// rather than a Rust text stack — a ticker or a venue caption can arrive in an alphabet no single
/// bundled font covers, and falling back is the platform's job, not ours.
///
/// `CLEARTYPE_QUALITY` is not decoration either: GDI's default quality is ALIASED, and aliased
/// text at this size is the single loudest thing that makes a burnt-in caption look like a
/// screenshot from two decades ago.
///
/// Args:
///     px: Character height in pixels.
///     weight: The stroke weight.
///
/// Returns:
///     The font description to hand `CreateFontIndirectW`.
fn logfont(px: i32, weight: FONT_WEIGHT) -> LOGFONTW {
    LOGFONTW {
        // NEGATIVE asks for a CHARACTER height rather than a cell height, which is what makes the
        // strip's padding arithmetic in `super::resize` mean what it says.
        lfHeight: -px,
        lfWeight: weight.0 as i32,
        lfCharSet: DEFAULT_CHARSET,
        lfOutPrecision: OUT_DEFAULT_PRECIS,
        lfQuality: CLEARTYPE_QUALITY,
        lfPitchAndFamily: FF_DONTCARE.0,
        ..Default::default()
    }
}

/// Pack an RGB triple into GDI's `COLORREF`, which is `0x00BBGGRR`.
///
/// Args:
///     rgb: The colour as the theme states it.
///
/// Returns:
///     The same colour in the byte order GDI reads.
fn colorref(rgb: [u8; 3]) -> COLORREF {
    COLORREF(u32::from(rgb[0]) | (u32::from(rgb[1]) << 8) | (u32::from(rgb[2]) << 16))
}
