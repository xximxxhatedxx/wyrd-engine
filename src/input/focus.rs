//! FocusManager: modal stack for keyboard focus.

use super::InputEvent;
use crate::widgets::WidgetId;
use log::debug;

/// Focus scope pushed by modal popups.
struct FocusScope {
    focused_widget: Option<WidgetId>,
}

pub struct FocusManager {
    stack: Vec<FocusScope>,
    events: Vec<(WidgetId, InputEvent)>,
    escape_requested: bool,
}

impl Default for FocusManager {
    fn default() -> Self {
        Self::new()
    }
}

impl FocusManager {
    pub fn new() -> Self {
        Self {
            stack: vec![FocusScope {
                focused_widget: None,
            }],
            events: Vec::new(),
            escape_requested: false,
        }
    }

    pub fn push_scope(&mut self) {
        debug!("Pushing focus scope");
        self.stack.push(FocusScope {
            focused_widget: None,
        });
    }

    pub fn pop_scope(&mut self) -> bool {
        if self.stack.len() > 1 {
            self.stack.pop();
            true
        } else {
            false
        }
    }

    pub fn current_focus(&self) -> Option<WidgetId> {
        self.stack.last().and_then(|s| s.focused_widget)
    }

    pub fn set_focus(&mut self, widget: WidgetId) {
        if let Some(scope) = self.stack.last_mut() {
            scope.focused_widget = Some(widget);
        }
    }

    pub fn clear_focus(&mut self) {
        if let Some(scope) = self.stack.last_mut() {
            scope.focused_widget = None;
        }
    }

    pub fn dispatch(&mut self, keysym: u32, utf8: String, pressed: bool) {
        if pressed && keysym == 0xff1b && self.stack.len() > 1 {
            self.pop_scope();
            self.escape_requested = true;
            return;
        }
        let Some(widget) = self.current_focus() else {
            return;
        };
        let event = if pressed {
            InputEvent::KeyPress {
                keysym,
                utf8: (!utf8.is_empty()).then_some(utf8),
            }
        } else {
            InputEvent::KeyRelease { keysym }
        };
        self.events.push((widget, event));
    }

    pub fn drain_events(&mut self) -> impl Iterator<Item = (WidgetId, InputEvent)> + '_ {
        self.events.drain(..)
    }

    pub fn take_escape_requested(&mut self) -> bool {
        std::mem::take(&mut self.escape_requested)
    }
}
