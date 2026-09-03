use egui::{Color32, Context, Id, Pos2, Vec2};
use std::f32::consts::TAU;

use super::motion;

/// How long a burst lives. Short on purpose: this is a reward for an action,
/// not something to sit and watch, and someone working down a list will set
/// off a dozen of them.
const LIFE: f32 = 0.45;
const PARTICLES: usize = 8;
/// How far out a particle ends up.
const REACH: f32 = 21.0;
/// Where a particle starts: the heart's rim, not its middle. Leaving from the
/// centre puts the whole burst on top of the glyph for the first half of it,
/// which reads as clutter rather than as something thrown.
const RIM: f32 = 9.0;
/// The most that can be in the air at once. Bursts are keyed by the control
/// that fired them and dropped when they finish, so this is a ceiling on a
/// number that is normally one, not a budget anything runs up against.
const AT_ONCE: usize = 4;

#[derive(Clone, Copy, Debug)]
struct Burst {
    owner: Id,
    center: Pos2,
    color: Color32,
    started: f64,
}

fn slot() -> Id {
    Id::new("fastpotify-spark")
}

fn live(ctx: &Context) -> Vec<Burst> {
    ctx.data(|data| data.get_temp::<Vec<Burst>>(slot()))
        .unwrap_or_default()
}

/// Sets one off at `center`. Silent where the desktop asked for less movement,
/// which leaves the control's own state change to say what happened.
pub fn burst(ctx: &Context, owner: Id, center: Pos2, color: Color32) {
    if motion::reduced(ctx) {
        return;
    }
    let started = ctx.input(|input| input.time);
    let mut bursts = live(ctx);
    bursts.retain(|burst| burst.owner != owner && age(started, burst) < 1.0);
    if bursts.len() >= AT_ONCE {
        bursts.remove(0);
    }
    bursts.push(Burst {
        owner,
        center,
        color,
        started,
    });
    ctx.data_mut(|data| data.insert_temp(slot(), bursts));
    ctx.request_repaint();
}

/// How far along the burst `owner` set off is, or nothing if it has none. The
/// control reads this to pop and fill in step with its own particles.
pub fn phase(ctx: &Context, owner: Id) -> Option<f32> {
    let now = ctx.input(|input| input.time);
    live(ctx)
        .iter()
        .find(|burst| burst.owner == owner)
        .map(|burst| age(now, burst))
        .filter(|t| *t < 1.0)
}

fn age(now: f64, burst: &Burst) -> f32 {
    (((now - burst.started) as f32) / LIFE).max(0.0)
}

/// Draws every burst in flight, above everything else.
///
/// Particles leave the control that fired them, and a list row lives inside a
/// scroll area that would cut them off at its edge, so they are painted in
/// screen coordinates on the foreground layer rather than by the control.
pub fn draw(ctx: &Context) {
    let bursts = live(ctx);
    if bursts.is_empty() {
        return;
    }
    let now = ctx.input(|input| input.time);
    let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, slot()));
    let mut alive = Vec::with_capacity(bursts.len());
    for burst in bursts {
        let t = age(now, &burst);
        if t >= 1.0 {
            continue;
        }
        let (radius, width, ring_alpha) = ring(t);
        if ring_alpha > 0.004 {
            painter.circle_stroke(
                burst.center,
                radius,
                egui::Stroke::new(width, burst.color.gamma_multiply(ring_alpha)),
            );
        }
        let alpha = particle_alpha(t);
        if alpha > 0.004 {
            let color = burst.color.gamma_multiply(alpha);
            for index in 0..PARTICLES {
                painter.circle_filled(
                    burst.center + particle_offset(index, t),
                    particle_radius(index) * (1.0 - 0.35 * t),
                    color,
                );
            }
        }
        alive.push(burst);
    }
    if alive.is_empty() {
        ctx.data_mut(|data| data.remove::<Vec<Burst>>(slot()));
    } else {
        ctx.data_mut(|data| data.insert_temp(slot(), alive));
        ctx.request_repaint();
    }
}

/// Where particle `index` sits at `t`, relative to the burst's centre.
///
/// Every particle leaves on the same ease, decelerating the way something
/// thrown does. The reach varies a little between them so the ring of them
/// does not read as a drawn circle.
pub fn particle_offset(index: usize, t: f32) -> Vec2 {
    let angle = TAU * (index as f32 + 0.5) / PARTICLES as f32;
    let spread = 0.82 + 0.18 * ((index * 3) % 5) as f32 / 4.0;
    // Some particles run slightly ahead of the others, so the eight of them
    // do not travel as one rigid ring.
    let lead = 0.85 + 0.15 * ((index * 5) % 3) as f32 / 2.0;
    let eased = motion::Curve::EaseOut.apply((t * lead).clamp(0.0, 1.0));
    Vec2::angled(angle) * (RIM + (REACH * spread - RIM) * eased)
}

