use gpui::{div, px, IntoElement, ParentElement, Styled};

pub const ROW_HEIGHT: f32 = 26.0;

pub fn row(label: &str, selected: bool) -> impl IntoElement {
    let base = div().h(px(ROW_HEIGHT)).px(px(8.0)).child(label.to_owned());
    if selected {
        base.bg(gpui::rgb(0x2a2a33))
    } else {
        base
    }
}
