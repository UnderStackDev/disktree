//! Window decorations for desktops that leave them to the app.
//!
//! A tiling compositor such as Hyprland draws no frame and moves windows with
//! its own keys, so the app was built without one. GNOME on Wayland does not
//! implement server-side decorations at all: there a window without its own
//! title bar has no buttons, cannot be moved and cannot be resized. This
//! module draws what such a desktop expects — a title bar to drag, the
//! window buttons, a shadow and resize edges — and only when the platform
//! says decorations are the client's job, so nothing changes where the
//! compositor already does it.

use std::borrow::Cow;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AssetSource, BoxShadow, Context, CursorStyle, Decorations, Div,
    Edges, FontWeight, Hsla, InteractiveElement as _, IntoElement, MouseButton,
    MouseDownEvent, MouseMoveEvent, ParentElement, Pixels, Point, ResizeEdge,
    SharedString, Size, StatefulInteractiveElement as _, Styled, Tiling,
    Window, div, point, px, svg,
};
use gpui_omarchy::Theme;

use crate::state::Disktree;
use crate::ui::{icon, size, space, text};

/// The window buttons' glyphs, drawn to GNOME's symbolic shapes on its 16px
/// grid. The bundled set has Windows' thin-line glyphs, which look foreign
/// next to every GTK window on the same desktop.
const GLYPHS: [(&str, &[u8]); 4] = [
    (MINIMIZE, include_bytes!("../assets/window/minimize.svg")),
    (MAXIMIZE, include_bytes!("../assets/window/maximize.svg")),
    (RESTORE, include_bytes!("../assets/window/restore.svg")),
    (CLOSE, include_bytes!("../assets/window/close.svg")),
];
const MINIMIZE: &str = "disktree/window/minimize.svg";
const MAXIMIZE: &str = "disktree/window/maximize.svg";
const RESTORE: &str = "disktree/window/restore.svg";
const CLOSE: &str = "disktree/window/close.svg";

/// The default asset bundle plus the window buttons' glyphs.
pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> anyhow::Result<Option<Cow<'static, [u8]>>> {
        if let Some((_, bytes)) = GLYPHS.iter().find(|(name, _)| *name == path)
        {
            return Ok(Some(Cow::Borrowed(bytes)));
        }
        gpui_kit::assets::Assets.load(path)
    }

    fn list(&self, path: &str) -> anyhow::Result<Vec<SharedString>> {
        let mut found = gpui_kit::assets::Assets.list(path)?;
        found.extend(
            GLYPHS
                .iter()
                .map(|(name, _)| *name)
                .filter(|name| name.starts_with(path))
                .map(SharedString::from),
        );
        Ok(found)
    }
}

/// Room around the frame for its shadow. Also where the resize edges are
/// caught, so it is reserved even on a side that draws no shadow.
const SHADOW: Pixels = px(12.);
/// How far on each side of the visible edge a press starts a resize.
const RESIZE_HIT: Pixels = px(5.);
/// Pointer travel before a press on the title bar becomes a window move, so
/// a click with a shaky hand stays a click.
const DRAG_SLOP: f32 = 4.;

/// Whether this window draws its own frame.
pub fn client_side(window: &Window) -> bool {
    matches!(window.window_decorations(), Decorations::Client { .. })
}

/// Wrap the whole window: shadow, outline and resize edges when the frame
/// is ours, the content untouched when it is not.
pub fn frame(
    content: impl IntoElement,
    theme: &Theme,
    window: &mut Window,
) -> AnyElement {
    let Decorations::Client { tiling } = window.window_decorations() else {
        return content.into_any_element();
    };
    // The platform inset stays the full shadow even while tiled: clearing it
    // makes the first resize after a restore count the shadow twice.
    window.set_client_inset(SHADOW);
    let insets = frame_insets(tiling);
    let size = window.window_bounds().get_bounds().size;
    let active = window.is_window_active();
    div()
        .id("window-frame")
        .size_full()
        .flex()
        .flex_col()
        .pt(insets.top)
        .pb(insets.bottom)
        .pl(insets.left)
        .pr(insets.right)
        .on_mouse_down(MouseButton::Left, move |_, window, _| {
            let Decorations::Client { tiling } = window.window_decorations()
            else {
                return;
            };
            let size = window.window_bounds().get_bounds().size;
            if let Some(edge) = resize_edge(
                window.mouse_position(),
                size,
                frame_insets(tiling),
                tiling,
            ) {
                window.start_window_resize(edge);
            }
        })
        .child(
            div()
                .flex_1()
                .min_h_0()
                .min_w_0()
                .overflow_hidden()
                .cursor(CursorStyle::default())
                .border_color(theme.divider())
                .when(!tiling.top, Styled::border_t_1)
                .when(!tiling.bottom, Styled::border_b_1)
                .when(!tiling.left, Styled::border_l_1)
                .when(!tiling.right, Styled::border_r_1)
                .when(!tiling.is_tiled(), |d| d.shadow(shadow(active)))
                .child(content),
        )
        .child(
            div()
                .absolute()
                .top(px(-insets.top.as_f32()))
                .left(px(-insets.left.as_f32()))
                .size_full()
                .children(resize_zones(size, insets, tiling)),
        )
        .into_any_element()
}