/// Alternating sizes, so the burst has some texture up close.
pub fn particle_radius(index: usize) -> f32 {
    if index.is_multiple_of(2) { 2.0 } else { 1.4 }
}

/// Full while the particles are moving fastest, then gone quickly.
pub fn particle_alpha(t: f32) -> f32 {
    (1.0 - t * t).clamp(0.0, 1.0)
}

/// The ring's radius, stroke width, and alpha: it opens out past the
/// particles and thins to nothing.
pub fn ring(t: f32) -> (f32, f32, f32) {
    let t = t.clamp(0.0, 1.0);
    let eased = motion::Curve::EaseOut.apply(t);
    (
        9.0 + 15.0 * eased,
        2.0 * (1.0 - t),
        (1.0 - t).powf(1.5) * 0.85,
    )
}

/// What to multiply the heart's size by. It swells and comes back inside the
/// burst's first half, so the control has settled before the particles have.
pub fn pop(t: f32) -> f32 {
    let p = (t / 0.45).clamp(0.0, 1.0);
    1.0 + 0.30 * (p * (1.0 - p) * 4.0)
}

/// How much of the filled heart shows. It fills well before the pop peaks:
/// the answer should look like it arrived with the click.
pub fn fill(t: f32) -> f32 {
    motion::Curve::EaseOut.apply((t / 0.18).clamp(0.0, 1.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn particles_leave_the_centre_and_keep_going_out() {
        for index in 0..PARTICLES {
            let start = particle_offset(index, 0.0).length();
            assert!(
                (start - RIM).abs() < 1e-4,
                "particle {index} started at {start}, not on the heart's rim"
            );
            let mut previous = 0.0;
            for step in 0..=20 {
                let out = particle_offset(index, step as f32 / 20.0).length();
                assert!(
                    out >= previous - 1e-4,
                    "particle {index} came back at {step}"
                );
                previous = out;
            }
            assert!(
                previous > RIM * 1.5 && previous <= REACH,
                "particle {index} ended at {previous}, outside its reach"
            );
        }
    }

    #[test]
    fn particles_spread_all_the_way_round() {
        let mut quadrants = [false; 4];
        for index in 0..PARTICLES {
            let offset = particle_offset(index, 1.0);
            let quadrant = usize::from(offset.x < 0.0) + 2 * usize::from(offset.y < 0.0);
            quadrants[quadrant] = true;
        }
        assert_eq!(quadrants, [true; 4], "the burst is lopsided");
    }

    #[test]
    fn no_two_particles_land_on_top_of_each_other() {
        let ends: Vec<Vec2> = (0..PARTICLES).map(|i| particle_offset(i, 1.0)).collect();
        for (index, one) in ends.iter().enumerate() {
            for other in &ends[index + 1..] {
                assert!(
                    (*one - *other).length() > 3.0,
                    "two particles overlap at {one:?}"
                );
            }
        }
    }

    #[test]
    fn the_particles_do_not_move_as_one_ring() {
        let midway: Vec<f32> = (0..PARTICLES)
            .map(|i| particle_offset(i, 0.4).length())
            .collect();
        let spread = midway.iter().fold(0.0_f32, |a, b| a.max(*b))
            - midway.iter().fold(f32::MAX, |a, b| a.min(*b));
        assert!(
            spread > 1.0,
            "every particle is at the same distance halfway through: {midway:?}"
        );
    }

    #[test]
    fn the_particles_clear_the_glyph_before_the_pop_peaks() {
        // The heart is drawn at sixteen points, so its rim is about eight out
        // and the pop pushes that to ten. Nothing should still be sitting on
        // it by the time it is at its largest.
        for index in 0..PARTICLES {
            let out = particle_offset(index, 0.225).length();
            assert!(
                out > 11.0,
                "particle {index} is still on the glyph at {out}"
            );
        }
    }

    #[test]
    fn the_reach_varies_between_particles() {
        let first = particle_offset(0, 1.0).length();
        assert!(
            (1..PARTICLES).any(|i| (particle_offset(i, 1.0).length() - first).abs() > 0.5),
            "every particle travels exactly as far, which reads as a drawn circle"
        );
    }

    #[test]
    fn everything_fades_out_by_the_end() {
        assert_eq!(particle_alpha(1.0), 0.0);
        assert!(particle_alpha(0.0) > 0.99);
        let (_, width, alpha) = ring(1.0);
        assert!(width.abs() < 1e-4 && alpha.abs() < 1e-4);
        assert!(ring(0.0).2 > 0.5);
    }

    #[test]
    fn the_ring_opens_outwards_past_the_particles() {
        let mut previous = 0.0;
        for step in 0..=20 {
            let (radius, _, _) = ring(step as f32 / 20.0);
            assert!(radius >= previous, "the ring closed at {step}");
            previous = radius;
        }
        assert!(previous > REACH, "the ring never overtakes the particles");
    }

    #[test]
    fn the_heart_swells_and_comes_back_to_its_own_size() {
        assert_eq!(pop(0.0), 1.0);
        assert!((pop(1.0) - 1.0).abs() < 1e-5);
        let peak = (0..=100)
            .map(|step| pop(step as f32 / 100.0))
            .fold(0.0_f32, f32::max);
        assert!(
            peak > 1.25 && peak < 1.35,
            "the pop reached {peak}, not the third it was drawn for"
        );
        assert!(pop(0.5) < pop(0.2), "it is still growing halfway through");
    }

    #[test]
    fn the_heart_fills_before_it_finishes_swelling() {
        assert_eq!(fill(0.0), 0.0);
        assert_eq!(fill(0.18), 1.0);
        assert_eq!(fill(1.0), 1.0);
    }
}

#[cfg(test)]
mod lifecycle {
    use super::*;

    fn frame(ctx: &Context, at: f64, fire: Option<(Id, Pos2)>) -> usize {
        let input = egui::RawInput {
            time: Some(at),
            screen_rect: Some(egui::Rect::from_min_size(
                Pos2::ZERO,
                Vec2::new(400.0, 300.0),
            )),
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| {
            if let Some((owner, center)) = fire {
                burst(ui.ctx(), owner, center, Color32::GREEN);
            }
            draw(ui.ctx());
        });
        output.textures_delta.clear();
        fn count(shape: &egui::epaint::Shape) -> usize {
            match shape {
                egui::epaint::Shape::Circle(_) => 1,
                egui::epaint::Shape::Vec(shapes) => shapes.iter().map(count).sum(),
                _ => 0,
            }
        }
        output.shapes.iter().map(|c| count(&c.shape)).sum()
    }

    fn animating(ctx: &Context) -> bool {
        let mut answer = true;
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            answer = !motion::reduced(ui.ctx());
        });
        output.textures_delta.clear();
        answer
    }

    #[test]
    fn a_burst_is_drawn_then_leaves_nothing_behind() {
        let ctx = Context::default();
        if !animating(&ctx) {
            return;
        }
        let heart = Id::new("a-heart");
        let at = Pos2::new(100.0, 100.0);

        let drawn = frame(&ctx, 0.0, Some((heart, at)));
        assert!(
            drawn > PARTICLES,
            "the burst drew {drawn} shapes, fewer than its ring and particles"
        );
        assert!(
            phase(&ctx, heart).is_some(),
            "the heart cannot see its burst"
        );
        assert!(
            phase(&ctx, Id::new("another")).is_none(),
            "it leaked sideways"
        );

        assert!(
            frame(&ctx, f64::from(LIFE) * 0.5, None) > 0,
            "it went out early"
        );

        assert_eq!(
            frame(&ctx, f64::from(LIFE) + 0.05, None),
            0,
            "the burst outlived its own life"
        );
        assert!(phase(&ctx, heart).is_none());
        assert!(
            ctx.data(|data| data.get_temp::<Vec<Burst>>(slot()))
                .is_none(),
            "a finished burst stayed in memory"
        );
    }

    #[test]
    fn liking_a_run_of_songs_does_not_pile_bursts_up() {
        let ctx = Context::default();
        if !animating(&ctx) {
            return;
        }
        for step in 0..40 {
            frame(
                &ctx,
                f64::from(step) * 0.02,
                Some((Id::new(step), Pos2::new(100.0, 20.0 + step as f32))),
            );
        }
        let held = ctx
            .data(|data| data.get_temp::<Vec<Burst>>(slot()))
            .unwrap_or_default();
        assert!(
            held.len() <= AT_ONCE,
            "forty likes left {} bursts in the air",
            held.len()
        );
    }

    #[test]
    fn liking_the_same_heart_again_restarts_its_burst_rather_than_adding_one() {
        let ctx = Context::default();
        if !animating(&ctx) {
            return;
        }
        let heart = Id::new("one-heart");
        let at = Pos2::new(50.0, 50.0);
        frame(&ctx, 0.0, Some((heart, at)));
        frame(&ctx, 0.1, Some((heart, at)));
        let held = ctx
            .data(|data| data.get_temp::<Vec<Burst>>(slot()))
            .unwrap_or_default();
        assert_eq!(held.len(), 1, "one heart owns two bursts");
        let t = phase(&ctx, heart).expect("the restarted burst is gone");
        assert!(
            t < 0.05,
            "it carried on from the first burst instead of restarting: {t}"
        );
    }
}
