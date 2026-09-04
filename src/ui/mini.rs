//! The small player: album art, what is playing, and the controls, in a
//! window that floats over whatever else is on screen.
//!
//! It is a third kind of window beside the main one and the Winamp one, and
//! like them it owns the whole surface: the window has no frame of its own, so
//! everything here including the rounding and the dragging is painted.

use egui::{Align, CornerRadius, Layout, Rect, Sense, UiBuilder, Vec2, pos2, vec2};

use crate::api::models::PlayableItem;
use crate::app::App;
use crate::model::{Action, Loadable};
use crate::theme::{self, Icon, motion};

use super::widgets::{self, SliderEvent};

const PAD: f32 = 10.0;
pub const WIDTH: f32 = 250.0;
/// The strip along the top carrying the close button, the grip, and the way
/// into the settings.
const TOPBAR: f32 = 28.0;
/// The art is square and as wide as the window allows.
const ART: f32 = WIDTH - 2.0 * PAD;
const TITLE: f32 = 17.0;
const SUBTITLE: f32 = 15.0;
const PROGRESS: f32 = 16.0;
const GAP: f32 = 8.0;
/// One row of what is coming next.
const QUEUE_ROW: f32 = 34.0;
const QUEUE_ROWS: usize = 3;
/// The window's own rounding, which it has to draw because it has no frame.
pub const CORNER: f32 = 12.0;

/// How tall the window has to be for what the settings ask it to show.
pub fn window_size(settings: &crate::settings::Settings) -> Vec2 {
    let queue = if settings.mini_queue {
        GAP + QUEUE_ROW * QUEUE_ROWS as f32
    } else {
        0.0
    };
    vec2(
        WIDTH,
        TOPBAR + ART + GAP + TITLE + SUBTITLE + PROGRESS + queue + PAD,
    )
}

/// Holds the window to the size the player is drawn at.
///
/// eframe remembers window geometry between runs, and what it remembers is the
/// big window's, so opening this one restores that size over the one it was
/// asked for. The Winamp window has the same fight; both settle it by asking
/// again from inside. It is also how the window grows when the queue is
/// switched on.
fn fit_window(ctx: &egui::Context, settings: &crate::settings::Settings) {
    let wanted = window_size(settings);
    // Not `inner_rect`: Wayland reports no window positions, so that is `None`
    // there; the viewport rect is the window's size on every desktop.
    if (ctx.viewport_rect().size() - wanted).abs().max_elem() < 1.0 {
        return;
    }
    // Retry rejected resize requests at most once per second.
    let asked = egui::Id::new("mini-fit-asked");
    let now = ctx.input(|input| input.time);
    if ctx
        .data(|data| data.get_temp::<f64>(asked))
        .is_some_and(|last| now - last < 1.0)
    {
        return;
    }
    ctx.data_mut(|data| data.insert_temp(asked, now));
    ctx.send_viewport_cmd(egui::ViewportCommand::MinInnerSize(wanted));
    ctx.send_viewport_cmd(egui::ViewportCommand::MaxInnerSize(wanted));
    ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(wanted));
}

fn settings_open(ctx: &egui::Context) -> bool {
    ctx.data(|data| data.get_temp(egui::Id::new("mini-settings")))
        .unwrap_or(false)
}

