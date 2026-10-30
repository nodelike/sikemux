//! The device's own frame around its live screen, as Xcode's DeviceKit draws it: the bezel,
//! the side buttons the person can press, and the screen's outline, all turned with the device.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use gpui::RenderImage;
use image::{imageops, RgbaImage};
use serde_json::Value;

/// A rectangle in points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

/// The chrome as the helper hands it over, upright.
pub struct Chrome {
    /// The bezel's size in points, inside the room its side buttons need.
    art: (f32, f32),
    top: f32,
    left: f32,
    bottom: f32,
    right: f32,
    bezel: RgbaImage,
    /// Where the screen shows, by its alpha, at the screen's own size.
    mask: Option<RgbaImage>,
    buttons: Vec<Button>,
    turned: RefCell<HashMap<(i32, u32, u32), Rc<Turned>>>,
}

struct Button {
    name: String,
    title: String,
    on_left: bool,
    /// Its left edge from the frame's side, or its right edge when on the right; `hover_x`
    /// is where it slides out to under the pointer.
    x: f32,
    hover_x: f32,
    y: f32,
    w: f32,
    h: f32,
    image: RgbaImage,
    image_down: Option<RgbaImage>,
}

/// The frame laid out for one way of holding the device, in points of the turned frame.
pub struct Turned {
    pub size: (f32, f32),
    pub bezel: (Rect, Arc<RenderImage>),
    pub screen: Rect,
    /// The bezel's own pixels wherever the screen's outline hides the screen, to draw over it.
    pub cap: Option<Arc<RenderImage>>,
    pub buttons: Vec<TurnedButton>,
}

pub struct TurnedButton {
    pub name: String,
    pub title: String,
    /// The strip of the frame's side the button slides along, which clips it.
    pub clip: Rect,
    /// Where the button rests and where it slides out to, inside `clip`.
    pub rest: Rect,
    pub out: Rect,
    pub image: Arc<RenderImage>,
    pub image_down: Option<Arc<RenderImage>>,
}

impl Chrome {
    /// The helper's `chrome` answer, with its `mask` answer, as pictures to draw.
    pub fn read(chrome: &Value, mask: Option<&str>) -> Option<Chrome> {
        let number = |value: &Value| value.as_f64().map(|value| value as f32);
        let padding = &chrome["padding"];
        let buttons = chrome["buttons"]
            .as_array()
            .map_or(&[][..], Vec::as_slice)
            .iter()
            .filter_map(|button| {
                Some(Button {
                    name: button["name"].as_str()?.to_owned(),
                    title: button["title"].as_str().unwrap_or_default().to_owned(),
                    on_left: button["anchor"] == "left",
                    x: number(&button["x"])?,
                    hover_x: number(&button["hoverX"])?,
                    y: number(&button["y"])?,
                    w: number(&button["width"])?,
                    h: number(&button["height"])?,
                    image: picture(button["image"].as_str()?)?,
                    image_down: button["imageDown"].as_str().and_then(picture),
                })
            })
            .collect();
        Some(Chrome {
            art: (number(&chrome["width"])?, number(&chrome["height"])?),
            top: number(&padding["top"]).unwrap_or(0.),
            left: number(&padding["left"]).unwrap_or(0.),
            bottom: number(&padding["bottom"]).unwrap_or(0.),
            right: number(&padding["right"]).unwrap_or(0.),
            bezel: picture(chrome["image"].as_str()?)?,
            mask: mask.and_then(picture),
            buttons,
            turned: RefCell::new(HashMap::new()),
        })
    }

    /// The frame turned `degrees` clockwise around a screen `screen` points across as turned.
    pub fn turned(&self, degrees: i32, screen: (f32, f32)) -> Rc<Turned> {
        let degrees = degrees.rem_euclid(360);
        let key = (degrees, screen.0.round() as u32, screen.1.round() as u32);
        if let Some(turned) = self.turned.borrow().get(&key) {
            return Rc::clone(turned);
        }
        let turned = Rc::new(self.turn(degrees, screen));
        self.turned.borrow_mut().insert(key, Rc::clone(&turned));
        turned
    }

