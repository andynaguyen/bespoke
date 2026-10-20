//! A Lucide icon that, like an inline SVG with `stroke="currentColor"`,
//! takes the inherited text color at paint time, so it follows its parent's
//! hover and active colors. GPUI's `svg()` paints only with a color set on
//! the element itself.

use gpui::{
    point, px, App, Bounds, Element, ElementId, GlobalElementId, Hsla, InspectorElementId,
    IntoElement, LayoutId, Pixels, Radians, SharedString, Style, StyleRefinement, Styled,
    TransformationMatrix, Window,
};

pub struct Icon {
    path: SharedString,
    style: StyleRefinement,
    color: Option<Hsla>,
    rotation: Option<Radians>,
}

/// An icon from `assets/icons/<name>.svg`.
pub fn icon(name: &str) -> Icon {
    Icon {
        path: SharedString::from(format!("icons/{name}.svg")),
        style: StyleRefinement::default(),
        color: None,
        rotation: None,
    }
}

impl Icon {
    /// Paints in `color` instead of the inherited text color.
    pub fn text_color(mut self, color: impl Into<Hsla>) -> Self {
        self.color = Some(color.into());
        self
    }

    /// Rotates around the center, e.g. a chevron flipped open.
    pub fn rotate(mut self, angle: impl Into<Radians>) -> Self {
        self.rotation = Some(angle.into());
        self
    }
}

impl Styled for Icon {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl IntoElement for Icon {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for Icon {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let mut style = Style::default();
        gpui::Refineable::refine(&mut style, &self.style);
        (window.request_layout(style, None, cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        _: &mut Window,
        _: &mut App,
    ) {
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let color = self.color.unwrap_or_else(|| window.text_style().color);
        let opacity = self.style.opacity.unwrap_or(1.);
        let color = Hsla {
            a: color.a * opacity,
            ..color
        };
        let transformation = match self.rotation {
            Some(angle) => {
                let center = bounds.center().scale(window.scale_factor());
                let origin = (point(px(0.), px(0.)) - bounds.center()).scale(window.scale_factor());
                TransformationMatrix::unit()
                    .translate(center)
                    .rotate(angle)
                    .translate(origin)
            }
            None => TransformationMatrix::unit(),
        };
        if let Err(e) = window.paint_svg(bounds, self.path.clone(), transformation, color, cx) {
            log::warn!("Failed to paint icon {}: {e}", self.path);
        }
    }
}
