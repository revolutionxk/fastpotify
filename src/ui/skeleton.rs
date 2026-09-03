//! Placeholders shaped like the content that is coming.
//!
//! A page waiting on Spotify used to show one line reading "Loading…", which a
//! shelf two hundred pixels tall then shoved out of the way. These stand in at
//! the size of the real thing, so the page is laid out before it has anything
//! to say and nothing jumps when the answer arrives.

use egui::{CornerRadius, Rect, Sense, Ui, Vec2, pos2, vec2};

use crate::theme::{self, Palette, motion};

use super::widgets::{CARD_GAP, CARD_WIDTH};

/// One pass of the highlight across the window.
const SWEEP: f32 = 1.4;
/// Half the width of the bright band, in points.
const HIGHLIGHT: f32 = 220.0;
/// How wide each piece of a block is. Narrow enough that the band reads as a
/// gradient across it, wide enough to stay free.
const STRIP: f32 = 12.0;

/// The middle of the highlight, in screen coordinates.
///
/// Deliberately a function of where a block sits rather than of how long it
/// has been on screen: one wave crosses the whole window, so a page of
/// placeholders reads as a single surface catching the light instead of a
/// dozen things blinking out of step.
fn sweep(ui: &Ui) -> Option<f32> {
    let ctx = ui.ctx();
    if motion::reduced(ctx) {
        return None;
    }
    ctx.request_repaint();
    let (width, time) = ctx.input(|input| (input.viewport_rect().width(), input.time));
    let travel = width + 2.0 * HIGHLIGHT;
    Some(-HIGHLIGHT + travel * (time as f32 / SWEEP).rem_euclid(1.0))
}

/// The resting and lit shades. Both sit on the same side of the page's own
/// colour: a placeholder is always darker than the page in a light theme and
/// lighter in a dark one, so the band brightens a shape rather than punching
/// holes through it.
fn tones(palette: &Palette) -> (egui::Color32, egui::Color32) {
    if palette.dark {
        (palette.surface, palette.surface_active)
    } else {
        (palette.surface_active, palette.surface)
    }
}

/// How many pieces to draw a block in, or `None` for shapes whose rounding is
/// wider than a piece.
///
/// Only the outermost pieces carry any rounding, so a circle or a pill cannot
/// be built out of them: the ones in the middle would come out square. Those
/// are drawn whole instead, lit evenly by the band where their middle sits.
fn strips(width: f32, radius: f32) -> Option<usize> {
    let count = (width / STRIP).round().max(1.0);
    (radius < width / count).then_some(count as usize)
}

/// How brightly the band falls on `x`, easing off to nothing at its edges.
fn lit(sweep: Option<f32>, x: f32) -> f32 {
    let Some(sweep) = sweep else {
        return 0.0;
    };
    let distance = ((x - sweep).abs() / HIGHLIGHT).clamp(0.0, 1.0);
    let fall = 1.0 - distance;
    fall * fall * (3.0 - 2.0 * fall)
}

/// One placeholder shape.
///
/// Drawn as a run of pieces so the band can cross it, with only the outermost
/// two carrying the rounding. Clipping a gradient to a rounded rectangle is
/// not something egui offers, and painting one over the top would light the
/// corners the rounding cut away.
pub fn block(ui: &Ui, palette: &Palette, rect: Rect, radius: f32) {
    if !ui.is_rect_visible(rect) {
        return;
    }
    let (base, bright) = tones(palette);
    let sweep = sweep(ui);
    let radius = radius.min(rect.height() / 2.0).min(rect.width() / 2.0);
    let corner = radius.min(127.0) as u8;
    let painter = ui.painter();
    let Some(count) = strips(rect.width(), radius) else {
        let shade = motion::mix(base, bright, lit(sweep, rect.center().x));
        painter.rect_filled(rect, CornerRadius::same(corner), shade);
        return;
    };
    let step = rect.width() / count as f32;
    for index in 0..count {
        let left = rect.left() + step * index as f32;
        // Overlap by a hair, or the seams show as darker hairlines.
        let piece = Rect::from_min_max(
            pos2(left, rect.top()),
            pos2((left + step + 0.5).min(rect.right()), rect.bottom()),
        );
        let rounding = CornerRadius {
            nw: if index == 0 { corner } else { 0 },
            sw: if index == 0 { corner } else { 0 },
            ne: if index + 1 == count { corner } else { 0 },
            se: if index + 1 == count { corner } else { 0 },
        };
        let shade = motion::mix(base, bright, lit(sweep, piece.center().x));
        painter.rect_filled(piece, rounding, shade);
    }
}

