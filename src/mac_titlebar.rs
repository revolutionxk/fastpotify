use objc2::MainThreadMarker;
use objc2_app_kit::{NSApplication, NSEventType};
use objc2_foundation::{NSUserDefaults, ns_string};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DoubleClickAction {
    Zoom,
    Minimize,
    Nothing,
}

pub fn double_click() -> bool {
    let Some(mtm) = MainThreadMarker::new() else {
        return false;
    };
    let Some(event) = NSApplication::sharedApplication(mtm).currentEvent() else {
        return false;
    };
    event.r#type() == NSEventType::LeftMouseDown && event.clickCount() == 2
}

pub fn double_click_action() -> DoubleClickAction {
    let setting = NSUserDefaults::standardUserDefaults()
        .stringForKey(ns_string!("AppleActionOnDoubleClick"))
        .map(|setting| setting.to_string());
    action_for(setting.as_deref())
}

fn action_for(setting: Option<&str>) -> DoubleClickAction {
    match setting {
        Some("Minimize") => DoubleClickAction::Minimize,
        Some("None") => DoubleClickAction::Nothing,
        _ => DoubleClickAction::Zoom,
    }
}

#[cfg(test)]
mod tests {
    use super::{DoubleClickAction, action_for};

    #[test]
    fn an_unset_preference_zooms() {
        assert_eq!(action_for(None), DoubleClickAction::Zoom);
        assert_eq!(action_for(Some("Maximize")), DoubleClickAction::Zoom);
        assert_eq!(action_for(Some("Fill")), DoubleClickAction::Zoom);
    }

    #[test]
    fn the_other_two_choices_are_honoured() {
        assert_eq!(action_for(Some("Minimize")), DoubleClickAction::Minimize);
        assert_eq!(action_for(Some("None")), DoubleClickAction::Nothing);
    }

    #[test]
    fn asking_the_system_for_the_preference_answers() {
        let _ = super::double_click_action();
    }
}
