//! FocusManager: modal stack for keyboard focus.

use super::{InputEvent, WidgetId};
use log::debug;
use wyrd_graphics::FocusableScene;

struct FocusScope {
    focused_widget: Option<WidgetId>,
    focused_name: Option<String>,
    focused_index: Option<usize>,
}

/// Manages a stack of modal keyboard focus scopes across surfaces and focusable targets.
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
                focused_name: None,
                focused_index: None,
            }],
            events: Vec::new(),
            escape_requested: false,
        }
    }

    pub fn push_scope(&mut self) {
        debug!("Pushing focus scope");
        self.stack.push(FocusScope {
            focused_widget: None,
            focused_name: None,
            focused_index: Some(0),
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

    pub fn set_focus_with_meta(
        &mut self,
        widget: WidgetId,
        name: Option<String>,
        index: Option<usize>,
    ) {
        if let Some(scope) = self.stack.last_mut() {
            scope.focused_widget = Some(widget);
            scope.focused_name = name;
            scope.focused_index = index;
        }
    }

    pub fn resolve_focus<S: FocusableScene>(
        &mut self,
        scene: &S,
        navigable: &[WidgetId],
    ) -> Option<WidgetId> {
        if navigable.is_empty() {
            return None;
        }
        let scope = self.stack.last_mut()?;
        if let Some(cur) = scope.focused_widget {
            if let Some(pos) = navigable.iter().position(|&w| w == cur) {
                scope.focused_index = Some(pos);
                if scope.focused_name.is_none() {
                    scope.focused_name = scene.node_name(cur).map(str::to_owned);
                }
                return Some(cur);
            }
        }
        if let Some(ref name) = scope.focused_name {
            if let Some((pos, &wid)) = navigable
                .iter()
                .enumerate()
                .find(|(_, &w)| scene.node_name(w) == Some(name.as_str()))
            {
                scope.focused_widget = Some(wid);
                scope.focused_index = Some(pos);
                return Some(wid);
            }
        }
        let idx = scope
            .focused_index
            .unwrap_or(0)
            .min(navigable.len().saturating_sub(1));
        let chosen = navigable[idx];
        scope.focused_widget = Some(chosen);
        scope.focused_index = Some(idx);
        scope.focused_name = scene.node_name(chosen).map(str::to_owned);
        Some(chosen)
    }

    pub fn clear_focus(&mut self) {
        if let Some(scope) = self.stack.last_mut() {
            scope.focused_widget = None;
            scope.focused_name = None;
            scope.focused_index = None;
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