    fn turn(&self, degrees: i32, screen: (f32, f32)) -> Turned {
        let upright = (
            self.art.0 + self.left + self.right,
            self.art.1 + self.top + self.bottom,
        );
        let sideways = degrees % 180 != 0;
        let screen = match sideways {
            true => (screen.1, screen.0),
            false => screen,
        };
        let turn = |rect: Rect| turn_rect(rect, upright, degrees);
        let bezel = Rect {
            x: self.left,
            y: self.top,
            w: self.art.0,
            h: self.art.1,
        };
        let shown = Rect {
            x: self.left + (self.art.0 - screen.0) / 2.,
            y: self.top + (self.art.1 - screen.1) / 2.,
            w: screen.0,
            h: screen.1,
        };
        let buttons = self
            .buttons
            .iter()
            .map(|button| {
                let side = if button.on_left {
                    self.left
                } else {
                    self.right
                };
                let from = if button.on_left { 0. } else { upright.0 - side };
                let clip = Rect {
                    x: from,
                    y: self.top + button.y,
                    w: side,
                    h: button.h,
                };
                let at = |x: f32| Rect {
                    x: if button.on_left {
                        x
                    } else {
                        upright.0 + x - button.w
                    },
                    y: clip.y,
                    w: button.w,
                    h: button.h,
                };
                let clip_turned = turn(clip);
                let inside = |rect: Rect| {
                    let rect = turn(rect);
                    Rect {
                        x: rect.x - clip_turned.x,
                        y: rect.y - clip_turned.y,
                        ..rect
                    }
                };
                TurnedButton {
                    name: button.name.clone(),
                    title: button.title.clone(),
                    clip: clip_turned,
                    rest: inside(at(button.x)),
                    out: inside(at(button.hover_x)),
                    image: render(turn_image(&button.image, degrees)),
                    image_down: button
                        .image_down
                        .as_ref()
                        .map(|image| render(turn_image(image, degrees))),
                }
            })
            .collect();
        let size = match sideways {
            true => (upright.1, upright.0),
            false => upright,
        };
        Turned {
            size,
            bezel: (turn(bezel), render(turn_image(&self.bezel, degrees))),
            screen: turn(shown),
            cap: self.cap(shown).map(|cap| render(turn_image(&cap, degrees))),
            buttons,
        }
    }

    /// The bezel's pixels over the upright screen `shown` wherever the mask hides the screen.
    fn cap(&self, shown: Rect) -> Option<RgbaImage> {
        let mask = self.mask.as_ref()?;
        let (wide, high) = mask.dimensions();
        let per_point = (
            self.bezel.width() as f32 / self.art.0,
            self.bezel.height() as f32 / self.art.1,
        );
        Some(RgbaImage::from_fn(wide, high, |x, y| {
            let hidden = 255 - mask.get_pixel(x, y)[3];
            if hidden == 0 {
                return image::Rgba([0, 0, 0, 0]);
            }
            let point = (
                shown.x - self.left + (x as f32 + 0.5) / wide as f32 * shown.w,
                shown.y - self.top + (y as f32 + 0.5) / high as f32 * shown.h,
            );
            let pixel = (
                ((point.0 * per_point.0) as u32).min(self.bezel.width() - 1),
                ((point.1 * per_point.1) as u32).min(self.bezel.height() - 1),
            );
            let mut under = *self.bezel.get_pixel(pixel.0, pixel.1);
            under[3] = ((u16::from(under[3]) * u16::from(hidden)) / 255) as u8;
            under
        }))
    }
}

/// How far clockwise the screen turns for each way the device is held, as the helper names them.
pub fn turn_degrees(orientation: &str) -> i32 {
    match orientation {
        "landscapeRight" => 90,
        "portraitUpsideDown" => 180,
        "landscapeLeft" => -90,
        _ => 0,
    }
}

/// `rect` in a frame `size` across, as it lands once the frame turns `degrees` clockwise.
fn turn_rect(rect: Rect, size: (f32, f32), degrees: i32) -> Rect {
    let (wide, high) = size;
    match degrees {
        90 => Rect {
            x: high - rect.y - rect.h,
            y: rect.x,
            w: rect.h,
            h: rect.w,
        },
        180 => Rect {
            x: wide - rect.x - rect.w,
            y: high - rect.y - rect.h,
            ..rect
        },
        270 => Rect {
            x: rect.y,
            y: wide - rect.x - rect.w,
            w: rect.h,
            h: rect.w,
        },
        _ => rect,
    }
}

fn turn_image(image: &RgbaImage, degrees: i32) -> RgbaImage {
    match degrees {
        90 => imageops::rotate90(image),
        180 => imageops::rotate180(image),
        270 => imageops::rotate270(image),
        _ => image.clone(),
    }
}

/// A PNG `data:` address, decoded.
fn picture(data: &str) -> Option<RgbaImage> {
    use base64::Engine;
    let (_, body) = data.strip_prefix("data:")?.split_once(";base64,")?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(body)
        .ok()?;
    Some(image::load_from_memory(&bytes).ok()?.into_rgba8())
}

/// `image` as the sprite atlas takes it.
fn render(mut image: RgbaImage) -> Arc<RenderImage> {
    for pixel in image.pixels_mut() {
        pixel.0.swap(0, 2);
    }
    Arc::new(RenderImage::new([image::Frame::new(image)]))
}
