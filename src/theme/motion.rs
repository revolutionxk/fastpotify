use egui::{Color32, Context, Id, Pos2, Rect, Ui, Vec2};
use std::f32::consts::TAU;

const MAX_STEP: f32 = 0.1;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Spring {
    pub response: f32,
    pub damping: f32,
}

impl Spring {
    pub const fn new(response: f32, damping: f32) -> Self {
        Self { response, damping }
    }

    fn omega(&self) -> f32 {
        TAU / self.response.max(0.001)
    }

    fn zeta(&self) -> f32 {
        self.damping.clamp(0.05, 1.0)
    }

    pub fn advance(&self, state: &mut State, target: f32, dt: f32) -> bool {
        let dt = dt.clamp(0.0, MAX_STEP);
        let omega = self.omega();
        let zeta = self.zeta();
        let x0 = state.value - target;
        let v0 = state.velocity;
        let decay = (-zeta * omega * dt).exp();
        let (x, v) = if zeta >= 0.999 {
            let b = v0 + omega * x0;
            let x = (x0 + b * dt) * decay;
            let v = (b - omega * (x0 + b * dt)) * decay;
            (x, v)
        } else {
            let wd = omega * (1.0 - zeta * zeta).sqrt();
            let c1 = x0;
            let c2 = (v0 + zeta * omega * x0) / wd;
            let (sin, cos) = (wd * dt).sin_cos();
            let x = decay * (c1 * cos + c2 * sin);
            let v =
                decay * ((wd * c2 - zeta * omega * c1) * cos - (zeta * omega * c2 + wd * c1) * sin);
            (x, v)
        };
        let tolerance = 1e-3 * (1.0 + target.abs());
        if x.abs() < tolerance && v.abs() < tolerance * 20.0 {
            state.value = target;
            state.velocity = 0.0;
            true
        } else {
            state.value = target + x;
            state.velocity = v;
            false
        }
    }
}

pub const SMOOTH: Spring = Spring::new(0.30, 1.0);
pub const SNAPPY: Spring = Spring::new(0.22, 0.82);
pub const GENTLE: Spring = Spring::new(0.45, 1.0);
pub const HOVER: Spring = Spring::new(0.16, 1.0);

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct State {
    pub value: f32,
    pub velocity: f32,
}