/// The bar across the top: the title, a surface to drag and double-click,
/// and the window buttons. Nothing at all when the compositor draws them.
pub fn title_bar(
    title: &str,
    theme: &Theme,
    window: &Window,
    cx: &Context<'_, Disktree>,
) -> Option<AnyElement> {
    if !client_side(window) {
        return None;
    }
    let bar = div()
        .id("title-bar")
        .relative()
        .flex()
        .flex_row()
        .flex_shrink_0()
        .items_center()
        .justify_end()
        .h(size::TITLE_BAR)
        .px(space::SM)
        .bg(theme.surface)
        .border_b_1()
        .border_color(theme.divider())
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, event: &MouseDownEvent, window, _| {
                // The second press of a double click maximizes; arming a
                // move then would grab the window out of the zoom.
                if event.click_count >= 2 {
                    this.title_drag = None;
                    window.zoom_window();
                } else {
                    this.title_drag = Some(event.position);
                }
            }),
        )
        .on_mouse_up(
            MouseButton::Left,
            cx.listener(|this, _, _, _| this.title_drag = None),
        )
        .on_mouse_down_out(cx.listener(|this, _, _, _| this.title_drag = None))
        .on_mouse_move(cx.listener(
            |this, event: &MouseMoveEvent, window, _| {
                let Some(start) = this.title_drag else {
                    return;
                };
                if event.pressed_button != Some(MouseButton::Left) {
                    this.title_drag = None;
                    return;
                }
                let moved = event.position - start;
                if moved.x.as_f32().hypot(moved.y.as_f32()) >= DRAG_SLOP {
                    this.title_drag = None;
                    window.start_window_move();
                }
            },
        ))
        .on_mouse_down(MouseButton::Right, |event, window, _| {
            window.show_window_menu(event.position);
        })
        // GNOME centres the title on the whole bar, not on what the buttons
        // leave of it, so it spans the bar and the buttons paint over it.
        // The side margins keep a long title clear of them.
        .child(
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .px(size::TITLE_BAR_CONTROLS)
                .child(
                    div()
                        .min_w_0()
                        .overflow_hidden()
                        .text_ellipsis()
                        .whitespace_nowrap()
                        .text_size(text::TITLE)
                        .font_weight(FontWeight::BOLD)
                        .text_color(if window.is_window_active() {
                            theme.foreground
                        } else {
                            theme.secondary
                        })
                        .child(title.to_owned()),
                ),
        )
        .child(controls(theme, window));
    Some(bar.into_any_element())
}

/// Minimize, maximize or restore, and close: what the window manager says
/// it can honour, and close always.
fn controls(theme: &Theme, window: &Window) -> Div {
    let supported = window.window_controls();
    let zoom_glyph = if window.is_maximized() {
        RESTORE
    } else {
        MAXIMIZE
    };
    div()
        .relative()
        .flex()
        .flex_row()
        .items_center()
        .gap(space::SM)
        .when(supported.minimize, |row| {
            row.child(control("window-minimize", MINIMIZE, theme, |window| {
                window.minimize_window();
            }))
        })
        .when(supported.maximize, |row| {
            row.child(control("window-zoom", zoom_glyph, theme, |window| {
                window.zoom_window();
            }))
        })
        .child(control("window-close", CLOSE, theme, |w| {
            w.remove_window();
        }))
}

