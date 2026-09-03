//! The desktop showing through the window's edges, the way every Mac
//! application with a sidebar looks.
//!
//! One `NSVisualEffectView` sits underneath the whole surface egui draws on.
//! Which parts of the interface it shows through is decided in the palette,
//! not here: anything painted opaque hides it, and the sidebar, the player
//! bar, and the side panels are painted translucent so it does not.

use std::sync::atomic::{AtomicUsize, Ordering};

use objc2::MainThreadMarker;
use objc2_app_kit::{
    NSApplication, NSAutoresizingMaskOptions, NSVisualEffectBlendingMode, NSVisualEffectMaterial,
    NSVisualEffectState, NSVisualEffectView, NSWindow, NSWindowOrderingMode, NSWorkspace,
};

/// The window the layer was put under. A window rebuilt for the mini player
/// and back takes a new one; comparing the pointer notices that, where a plain
/// "done once" flag would leave the rebuilt window flat.
static INSTALLED_UNDER: AtomicUsize = AtomicUsize::new(0);

/// Whether the desktop is allowed to show through. Off while the person has
/// asked macOS to reduce transparency, which is an accessibility setting and
/// not a preference to second-guess.
pub fn wanted() -> bool {
    let Some(mtm) = MainThreadMarker::new() else {
        return false;
    };
    let _ = mtm;
    !NSWorkspace::sharedWorkspace().accessibilityDisplayShouldReduceTransparency()
}

/// Puts the blurred layer under the window, once. Safe to call every frame:
/// the window does not exist for the first of them.
pub fn install() {
    if !wanted() {
        return;
    }
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let Some(window) = main_window(mtm) else {
        return;
    };
    let under = objc2::rc::Retained::as_ptr(&window) as usize;
    if INSTALLED_UNDER.load(Ordering::Relaxed) == under {
        return;
    }
    let Some(content) = window.contentView() else {
        return;
    };
    // winit makes its own drawing view the window's content view, and a
    // subview is always drawn over its superview's own content: putting the
    // layer under `content` would put it over the interface. Its superview is
    // the window's frame, where the layer can sit genuinely underneath.
    // Safety: reading a view's superview on the main thread, which the
    // marker above proves this is.
    let Some(frame) = (unsafe { content.superview() }) else {
        return;
    };
    let effect = NSVisualEffectView::new(mtm);
    // Sidebar is the material Finder, Mail, and Music put behind theirs, and
    // it is the one the system tints to match the desktop underneath.
    effect.setMaterial(NSVisualEffectMaterial::Sidebar);
    // Behind the window, so it blurs the desktop rather than the app's own
    // pixels. Within-window blending would blur nothing: this view is the
    // bottom of the stack.
    effect.setBlendingMode(NSVisualEffectBlendingMode::BehindWindow);
    // Greys out with the window when the app loses focus, as the platform's
    // own sidebars do.
    effect.setState(NSVisualEffectState::FollowsWindowActiveState);
    effect.setFrame(frame.bounds());
    effect.setAutoresizingMask(
        NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable,
    );
    frame.addSubview_positioned_relativeTo(&effect, NSWindowOrderingMode::Below, Some(&content));
    window.setOpaque(false);
    INSTALLED_UNDER.store(under, Ordering::Relaxed);
}

/// Whether the desktop is showing through right now, which is what decides
/// whether the interface paints its chrome translucent.
pub fn active() -> bool {
    INSTALLED_UNDER.load(Ordering::Relaxed) != 0
}

fn main_window(mtm: MainThreadMarker) -> Option<objc2::rc::Retained<NSWindow>> {
    let app = NSApplication::sharedApplication(mtm);
    app.mainWindow()
        .or_else(|| app.windows().iter().find(|window| window.isVisible()))
}

#[cfg(test)]
mod tests {
    #[test]
    fn asking_the_desktop_about_transparency_answers() {
        // A selector that no longer exists would take the process down rather
        // than return, so reaching the assertion is the whole point.
        let _ = super::wanted();
    }
}
