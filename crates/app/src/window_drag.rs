//! Moving and resizing the frameless windows through the compositor.
//!
//! Once winit hands the drag to the compositor, the button's release goes
//! to the compositor, not to the window: Slint would keep the pressed
//! TouchArea grabbing the pointer and no other click would get through. A
//! release (outside, so nothing is clicked) is given to Slint right after.
//! The double click on a title bar is told here for the same reason.

use std::cell::Cell;
use std::time::{Duration, Instant};

use slint::platform::{PointerEventButton, WindowEvent};
use slint::winit_030::{WinitWindowAccessor, winit};
use slint::{ComponentHandle, LogicalPosition};

const DOUBLE_CLICK: Duration = Duration::from_millis(400);

thread_local! {
    static LAST_PRESS: Cell<Option<Instant>> = const { Cell::new(None) };
}

/// Lets go of what Slint thinks is still pressed (on the next turn of the
/// event loop, once the press has been handled).
fn release_later<C: ComponentHandle + 'static>(component: &C) {
    let weak = component.as_weak();
    slint::Timer::single_shot(Duration::ZERO, move || {
        if let Some(c) = weak.upgrade() {
            let outside = LogicalPosition::new(-1.0, -1.0);
            c.window()
                .dispatch_event(WindowEvent::PointerReleased { position: outside, button: PointerEventButton::Left });
            c.window().dispatch_event(WindowEvent::PointerExited);
        }
    });
}

/// A press on a title bar: the window moves with the pointer. True when it
/// is the second press of a double click (the caller maximizes instead).
pub fn start_move<C: ComponentHandle + 'static>(component: &C) -> bool {
    let now = Instant::now();
    let double = LAST_PRESS.with(|l| l.replace(Some(now))).is_some_and(|t| now.duration_since(t) < DOUBLE_CLICK);
    if double {
        LAST_PRESS.with(|l| l.set(None));
        release_later(component);
        return true;
    }
    component.window().with_winit_window(|w| {
        let _ = w.drag_window();
    });
    release_later(component);
    false
}

/// The minimize button: asked of the compositor every time. Slint keeps a
/// "minimized" flag of its own that Wayland never clears (a window restored
/// from the taskbar says nothing), so a second `set_minimized(true)` was a
/// no-op and the button stopped working.
pub fn minimize<C: ComponentHandle + 'static>(component: &C) {
    component.window().with_winit_window(|w| w.set_minimized(true));
}

/// A press on an edge or a corner ("n", "se"…): the window resizes.
pub fn start_resize<C: ComponentHandle + 'static>(component: &C, edge: &str) {
    use winit::window::ResizeDirection as D;
    let direction = match edge {
        "n" => D::North,
        "s" => D::South,
        "e" => D::East,
        "w" => D::West,
        "ne" => D::NorthEast,
        "nw" => D::NorthWest,
        "se" => D::SouthEast,
        _ => D::SouthWest,
    };
    component.window().with_winit_window(|w| {
        let _ = w.drag_resize_window(direction);
    });
    release_later(component);
}

/// Keeps `set` told whether the window is maximized, whoever maximized it
/// (the desktop too: a drag off the top, a shortcut), so the corners are
/// square only then.
pub fn follow_maximized<C: ComponentHandle + 'static>(component: &C, set: impl Fn(&C, bool) + 'static) {
    follow_window(component, set, |_, _, _| {});
}

/// follow_maximized, and `pressed` told where each press of a mouse button
/// landed (logical window coordinates), once Slint has handled it.
pub fn follow_window<C: ComponentHandle + 'static>(
    component: &C,
    set: impl Fn(&C, bool) + 'static,
    pressed: impl Fn(&C, f32, f32) + 'static,
) {
    let weak = component.as_weak();
    let set = std::rc::Rc::new(set);
    let pressed = std::rc::Rc::new(pressed);
    let cursor = std::rc::Rc::new(Cell::new((0.0f32, 0.0f32)));
    component.window().on_winit_window_event(move |window, event| {
        match event {
            winit::event::WindowEvent::Resized(_) => {
                // Asked once the event is through (the window is borrowed meanwhile).
                let (weak, set) = (weak.clone(), set.clone());
                slint::Timer::single_shot(Duration::ZERO, move || {
                    if let Some(c) = weak.upgrade() {
                        if let Some(on) = c.window().with_winit_window(|w| w.is_maximized()) {
                            set(&c, on);
                        }
                    }
                });
            }
            winit::event::WindowEvent::CursorMoved { position, .. } => {
                let scale = window.scale_factor();
                cursor.set((position.x as f32 / scale, position.y as f32 / scale));
            }
            winit::event::WindowEvent::MouseInput { state: winit::event::ElementState::Pressed, .. } => {
                let (weak, pressed, (x, y)) = (weak.clone(), pressed.clone(), cursor.get());
                slint::Timer::single_shot(Duration::ZERO, move || {
                    if let Some(c) = weak.upgrade() {
                        pressed(&c, x, y);
                    }
                });
            }
            _ => {}
        }
        slint::winit_030::EventResult::Propagate
    });
}