/// One round window button, as GNOME draws them.
fn control(
    id: &'static str,
    glyph: &'static str,
    theme: &Theme,
    act: impl Fn(&mut Window) + 'static,
) -> impl IntoElement {
    let rest = theme.foreground.opacity(0.1);
    let hover = theme.foreground.opacity(0.18);
    let press = theme.foreground.opacity(0.26);
    div()
        .id(id)
        .debug_selector(move || id.into())
        .flex()
        .items_center()
        .justify_center()
        .size(space::XL)
        .rounded_full()
        .bg(rest)
        .text_color(theme.foreground)
        .hover(move |style| style.bg(hover))
        .active(move |style| style.bg(press))
        // The bar under the button would take the press as the start of a
        // move or, twice, as a maximize.
        .on_mouse_down(MouseButton::Left, |_, window, cx| {
            window.prevent_default();
            cx.stop_propagation();
        })
        .on_click(move |_, window, cx| {
            cx.stop_propagation();
            act(window);
        })
        .child(
            svg()
                .path(glyph)
                .size(icon::WINDOW)
                .text_color(theme.foreground),
        )
}

/// GNOME's ambient shadow, fainter while another window has the focus. The
/// blur stays inside [`SHADOW`]: paint bounds do not grow for it, so a wider
/// one would be cut off at the surface's edge.
fn shadow(active: bool) -> Vec<BoxShadow> {
    let alpha = if active { 0.22 } else { 0.12 };
    let black = |a: f32| Hsla {
        h: 0.,
        s: 0.,
        l: 0.,
        a,
    };
    vec![
        BoxShadow {
            color: black(alpha),
            blur_radius: px(8.),
            spread_radius: px(-1.),
            offset: point(px(0.), px(2.)),
            inset: false,
        },
        BoxShadow {
            color: black(alpha),
            blur_radius: px(2.),
            spread_radius: px(0.),
            offset: point(px(0.), px(1.)),
            inset: false,
        },
    ]
}

/// The shadow's room on each side that is not against a screen edge.
fn frame_insets(tiling: Tiling) -> Edges<Pixels> {
    let side = |tiled: bool| if tiled { px(0.) } else { SHADOW };
    Edges {
        top: side(tiling.top),
        right: side(tiling.right),
        bottom: side(tiling.bottom),
        left: side(tiling.left),
    }
}

/// Which edge a press at `pos` resizes, in a band either side of the
/// visible frame. Corners win over the edges they join.
fn resize_edge(
    pos: Point<Pixels>,
    size: Size<Pixels>,
    insets: Edges<Pixels>,
    tiling: Tiling,
) -> Option<ResizeEdge> {
    let left = insets.left;
    let right = size.width - insets.right;
    let top = insets.top;
    let bottom = size.height - insets.bottom;
    let near = |value: Pixels, line: Pixels| {
        value >= line - RESIZE_HIT && value <= line + RESIZE_HIT
    };
    let within = |value: Pixels, from: Pixels, to: Pixels| {
        value >= from - RESIZE_HIT && value <= to + RESIZE_HIT
    };
    let on_left =
        !tiling.left && near(pos.x, left) && within(pos.y, top, bottom);
    let on_right =
        !tiling.right && near(pos.x, right) && within(pos.y, top, bottom);
    let on_top = !tiling.top && near(pos.y, top) && within(pos.x, left, right);
    let on_bottom =
        !tiling.bottom && near(pos.y, bottom) && within(pos.x, left, right);
    match (on_top, on_bottom, on_left, on_right) {
        (true, _, true, _) => Some(ResizeEdge::TopLeft),
        (true, _, _, true) => Some(ResizeEdge::TopRight),
        (_, true, true, _) => Some(ResizeEdge::BottomLeft),
        (_, true, _, true) => Some(ResizeEdge::BottomRight),
        (true, ..) => Some(ResizeEdge::Top),
        (_, true, ..) => Some(ResizeEdge::Bottom),
        (_, _, true, _) => Some(ResizeEdge::Left),
        (.., true) => Some(ResizeEdge::Right),
        _ => None,
    }
}

const fn resize_cursor(edge: ResizeEdge) -> CursorStyle {
    match edge {
        ResizeEdge::Top | ResizeEdge::Bottom => CursorStyle::ResizeUpDown,
        ResizeEdge::Left | ResizeEdge::Right => CursorStyle::ResizeLeftRight,
        ResizeEdge::TopLeft | ResizeEdge::BottomRight => {
            CursorStyle::ResizeUpLeftDownRight
        }
        ResizeEdge::TopRight | ResizeEdge::BottomLeft => {
            CursorStyle::ResizeUpRightDownLeft
        }
    }
}