/// A block the height of a line of text at `size`, `fraction` of the way
/// across what is available.
pub fn line(ui: &mut Ui, palette: &Palette, size: f32, fraction: f32) {
    let height = ui.fonts_mut(|fonts| fonts.row_height(&theme::regular(size))) * 0.72;
    let width = (ui.available_width() * fraction).max(24.0);
    let (rect, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
    block(ui, palette, rect, height / 2.0);
}

/// A card the exact size [`super::widgets::card`] takes, so a shelf does not
/// change height when the real ones arrive.
pub fn card(ui: &mut Ui, palette: &Palette, round: bool) {
    const PAD: f32 = 12.0;
    const TITLE_GAP: f32 = 10.0;
    const SUBTITLE_GAP: f32 = 2.0;
    const BOTTOM_PAD: f32 = 8.0;
    let image_size = CARD_WIDTH - 2.0 * PAD;
    let (title_row, subtitle_row) = ui.fonts_mut(|fonts| {
        (
            fonts.row_height(&theme::semibold(14.0)),
            fonts.row_height(&theme::regular(12.5)),
        )
    });
    let height =
        PAD + image_size + TITLE_GAP + title_row + SUBTITLE_GAP + 2.0 * subtitle_row + BOTTOM_PAD;
    let (rect, _) = ui.allocate_exact_size(vec2(CARD_WIDTH, height), Sense::hover());
    if !ui.is_rect_visible(rect) {
        return;
    }
    let cover = Rect::from_min_size(rect.min + vec2(PAD, PAD), Vec2::splat(image_size));
    block(
        ui,
        palette,
        cover,
        if round { image_size / 2.0 } else { 6.0 },
    );
    let left = rect.left() + PAD;
    let title = Rect::from_min_size(
        pos2(left, cover.bottom() + TITLE_GAP),
        vec2(image_size * 0.78, title_row * 0.72),
    );
    block(ui, palette, title, title.height() / 2.0);
    let subtitle = Rect::from_min_size(
        pos2(left, title.bottom() + SUBTITLE_GAP + 4.0),
        vec2(image_size * 0.52, subtitle_row * 0.72),
    );
    block(ui, palette, subtitle, subtitle.height() / 2.0);
}

/// A row of placeholder cards under a shelf's own heading.
pub fn shelf(ui: &mut Ui, palette: &Palette, title: &str, count: usize, round: bool) {
    ui.add_space(8.0);
    theme::section_title(ui, palette, title);
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = CARD_GAP / 2.0;
        for _ in 0..count {
            card(ui, palette, round);
        }
    });
}

/// Placeholder cards laid out the way [`super::widgets::grid`] lays out real
/// ones.
pub fn grid(ui: &mut Ui, palette: &Palette, count: usize, round: bool) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(CARD_GAP, CARD_GAP);
        for _ in 0..count {
            card(ui, palette, round);
        }
    });
}

/// Rows the height of the track rows they stand in for, with a cover, a title,
/// a second line, and a duration where the real ones have them.
pub fn track_rows(ui: &mut Ui, palette: &Palette, count: usize, compact: bool) {
    let height = if compact {
        theme::COMPACT_ROW_HEIGHT
    } else {
        theme::ROW_HEIGHT
    };
    let cover = if compact { 36.0 } else { 40.0 };
    for index in 0..count {
        let width = ui.available_width();
        let (rect, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
        if !ui.is_rect_visible(rect) {
            continue;
        }
        let art = Rect::from_center_size(
            pos2(rect.left() + 8.0 + cover / 2.0, rect.center().y),
            Vec2::splat(cover),
        );
        block(ui, palette, art, 4.0);
        let left = art.right() + 12.0;
        // The lines differ in length row to row, so a column of them does not
        // read as a table of identical bars.
        let stretch = [0.34, 0.27, 0.42, 0.30, 0.38][index % 5];
        let title = Rect::from_min_size(
            pos2(left, rect.center().y - 11.0),
            vec2(width * stretch, 10.0),
        );
        block(ui, palette, title, 5.0);
        let subtitle = Rect::from_min_size(
            pos2(left, rect.center().y + 3.0),
            vec2(width * stretch * 0.6, 9.0),
        );
        block(ui, palette, subtitle, 4.5);
        let duration = Rect::from_min_size(
            pos2(rect.right() - 46.0, rect.center().y - 4.5),
            vec2(32.0, 9.0),
        );
        block(ui, palette, duration, 4.5);
    }
}

/// The page header: the cover, the kind above the name, and the line of
/// details under it.
pub fn hero(ui: &mut Ui, palette: &Palette, round: bool) {
    ui.add_space(12.0);
    let cover_size = if ui.available_width() > 720.0 {
        212.0
    } else {
        160.0
    };
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 24.0;
        let (rect, _) = ui.allocate_exact_size(Vec2::splat(cover_size), Sense::hover());
        block(
            ui,
            palette,
            rect,
            if round { cover_size / 2.0 } else { 6.0 },
        );
        ui.vertical(|ui| {
            let width = ui.available_width();
            ui.set_width(width);
            ui.spacing_mut().item_spacing.y = 6.0;
            ui.add_space(cover_size * 0.08);
            line(ui, palette, 12.5, 0.10);
            ui.add_space(6.0);
            let title = Rect::from_min_size(
                pos2(ui.cursor().left(), ui.cursor().top()),
                vec2((width * 0.55).max(120.0), cover_size * 0.22),
            );
            ui.allocate_exact_size(title.size(), Sense::hover());
            block(ui, palette, title, 8.0);
            ui.add_space(10.0);
            line(ui, palette, 13.0, 0.32);
        });
    });
}