fn set_settings_open(ctx: &egui::Context, open: bool) {
    ctx.data_mut(|data| data.insert_temp(egui::Id::new("mini-settings"), open));
}

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    let ctx = &ctx;
    fit_window(ctx, &app.settings);
    if let Some(rect) = ctx.input(|input| input.viewport().outer_rect) {
        app.mini_last_pos = Some([rect.min.x, rect.min.y]);
    }
    super::keys::handle(app, ctx);

    let palette = app.palette;
    let rect = ui.max_rect();
    let now = app.now_playing();
    let art_url = now
        .as_ref()
        .and_then(|now| now.art_url.clone().or_else(|| now.art_small.clone()));
    // The ground is painted here: the window is see-through so its corners can
    // be round, which means nothing has drawn anything underneath.
    let ground = if app.settings.mini_tinted {
        app.tint_for(art_url.as_deref())
            .map_or(palette.panel, |tint| {
                super::blend(palette.panel, tint, 0.55)
            })
    } else {
        palette.panel
    };
    ui.painter()
        .rect_filled(rect, CornerRadius::same(CORNER as u8), ground);
    ui.painter().rect_stroke(
        rect,
        CornerRadius::same(CORNER as u8),
        egui::Stroke::new(1.0, palette.outline),
        egui::StrokeKind::Inside,
    );

    // Anywhere that is not a control moves the window, and the wheel changes
    // the volume wherever it is: in a window this small, reaching for a slider
    // to nudge the volume is most of the work.
    super::titlebar_drag(ui, rect);
    let notches = widgets::wheel_notches_over(
        ui,
        ui.rect_contains_pointer(rect),
        egui::Id::new("mini-wheel"),
    );
    if notches != 0 {
        app.actions
            .push(Action::VolumeBy((notches * 5).clamp(-100, 100) as i8));
    }
    top_bar(app, ui, rect);

    let art = Rect::from_min_size(
        pos2(rect.left() + PAD, rect.top() + TOPBAR),
        Vec2::splat(ART),
    );
    widgets::paint_cover_crossfade(
        ui,
        &palette,
        art_url.as_deref(),
        art,
        8.0,
        Icon::Music,
        egui::Id::new("mini-art"),
    );

    if settings_open(ctx) {
        panel(app, ui, art);
        return;
    }

    let lit = motion::toggle(
        ui,
        egui::Id::new("mini-lit"),
        ui.rect_contains_pointer(rect),
        motion::SMOOTH,
    );
    if lit > 0.004 {
        transport(app, ui, art, lit);
    }
    details(app, ui, rect, art, now.as_ref());
    progress(app, ui, rect, art, now.as_ref());
    if app.settings.mini_queue {
        coming_up(app, ui, rect);
    }
}

/// Close on the left, a grip in the middle, the settings on the right, the way
/// the player this one is modelled on arranges them.
fn top_bar(app: &mut App, ui: &mut egui::Ui, rect: Rect) {
    let palette = app.palette;
    let cy = rect.top() + TOPBAR / 2.0;
    let close = Rect::from_center_size(pos2(rect.left() + PAD + 6.0, cy), Vec2::splat(20.0));
    let response = ui
        .interact(close, egui::Id::new("mini-close"), Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    let lit = motion::toggle(
        ui,
        egui::Id::new("mini-close-lit"),
        response.hovered(),
        motion::HOVER,
    );
    ui.painter().circle_filled(
        close.center(),
        6.0,
        motion::mix(palette.danger.gamma_multiply(0.8), palette.danger, lit),
    );
    if response.clicked() {
        app.actions.push(Action::ToggleMiniPlayer);
    }
    response.on_hover_text("Back to the full window");

    // The grip says the window moves, which nothing else up here would.
    for step in 0..6 {
        let column = (step % 3) as f32;
        let row = (step / 3) as f32;
        ui.painter().circle_filled(
            pos2(rect.center().x - 5.0 + column * 5.0, cy - 2.5 + row * 5.0),
            1.3,
            palette.dim,
        );
    }

    let gear = Rect::from_center_size(pos2(rect.right() - PAD - 8.0, cy), Vec2::splat(24.0));
    let mut child = ui.new_child(
        UiBuilder::new()
            .max_rect(gear)
            .layout(Layout::centered_and_justified(egui::Direction::LeftToRight)),
    );
    let open = settings_open(ui.ctx());
    if theme::icon_button(
        &mut child,
        Icon::Settings,
        15.0,
        if open {
            palette.accent
        } else {
            palette.secondary
        },
        palette.text,
        "Mini player settings",
    )
    .clicked()
    {
        set_settings_open(ui.ctx(), !open);
    }
}

/// The settings, over the art, as a sheet with a way out.
fn panel(app: &mut App, ui: &mut egui::Ui, art: Rect) {
    let palette = app.palette;
    ui.painter().rect_filled(
        art,
        CornerRadius::same(8),
        palette.panel.gamma_multiply(0.97),
    );
    let inner = art.shrink(16.0);
    let mut child = ui.new_child(
        UiBuilder::new()
            .max_rect(inner)
            .layout(Layout::top_down(Align::Min)),
    );
    child.spacing_mut().item_spacing.y = 14.0;
    theme::text(
        &mut child,
        "Mini player settings",
        theme::bold(15.0),
        palette.text,
    );
    let mut picked = None;
    for (index, (label, on)) in [
        ("Background colour", app.settings.mini_tinted),
        ("Queue", app.settings.mini_queue),
        ("Stay on top", app.settings.mini_on_top),
    ]
    .into_iter()
    .enumerate()
    {
        let mut flag = on;
        child.horizontal(|ui| {
            ui.set_width(inner.width());
            theme::text(ui, label, theme::regular(13.0), palette.text);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if widgets::switch(ui, &palette, &mut flag).changed() {
                    picked = Some(index);
                }
            });
        });
    }
    match picked {
        Some(0) => {
            app.settings.mini_tinted = !app.settings.mini_tinted;
            app.actions.push(Action::SettingsChanged);
        }
        Some(1) => {
            app.settings.mini_queue = !app.settings.mini_queue;
            app.actions.push(Action::SettingsChanged);
        }
        Some(2) => app.actions.push(Action::ToggleMiniOnTop),
        _ => {}
    }
    child.add_space(6.0);
    child.with_layout(Layout::top_down(Align::Center), |ui| {
        if theme::pill_button(ui, &palette, "Done", true).clicked() {
            set_settings_open(ui.ctx(), false);
        }
    });
}