/// Cursor-only strips over the resize bands, in window coordinates. The
/// press itself is caught by the frame, from [`resize_edge`]; these only
/// show which way the edge under the pointer goes.
fn resize_zones(
    size: Size<Pixels>,
    insets: Edges<Pixels>,
    tiling: Tiling,
) -> Vec<AnyElement> {
    let left = insets.left - RESIZE_HIT;
    let right = size.width - insets.right - RESIZE_HIT;
    let top = insets.top - RESIZE_HIT;
    let bottom = size.height - insets.bottom - RESIZE_HIT;
    let band = RESIZE_HIT + RESIZE_HIT;
    let span_x = right - left + band;
    let span_y = bottom - top + band;
    let zone =
        |edge: ResizeEdge, x: Pixels, y: Pixels, w: Pixels, h: Pixels| {
            div()
                .absolute()
                .left(x)
                .top(y)
                .w(w)
                .h(h)
                .cursor(resize_cursor(edge))
                .into_any_element()
        };
    let mut zones = Vec::new();
    // Edges first, corners after, so the corners are on top where they
    // overlap.
    if !tiling.top {
        zones.push(zone(ResizeEdge::Top, left, top, span_x, band));
    }
    if !tiling.bottom {
        zones.push(zone(ResizeEdge::Bottom, left, bottom, span_x, band));
    }
    if !tiling.left {
        zones.push(zone(ResizeEdge::Left, left, top, band, span_y));
    }
    if !tiling.right {
        zones.push(zone(ResizeEdge::Right, right, top, band, span_y));
    }
    let corners = [
        (tiling.top || tiling.left, ResizeEdge::TopLeft, left, top),
        (tiling.top || tiling.right, ResizeEdge::TopRight, right, top),
        (
            tiling.bottom || tiling.left,
            ResizeEdge::BottomLeft,
            left,
            bottom,
        ),
        (
            tiling.bottom || tiling.right,
            ResizeEdge::BottomRight,
            right,
            bottom,
        ),
    ];
    for (tiled, edge, x, y) in corners {
        if !tiled {
            zones.push(zone(edge, x, y, band, band));
        }
    }
    zones
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_window_glyphs_load_beside_the_default_bundle() {
        for glyph in [MINIMIZE, MAXIMIZE, RESTORE, CLOSE] {
            let bytes = Assets.load(glyph).unwrap().unwrap();
            assert!(bytes.starts_with(b"<svg"), "{glyph}");
        }
        // The rest of the app still draws its icons from the bundle.
        let check = gpui_kit::assets::IconName::Check.path();
        assert!(Assets.load(&check).unwrap().is_some());
    }

    fn floating() -> Tiling {
        Tiling::default()
    }

    fn window() -> Size<Pixels> {
        Size::new(px(1000.), px(800.))
    }

    #[test]
    fn a_press_on_the_visible_edge_resizes_that_edge() {
        let insets = frame_insets(floating());
        let edge =
            resize_edge(point(SHADOW, px(400.)), window(), insets, floating());
        assert_eq!(edge, Some(ResizeEdge::Left));
        let edge = resize_edge(
            point(px(500.), px(800.) - SHADOW),
            window(),
            insets,
            floating(),
        );
        assert_eq!(edge, Some(ResizeEdge::Bottom));
    }

    #[test]
    fn corners_win_over_the_edges_they_join() {
        let insets = frame_insets(floating());
        let edge = resize_edge(
            point(px(1000.) - SHADOW, SHADOW),
            window(),
            insets,
            floating(),
        );
        assert_eq!(edge, Some(ResizeEdge::TopRight));
    }

    #[test]
    fn the_middle_and_tiled_edges_do_not_resize() {
        let insets = frame_insets(floating());
        assert_eq!(
            resize_edge(
                point(px(500.), px(400.)),
                window(),
                insets,
                floating()
            ),
            None
        );
        let tiled = Tiling::tiled();
        assert_eq!(
            resize_edge(
                point(px(0.), px(400.)),
                window(),
                frame_insets(tiled),
                tiled
            ),
            None
        );
    }
}