/// The row of controls under a page header: the play disc, the two round
/// buttons beside it, and the filter field off to the right.
pub fn actions(ui: &mut Ui, palette: &Palette) {
    ui.add_space(18.0);
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(width, 56.0), Sense::hover());
    if !ui.is_rect_visible(rect) {
        return;
    }
    let mut x = rect.left();
    for size in [56.0_f32, 26.0, 26.0] {
        let at = Rect::from_center_size(pos2(x + size / 2.0, rect.center().y), Vec2::splat(size));
        block(ui, palette, at, size / 2.0);
        x += size + 22.0;
    }
    let filter = Rect::from_center_size(
        pos2(rect.right() - 90.0, rect.center().y),
        vec2(176.0, 32.0),
    );
    block(ui, palette, filter, 16.0);
}

/// A whole page that has not arrived: its header, its controls, and its rows.
pub fn page(ui: &mut Ui, palette: &Palette, round: bool, rows: usize) {
    hero(ui, palette, round);
    actions(ui, palette);
    ui.add_space(16.0);
    track_rows(ui, palette, rows, false);
}

/// Stacked lines of varying length, for lyrics.
pub fn lines(ui: &mut Ui, palette: &Palette, count: usize) {
    ui.spacing_mut().item_spacing.y = 14.0;
    for index in 0..count {
        line(ui, palette, 16.0, [0.62, 0.48, 0.71, 0.55, 0.40][index % 5]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_band_is_brightest_at_its_middle_and_gone_at_its_edges() {
        let at = Some(500.0);
        assert!((lit(at, 500.0) - 1.0).abs() < 1e-5);
        assert_eq!(lit(at, 500.0 - HIGHLIGHT), 0.0);
        assert_eq!(lit(at, 500.0 + HIGHLIGHT), 0.0);
        assert_eq!(lit(at, 0.0), 0.0, "it reached clear across the window");
    }

    #[test]
    fn the_band_falls_off_evenly_on_both_sides() {
        let at = Some(300.0);
        for step in 0..=10 {
            let offset = HIGHLIGHT * step as f32 / 10.0;
            assert!(
                (lit(at, 300.0 - offset) - lit(at, 300.0 + offset)).abs() < 1e-5,
                "lopsided at {offset}"
            );
        }
    }

    #[test]
    fn the_band_only_ever_brightens() {
        let at = Some(300.0);
        for step in -20..=20 {
            let value = lit(at, 300.0 + HIGHLIGHT * step as f32 / 10.0);
            assert!((0.0..=1.0).contains(&value), "{value} is not a brightness");
        }
    }

    #[test]
    fn a_circle_is_drawn_whole_rather_than_in_pieces() {
        assert_eq!(
            strips(212.0, 106.0),
            None,
            "the artist cover came out square"
        );
        assert_eq!(strips(56.0, 28.0), None, "the play disc came out square");
        assert_eq!(strips(32.0, 16.0), None, "the filter pill came out square");
        assert_eq!(
            strips(26.0, 13.0),
            None,
            "a small round button landed on the boundary and came out square"
        );
    }

    #[test]
    fn a_long_bar_is_drawn_in_enough_pieces_for_the_band_to_cross_it() {
        let count = strips(600.0, 5.0).expect("a title bar lost its gradient");
        assert!(count >= 20, "600 points in only {count} pieces");
        assert!(
            strips(148.0, 6.0).is_some_and(|n| n > 1),
            "the cover lost its gradient"
        );
    }

    #[test]
    fn every_piece_is_wide_enough_to_carry_its_own_rounding() {
        for (width, radius) in [(600.0, 5.0), (148.0, 6.0), (40.0, 4.0), (176.0, 16.0)] {
            if let Some(count) = strips(width, radius) {
                assert!(
                    radius < width / count as f32,
                    "{width}x{radius} split into {count} pieces narrower than its rounding"
                );
            }
        }
    }

    #[test]
    fn a_still_page_is_evenly_lit() {
        for x in [0.0, 250.0, 1000.0] {
            assert_eq!(
                lit(None, x),
                0.0,
                "reduced motion still moved the band across"
            );
        }
    }
}