/// Previous, play, and next, over the art and only while the pointer is here.
fn transport(app: &mut App, ui: &mut egui::Ui, art: Rect, lit: f32) {
    let palette = app.palette;
    ui.painter().rect_filled(
        art,
        CornerRadius::same(8),
        egui::Color32::from_black_alpha((150.0 * lit) as u8),
    );
    let playing = app.believed_playing();
    let widths = [34.0_f32, 52.0, 34.0];
    let gap = 14.0;
    let total: f32 = widths.iter().sum::<f32>() + gap * 2.0;
    let mut x = art.center().x - total / 2.0;
    // Rising into place as they appear, the way the card's disc does.
    let cy = art.center().y + 8.0 * (1.0 - lit);
    let mut slot = |width: f32| {
        let at = Rect::from_center_size(pos2(x + width / 2.0, cy), Vec2::splat(width));
        x += width + gap;
        at
    };
    let cell = |ui: &mut egui::Ui, at: Rect| {
        let mut child = ui.new_child(
            UiBuilder::new()
                .max_rect(at)
                .layout(Layout::centered_and_justified(egui::Direction::LeftToRight)),
        );
        child.multiply_opacity(lit);
        child
    };

    let at = slot(widths[0]);
    let mut child = cell(ui, at);
    if theme::icon_button(
        &mut child,
        Icon::SkipBackFilled,
        20.0,
        egui::Color32::WHITE,
        egui::Color32::WHITE,
        "",
    )
    .clicked()
    {
        app.actions.push(Action::Previous);
    }

    let at = slot(widths[1]);
    let mut child = cell(ui, at);
    let icon = if playing {
        Icon::PauseFilled
    } else {
        Icon::PlayFilled
    };
    if theme::circle_button(
        &mut child,
        icon,
        44.0,
        egui::Color32::WHITE,
        egui::Color32::WHITE,
        palette.window,
        "",
    )
    .clicked()
    {
        app.actions.push(Action::TogglePlay);
    }

    let at = slot(widths[2]);
    let mut child = cell(ui, at);
    if theme::icon_button(
        &mut child,
        Icon::SkipForwardFilled,
        20.0,
        egui::Color32::WHITE,
        egui::Color32::WHITE,
        "",
    )
    .clicked()
    {
        app.actions.push(Action::Next);
    }

    volume(app, ui, art, lit);
}

