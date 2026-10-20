//! Keyboard focus for controls the React app builds from native buttons and
//! inputs: a tab stop, Enter and Space as clicks (GPUI does this for any
//! focused element with a click handler), and the `focus-visible` ring.
//!
//! The ring is a 2px outline painted just outside the control, like CSS
//! `ring-2`. A GPUI box shadow cannot stand in for it: GPUI fills the whole
//! shadow rectangle, which shows through transparent controls.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use gpui::{
    canvas, point, px, quad, AbsoluteLength, App, BorderStyle, Bounds, Corners, CornersRefinement,
    Element, ElementId, FocusHandle, Global, Hsla, InteractiveElement, IntoElement, ParentElement,
    Stateful, StyleRefinement, Styled, Window,
};

use crate::ui::theme::{palette, Alpha};

const RING_WIDTH: f32 = 2.;

/// Whether the last input was the keyboard. Browsers draw `focus-visible`
/// styles only then, so a clicked button shows no ring.
#[derive(Default)]
struct KeyboardModality(bool);

impl Global for KeyboardModality {}

/// Focus handles for controls, by element id, so a control keeps its handle
/// across renders without threading a `Window` through every builder.
#[derive(Default)]
struct FocusHandles(RefCell<HashMap<ElementId, FocusHandle>>);

impl Global for FocusHandles {}

pub fn init(cx: &mut App) {
    cx.set_global(KeyboardModality::default());
    cx.set_global(FocusHandles::default());
}

pub fn keyboard_modality(cx: &App) -> bool {
    cx.try_global::<KeyboardModality>().is_some_and(|m| m.0)
}

/// Records the input kind; the window redraws when it flips so focus
/// styles appear or disappear.
pub fn set_keyboard_modality(keyboard: bool, window: &mut Window, cx: &mut App) {
    if keyboard_modality(cx) != keyboard {
        cx.set_global(KeyboardModality(keyboard));
        window.refresh();
    }
}

/// The focus handle for the control with `id`, made on first use.
pub fn focus_handle_for(id: ElementId, cx: &App) -> FocusHandle {
    cx.global::<FocusHandles>()
        .0
        .borrow_mut()
        .entry(id)
        .or_insert_with(|| cx.focus_handle().tab_stop(true))
        .clone()
}

/// Paints the `ring-2` outline around its parent while `handle` has focus.
/// Add it as a child of the focusable element; `radii` are the element's
/// own corner radii.
pub fn focus_ring(
    handle: FocusHandle,
    color: Hsla,
    radii: CornersRefinement<AbsoluteLength>,
) -> impl IntoElement {
    canvas(
        |_, _, _| {},
        move |bounds: Bounds<gpui::Pixels>, _, window, _| {
            if !handle.is_focused(window) {
                return;
            }
            let rem = window.rem_size();
            let ring = px(RING_WIDTH);
            let radius =
                |r: Option<AbsoluteLength>| r.map(|r| r.to_pixels(rem)).unwrap_or_default() + ring;
            let corners = Corners {
                top_left: radius(radii.top_left),
                top_right: radius(radii.top_right),
                bottom_right: radius(radii.bottom_right),
                bottom_left: radius(radii.bottom_left),
            };
            let outer = Bounds::from_corners(
                bounds.origin - point(ring, ring),
                bounds.bottom_right() + point(ring, ring),
            );
            window.paint_quad(quad(
                outer,
                corners,
                gpui::transparent_black(),
                ring,
                color,
                BorderStyle::Solid,
            ));
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}

type ExtraStyle = Rc<dyn Fn(StyleRefinement) -> StyleRefinement>;

/// A control's `focus-visible` style, resolved up front so closures that
/// cannot borrow `cx` can apply it.
#[derive(Clone)]
pub struct FocusVisible {
    /// `None` outside keyboard modality: the control gets no focus style.
    style: Option<(Option<Hsla>, Option<ExtraStyle>)>,
}

impl FocusVisible {
    /// `focus-visible:ring-2 ring-accent/20`, the common button ring.
    pub fn ring(cx: &App) -> Self {
        Self::ring_alpha(0.2, cx)
    }

    pub fn ring_alpha(alpha: f32, cx: &App) -> Self {
        Self {
            style: keyboard_modality(cx).then(|| (Some(palette(cx).accent.a(alpha)), None)),
        }
    }

    /// No ring, only `extra` (e.g. `focus-visible:underline`).
    pub fn styled(cx: &App, extra: impl Fn(StyleRefinement) -> StyleRefinement + 'static) -> Self {
        Self {
            style: keyboard_modality(cx).then(|| (None, Some(Rc::new(extra) as ExtraStyle))),
        }
    }

    /// Adds `extra` on top of the ring.
    pub fn with(mut self, extra: impl Fn(StyleRefinement) -> StyleRefinement + 'static) -> Self {
        if let Some((_, slot)) = self.style.as_mut() {
            *slot = Some(Rc::new(extra));
        }
        self
    }

    /// Makes `element` a tab stop with this focus style. The element needs
    /// an id, which keys its focus handle.
    pub fn apply<E>(&self, element: E, cx: &App) -> E
    where
        E: Element + ParentElement + InteractiveElement + Styled,
    {
        match Element::id(&element) {
            Some(id) => self.apply_with(element, focus_handle_for(id, cx)),
            None => element.tab_index(0),
        }
    }

    /// `apply` with a handle the caller owns and configures (`tab_stop`).
    pub fn apply_with<E>(&self, element: E, handle: FocusHandle) -> E
    where
        E: ParentElement + InteractiveElement + Styled,
    {
        let mut element = element.track_focus(&handle);
        let Some((ring, extra)) = self.style.clone() else {
            return element;
        };
        if let Some(extra) = extra {
            element = element.focus(move |s| extra(s));
        }
        match ring {
            Some(color) => {
                let radii = element.style().corner_radii.clone();
                element.child(focus_ring(handle, color, radii))
            }
            None => element,
        }
    }
}

pub trait KeyboardFocus: Element + ParentElement + InteractiveElement + Styled + Sized {
    /// A tab stop with the `focus-visible:ring-2 ring-accent/20` ring.
    fn keyboard_focus(self, cx: &App) -> Self {
        FocusVisible::ring(cx).apply(self, cx)
    }

    /// A tab stop whose ring uses the accent at `alpha`.
    fn keyboard_focus_ring(self, alpha: f32, cx: &App) -> Self {
        FocusVisible::ring_alpha(alpha, cx).apply(self, cx)
    }

    /// A tab stop for a control shown only on row hover
    /// (`opacity-0 group-hover:opacity-100 focus-visible:opacity-100`).
    fn keyboard_focus_reveal(self, cx: &App) -> Self {
        FocusVisible::ring(cx)
            .with(|s| s.opacity(1.))
            .apply(self, cx)
    }
}

impl<E> KeyboardFocus for Stateful<E> where
    Stateful<E>: Element + ParentElement + InteractiveElement + Styled
{
}
