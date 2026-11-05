use gpui::{div, px, IntoElement, ParentElement, Styled};

pub fn tab_pill(title: &str, active: bool) -> impl IntoElement {
    div()
        .h(px(24.0))
        .px(px(10.0))
        .rounded(px(6.0))
        .when(active, |pill| pill.bg(gpui::rgb(0x34343d)))
        .child(title.to_owned())
}