/// The volume, under the transport and on the same hover. The wheel does this
/// too and does it faster; this is here so someone finds out it can be done.
fn volume(app: &mut App, ui: &mut egui::Ui, art: Rect, lit: f32) {
    let now = app.now_playing();
    let level = now
        .as_ref()
        .map(|now| now.volume_percent)
        .unwrap_or_else(|| crate::app::volume_to_percent(app.local.volume));
    let shown = match app.volume_preview {
        Some(fraction) => (fraction * 100.0).round() as u8,
        None => level,
    };
    let local = now.is_none_or(|now| now.local);
    let cy = art.bottom() - 24.0 + 8.0 * (1.0 - lit);
    let icon_rect = Rect::from_center_size(pos2(art.left() + 24.0, cy), Vec2::splat(26.0));
    let mut child = ui.new_child(
        UiBuilder::new()
            .max_rect(icon_rect)
            .layout(Layout::centered_and_justified(egui::Direction::LeftToRight)),
    );
    child.multiply_opacity(lit);
    let icon = match shown {
        0 => Icon::VolumeX,
        1..=33 => Icon::Volume,
        34..=66 => Icon::Volume1,
        _ => Icon::Volume2,
    };
    if theme::icon_button(
        &mut child,
        icon,
        16.0,
        egui::Color32::WHITE,
        egui::Color32::WHITE,
        "",
    )
    .clicked()
    {
        app.actions.push(Action::ToggleMute);
    }

    let width = art.right() - 16.0 - (icon_rect.right() + 6.0);
    let bar = Rect::from_min_size(
        pos2(icon_rect.right() + 6.0, cy - 8.0),
        vec2(width.max(20.0), 16.0),
    );
    let mut child = ui.new_child(
        UiBuilder::new()
            .max_rect(bar)
            .layout(Layout::left_to_right(Align::Center)),
    );
    child.multiply_opacity(lit);
    // No wheel step: the whole window already answers the wheel, and two
    // handlers would move the volume twice for one turn.
    match widgets::thin_slider(
        &mut child,
        &app.palette,
        egui::Id::new("mini-volume"),
        f32::from(shown) / 100.0,
        bar.width(),
        app.palette.accent,
        None,
    ) {
        SliderEvent::Dragging(value) => {
            app.volume_preview = Some(value);
            if local {
                app.actions
                    .push(Action::PreviewVolume((value * 100.0).round() as u8));
            }
        }
        SliderEvent::Committed(value) => {
            app.volume_preview = None;
            app.actions
                .push(Action::SetVolume((value * 100.0).round() as u8));
        }
        SliderEvent::None => {}
    }
}

fn details(
    app: &mut App,
    ui: &mut egui::Ui,
    rect: Rect,
    art: Rect,
    now: Option<&crate::app::NowPlaying>,
) {
    let palette = app.palette;
    let top = art.bottom() + GAP;
    let saved = now.is_some_and(|now| app.is_saved(&now.uri).unwrap_or(false));
    let heart = Rect::from_center_size(
        pos2(rect.right() - PAD - 12.0, top + (TITLE + SUBTITLE) / 2.0),
        Vec2::splat(28.0),
    );
    let likeable = now.is_some_and(|now| !now.is_episode);
    let text_right = if likeable {
        heart.left() - 4.0
    } else {
        rect.right() - PAD
    };
    let (title, subtitle) = match now {
        Some(now) => (now.title.clone(), now.subtitle.clone()),
        None => ("Nothing playing".to_owned(), String::new()),
    };
    let width = (text_right - rect.left() - PAD).max(20.0);
    let galley = widgets::ellipsized(ui, &title, theme::semibold(13.5), palette.text, width, 1);
    ui.painter()
        .galley(pos2(rect.left() + PAD, top), galley, palette.text);
    let galley = widgets::ellipsized(
        ui,
        &subtitle,
        theme::regular(12.0),
        palette.secondary,
        width,
        1,
    );
    ui.painter().galley(
        pos2(rect.left() + PAD, top + TITLE),
        galley,
        palette.secondary,
    );

    if let Some(now) = now
        && likeable
    {
        let uri = now.uri.clone();
        let mut child = ui.new_child(
            UiBuilder::new()
                .max_rect(heart)
                .layout(Layout::centered_and_justified(egui::Direction::LeftToRight)),
        );
        if theme::heart_button(&mut child, &palette, saved, 15.0, "").clicked() {
            app.actions.push(Action::ToggleSaved(uri));
        }
    }
}