impl State {
    pub const fn at(value: f32) -> Self {
        Self {
            value,
            velocity: 0.0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Curve {
    Linear,
    EaseIn,
    EaseOut,
    EaseInOut,
    Standard,
}

impl Curve {
    fn control_points(self) -> (f32, f32, f32, f32) {
        match self {
            Self::Linear => (0.0, 0.0, 1.0, 1.0),
            Self::EaseIn => (0.42, 0.0, 1.0, 1.0),
            Self::EaseOut => (0.0, 0.0, 0.58, 1.0),
            Self::EaseInOut => (0.42, 0.0, 0.58, 1.0),
            Self::Standard => (0.25, 0.1, 0.25, 1.0),
        }
    }

    pub fn apply(self, t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        if self == Self::Linear || t <= 0.0 || t >= 1.0 {
            return t;
        }
        let (x1, y1, x2, y2) = self.control_points();
        bezier(x1, y1, x2, y2, t)
    }
}

fn bezier_axis(a: f32, b: f32, t: f32) -> f32 {
    let c = 3.0 * a;
    let bb = 3.0 * (b - a) - c;
    let aa = 1.0 - c - bb;
    ((aa * t + bb) * t + c) * t
}

fn bezier_slope(a: f32, b: f32, t: f32) -> f32 {
    let c = 3.0 * a;
    let bb = 3.0 * (b - a) - c;
    let aa = 1.0 - c - bb;
    (3.0 * aa * t + 2.0 * bb) * t + c
}

fn bezier(x1: f32, y1: f32, x2: f32, y2: f32, x: f32) -> f32 {
    let mut t = x;
    for _ in 0..8 {
        let error = bezier_axis(x1, x2, t) - x;
        if error.abs() < 1e-5 {
            return bezier_axis(y1, y2, t);
        }
        let slope = bezier_slope(x1, x2, t);
        if slope.abs() < 1e-6 {
            break;
        }
        t -= error / slope;
    }
    let (mut low, mut high) = (0.0_f32, 1.0_f32);
    t = x;
    for _ in 0..16 {
        let value = bezier_axis(x1, x2, t);
        if (value - x).abs() < 1e-5 {
            break;
        }
        if value < x {
            low = t;
        } else {
            high = t;
        }
        t = (low + high) / 2.0;
    }
    bezier_axis(y1, y2, t)
}

pub fn reduced(ctx: &Context) -> bool {
    #[cfg(target_os = "macos")]
    {
        const REFRESH: f64 = 2.0;
        let id = Id::new("fastpotify-reduce-motion");
        let now = ctx.input(|input| input.time);
        if let Some((at, value)) = ctx.data(|data| data.get_temp::<(f64, bool)>(id))
            && now - at < REFRESH
        {
            return value;
        }
        let value =
            objc2_app_kit::NSWorkspace::sharedWorkspace().accessibilityDisplayShouldReduceMotion();
        ctx.data_mut(|data| data.insert_temp(id, (now, value)));
        value
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = ctx;
        false
    }
}

fn step(ui: &Ui, id: Id, target: f32, spring: Spring, start: f32) -> f32 {
    let ctx = ui.ctx();
    if reduced(ctx) {
        ctx.data_mut(|data| data.insert_temp(id, State::at(target)));
        return target;
    }
    let dt = ctx.input(|input| input.stable_dt);
    let mut state: State = ctx
        .data(|data| data.get_temp(id))
        .unwrap_or(State::at(start));
    if !spring.advance(&mut state, target, dt) {
        ctx.request_repaint();
    }
    ctx.data_mut(|data| data.insert_temp(id, state));
    state.value
}

pub fn value(ui: &Ui, id: Id, target: f32, spring: Spring) -> f32 {
    step(ui, id, target, spring, target)
}

pub fn toggle(ui: &Ui, id: Id, on: bool, spring: Spring) -> f32 {
    step(ui, id, if on { 1.0 } else { 0.0 }, spring, 0.0)
}

pub fn fade(ui: &Ui, id: Id, on: bool, curve: Curve, seconds: f32) -> f32 {
    let ctx = ui.ctx();
    if reduced(ctx) {
        return if on { 1.0 } else { 0.0 };
    }
    curve.apply(ctx.animate_bool_with_time(id, on, seconds))
}

pub fn vec2(ui: &Ui, id: Id, target: Vec2, spring: Spring) -> Vec2 {
    Vec2::new(
        value(ui, id.with("x"), target.x, spring),
        value(ui, id.with("y"), target.y, spring),
    )
}

pub fn pos2(ui: &Ui, id: Id, target: Pos2, spring: Spring) -> Pos2 {
    Pos2::new(
        value(ui, id.with("x"), target.x, spring),
        value(ui, id.with("y"), target.y, spring),
    )
}

pub fn rect(ui: &Ui, id: Id, target: Rect, spring: Spring) -> Rect {
    Rect::from_min_max(
        pos2(ui, id.with("min"), target.min, spring),
        pos2(ui, id.with("max"), target.max, spring),
    )
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Trail {
    pass: u64,
    marked: Option<Id>,
    current: Option<Id>,
    current_state: State,
    leaving: Option<Id>,
    leaving_state: State,
}

impl Trail {
    pub fn amount(&self, id: Id) -> f32 {
        if self.current == Some(id) {
            self.current_state.value
        } else if self.leaving == Some(id) {
            self.leaving_state.value
        } else {
            0.0
        }
    }
}

/// Names the row the pointer is over. A list draws its rows in order and only
/// one of them is under the pointer, so the row itself reports it and the trail
/// picks it up on the next pass.
pub fn mark(ctx: &Context, scope: Id, id: Id) {
    let wake = ctx.data_mut(|data| {
        let mut trail: Trail = data.get_temp(scope).unwrap_or_default();
        let wake = trail.marked != Some(id);
        trail.marked = Some(id);
        data.insert_temp(scope, trail);
        wake
    });
    if wake {
        ctx.request_repaint();
    }
}

/// Two slots, one fading in and one fading out, for a list with more rows than
/// can each hold their own state. Advances once per pass however often it is
/// asked.
pub fn trail(ctx: &Context, scope: Id, spring: Spring) -> Trail {
    let mut trail: Trail = ctx.data(|data| data.get_temp(scope)).unwrap_or_default();
    let pass = ctx.cumulative_pass_nr();
    if trail.pass == pass {
        return trail;
    }
    trail.pass = pass;
    let active = trail.marked.take();
    if trail.current != active {
        if trail.leaving == active {
            std::mem::swap(&mut trail.current, &mut trail.leaving);
            std::mem::swap(&mut trail.current_state, &mut trail.leaving_state);
        } else {
            trail.leaving = trail.current;
            trail.leaving_state = trail.current_state;
            trail.current = active;
            trail.current_state = State::at(0.0);
        }
    }
    if reduced(ctx) {
        trail.current_state = State::at(if active.is_some() { 1.0 } else { 0.0 });
        trail.leaving = None;
        trail.leaving_state = State::default();
        ctx.data_mut(|data| data.insert_temp(scope, trail));
        return trail;
    }
    let dt = ctx.input(|input| input.stable_dt);
    let mut busy = false;
    if trail.current.is_some() {
        busy |= !spring.advance(&mut trail.current_state, 1.0, dt);
    }
    if trail.leaving.is_some() {
        if spring.advance(&mut trail.leaving_state, 0.0, dt) {
            trail.leaving = None;
        } else {
            busy = true;
        }
    }
    if busy {
        ctx.request_repaint();
    }
    ctx.data_mut(|data| data.insert_temp(scope, trail));
    trail
}

pub fn mix(from: Color32, to: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let channel = |a: u8, b: u8| (f32::from(a) + (f32::from(b) - f32::from(a)) * t).round() as u8;
    Color32::from_rgba_premultiplied(
        channel(from.r(), to.r()),
        channel(from.g(), to.g()),
        channel(from.b(), to.b()),
        channel(from.a(), to.a()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settle(spring: Spring, target: f32, dt: f32) -> (State, usize, f32) {
        let mut state = State::at(0.0);
        let mut peak = 0.0_f32;
        for step in 0..4000 {
            if spring.advance(&mut state, target, dt) {
                return (state, step, peak);
            }
            peak = peak.max(state.value);
        }
        panic!("spring never settled");
    }

    #[test]
    fn a_spring_reaches_its_target_and_stops() {
        let (state, steps, _) = settle(SMOOTH, 1.0, 1.0 / 60.0);
        assert_eq!(state.value, 1.0);
        assert_eq!(state.velocity, 0.0);
        assert!(steps > 1, "it jumped there in one frame");
        assert!(steps < 120, "SMOOTH took {steps} frames, over two seconds");
    }

    #[test]
    fn a_critically_damped_spring_never_overshoots() {
        for (spring, dt) in [(SMOOTH, 1.0 / 60.0), (GENTLE, 1.0 / 120.0), (HOVER, 0.05)] {
            let (_, _, peak) = settle(spring, 1.0, dt);
            assert!(peak <= 1.0 + 1e-4, "{spring:?} overshot to {peak}");
        }
    }

    #[test]
    fn the_bouncier_preset_overshoots_a_little_and_comes_back() {
        let (state, _, peak) = settle(SNAPPY, 1.0, 1.0 / 120.0);
        assert!(peak > 1.0, "SNAPPY did not overshoot at all");
        assert!(
            peak < 1.15,
            "SNAPPY overshot to {peak}, too far for a toggle"
        );
        assert_eq!(state.value, 1.0);
    }

    #[test]
    fn the_frame_rate_does_not_change_where_a_spring_gets_to() {
        for dt in [1.0 / 144.0, 1.0 / 60.0, 1.0 / 30.0, 0.25] {
            let (state, _, _) = settle(SMOOTH, 240.0, dt);
            assert_eq!(state.value, 240.0, "at dt {dt}");
        }
    }

    #[test]
    fn a_long_stall_does_not_blow_a_spring_up() {
        let mut state = State::at(0.0);
        for _ in 0..50 {
            SMOOTH.advance(&mut state, 1.0, 5.0);
            assert!(state.value.is_finite() && state.value.abs() <= 2.0);
        }
    }

    #[test]
    fn a_reversal_coasts_before_it_turns_around() {
        let mut state = State::at(0.0);
        for _ in 0..6 {
            SMOOTH.advance(&mut state, 1.0, 1.0 / 60.0);
        }
        let (was, moving) = (state.value, state.velocity);
        assert!(moving > 0.0 && was > 0.0 && was < 1.0);
        SMOOTH.advance(&mut state, 0.0, 1.0 / 60.0);
        assert!(
            state.value > was,
            "the reversal snapped back instead of coasting: {was} then {}",
            state.value
        );
        for _ in 0..400 {
            if SMOOTH.advance(&mut state, 0.0, 1.0 / 60.0) {
                break;
            }
        }
        assert_eq!(state.value, 0.0, "it never came back");
    }

    #[test]
    fn the_curves_span_the_unit_square_and_rise() {
        for curve in [
            Curve::Linear,
            Curve::EaseIn,
            Curve::EaseOut,
            Curve::EaseInOut,
            Curve::Standard,
        ] {
            assert_eq!(curve.apply(0.0), 0.0, "{curve:?}");
            assert!((curve.apply(1.0) - 1.0).abs() < 1e-4, "{curve:?}");
            let mut previous = 0.0;
            for step in 0..=100 {
                let value = curve.apply(step as f32 / 100.0);
                assert!(value >= previous - 1e-4, "{curve:?} dipped at {step}");
                previous = value;
            }
        }
    }

    #[test]
    fn ease_out_leads_and_ease_in_trails() {
        assert!(Curve::EaseOut.apply(0.25) > 0.25);
        assert!(Curve::EaseIn.apply(0.25) < 0.25);
    }

    #[test]
    fn mixing_ends_on_the_colours_it_was_given() {
        let a = Color32::from_rgb(0, 0, 0);
        let b = Color32::from_rgb(255, 128, 64);
        assert_eq!(mix(a, b, 0.0), a);
        assert_eq!(mix(a, b, 1.0), b);
        assert_eq!(mix(a, b, -1.0), a);
        assert_eq!(mix(a, b, 2.0), b);
        assert_eq!(mix(a, b, 0.5).g(), 64);
    }
}

#[cfg(test)]
mod ui_tests {
    use super::*;

    fn pass(ctx: &Context, mut body: impl FnMut(&Ui)) {
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0))),
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| body(ui));
        output.textures_delta.clear();
    }

    fn animating(ctx: &Context) -> bool {
        let mut answer = true;
        pass(ctx, |ui| answer = !reduced(ui.ctx()));
        answer
    }

    #[test]
    fn a_value_is_already_there_the_first_time_it_is_asked_for() {
        let ctx = Context::default();
        let id = Id::new("seed");
        let mut first = 0.0;
        pass(&ctx, |ui| first = value(ui, id, 1.0, GENTLE));
        assert_eq!(
            first, 1.0,
            "a panel that starts open would have slid in at launch"
        );
        if !animating(&ctx) {
            return;
        }
        let mut next = 1.0;
        pass(&ctx, |ui| next = value(ui, id, 0.0, GENTLE));
        assert!(next > 0.0 && next < 1.0, "it cut straight there: {next}");
    }

    #[test]
    fn a_toggle_starts_from_off_and_travels() {
        let ctx = Context::default();
        if !animating(&ctx) {
            return;
        }
        let id = Id::new("toggle");
        let mut first = 1.0;
        pass(&ctx, |ui| first = toggle(ui, id, true, SMOOTH));
        assert!(first < 0.5, "the first frame was already lit: {first}");
        let mut later = first;
        for _ in 0..60 {
            pass(&ctx, |ui| later = toggle(ui, id, true, SMOOTH));
        }
        assert_eq!(later, 1.0, "it never arrived");
    }

    #[test]
    fn a_trail_lights_what_is_marked_and_lets_the_last_one_fade() {
        let ctx = Context::default();
        if !animating(&ctx) {
            return;
        }
        let scope = Id::new("list");
        let (first_row, second_row) = (Id::new("row-a"), Id::new("row-b"));
        let mut lit = (0.0, 0.0);
        for _ in 0..60 {
            pass(&ctx, |ui| {
                mark(ui.ctx(), scope, first_row);
                let trail = trail(ui.ctx(), scope, HOVER);
                lit = (trail.amount(first_row), trail.amount(second_row));
            });
        }
        assert_eq!(lit, (1.0, 0.0), "the marked row never lit");
        pass(&ctx, |ui| {
            mark(ui.ctx(), scope, second_row);
            let trail = trail(ui.ctx(), scope, HOVER);
            lit = (trail.amount(first_row), trail.amount(second_row));
        });
        pass(&ctx, |ui| {
            mark(ui.ctx(), scope, second_row);
            let trail = trail(ui.ctx(), scope, HOVER);
            lit = (trail.amount(first_row), trail.amount(second_row));
        });
        assert!(
            lit.0 > 0.0 && lit.0 < 1.0,
            "the row left behind blinked out: {lit:?}"
        );
        assert!(lit.1 > 0.0, "the row moved to never lit: {lit:?}");
        for _ in 0..60 {
            pass(&ctx, |ui| {
                mark(ui.ctx(), scope, second_row);
                let trail = trail(ui.ctx(), scope, HOVER);
                lit = (trail.amount(first_row), trail.amount(second_row));
            });
        }
        assert_eq!(lit, (0.0, 1.0), "the trail never settled on one row");
    }
}