fn progress(
    app: &mut App,
    ui: &mut egui::Ui,
    rect: Rect,
    art: Rect,
    now: Option<&crate::app::NowPlaying>,
) {
    let (position, duration) = now
        .map(|now| (now.position_ms, now.duration_ms))
        .unwrap_or((0, 0));
    let fraction = if duration > 0 {
        position as f32 / duration as f32
    } else {
        0.0
    };
    let width = rect.width() - 2.0 * PAD;
    let bar = Rect::from_min_size(
        pos2(
            rect.left() + PAD,
            art.bottom() + GAP + TITLE + SUBTITLE + 2.0,
        ),
        vec2(width, 16.0),
    );
    let mut child = ui.new_child(
        UiBuilder::new()
            .max_rect(bar)
            .layout(Layout::left_to_right(Align::Center)),
    );
    match widgets::thin_slider(
        &mut child,
        &app.palette,
        egui::Id::new("mini-seek"),
        fraction,
        width,
        app.palette.accent,
        None,
    ) {
        SliderEvent::Dragging(value) => app.seek_preview = Some(value),
        SliderEvent::Committed(value) => {
            app.seek_preview = None;
            if duration > 0 {
                app.actions
                    .push(Action::Seek((value * duration as f32) as u32));
            }
        }
        SliderEvent::None => {}
    }
}

/// The next few songs, when the settings ask for them.
fn coming_up(app: &mut App, ui: &mut egui::Ui, rect: Rect) {
    let palette = app.palette;
    let items: Vec<PlayableItem> = match &app.queue {
        Loadable::Loaded(queue) => queue.queue.iter().take(QUEUE_ROWS).cloned().collect(),
        _ => Vec::new(),
    };
    let top = rect.bottom() - PAD - QUEUE_ROW * QUEUE_ROWS as f32;
    if items.is_empty() {
        ui.painter().text(
            pos2(rect.center().x, top + QUEUE_ROW / 2.0),
            egui::Align2::CENTER_CENTER,
            "Nothing queued",
            theme::regular(11.5),
            palette.dim,
        );
        return;
    }
    for (index, item) in items.iter().enumerate() {
        let row = Rect::from_min_size(
            pos2(rect.left() + PAD, top + QUEUE_ROW * index as f32),
            vec2(rect.width() - 2.0 * PAD, QUEUE_ROW),
        );
        let cover =
            Rect::from_center_size(pos2(row.left() + 13.0, row.center().y), Vec2::splat(26.0));
        widgets::paint_cover(ui, &palette, item.image(64), cover, 4.0, Icon::Music);
        let left = cover.right() + 8.0;
        let width = (row.right() - left).max(20.0);
        let galley =
            widgets::ellipsized(ui, item.name(), theme::medium(11.5), palette.text, width, 1);
        ui.painter()
            .galley(pos2(left, row.center().y - 12.0), galley, palette.text);
        let galley = widgets::ellipsized(
            ui,
            &item.subtitle(),
            theme::regular(10.5),
            palette.dim,
            width,
            1,
        );
        ui.painter()
            .galley(pos2(left, row.center().y + 1.0), galley, palette.dim);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::Settings;

    #[test]
    fn the_window_is_tall_enough_for_everything_it_draws() {
        let size = window_size(&Settings::default());
        assert_eq!(size.x, WIDTH);
        assert_eq!(
            size.y,
            TOPBAR + ART + GAP + TITLE + SUBTITLE + PROGRESS + PAD
        );
        assert!(
            size.y > size.x,
            "the art is square, so the window has to be taller than it is wide"
        );
    }

    #[test]
    fn the_art_is_most_of_the_window() {
        let size = window_size(&Settings::default());
        assert_eq!(ART, size.x - 2.0 * PAD);
        assert!(
            ART > size.x * 0.85,
            "the art stopped being most of the window, which is the whole point"
        );
    }

    #[test]
    fn asking_for_the_queue_makes_room_for_it() {
        let plain = window_size(&Settings::default());
        let with_queue = window_size(&Settings {
            mini_queue: true,
            ..Settings::default()
        });
        assert_eq!(with_queue.x, plain.x, "the queue widened the window");
        let grew = with_queue.y - plain.y;
        assert!(
            grew >= QUEUE_ROW * QUEUE_ROWS as f32,
            "three rows were promised and {grew} points arrived"
        );
    }
}
