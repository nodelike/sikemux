//! What opens over the window or in place of part of it: pickers, dialogs, the welcome, a
//! title being edited. Each is held here, and [`Overlay`] says of each what it does with the keys
//! and where it draws, so the window answers what is open from one list.

use gpui_kit::Entity;

use crate::asking::Asking;
use crate::workbench::Retitling;

/// Where an overlay draws.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Layer {
    /// The pickers, in their own part over the stage.
    Finder,
    /// Over the whole window, in the part that covers it.
    Over,
    /// Where it belongs: a dialog over the window, a card on the stage, a field in a row.
    InPlace,
}

macro_rules! overlays {
    ($(
        $(#[$doc:meta])*
        $field:ident: $held:ty => $overlay:ident, $layer:ident, $keys:literal;
    )*) => {
        #[derive(Clone, Copy, PartialEq, Eq, Debug)]
        pub enum Overlay {
            $($overlay,)*
        }

        impl Overlay {
            /// Whether the keyboard stays with it while it is open, rather than going back to the
            /// view in front when nothing drawn holds it.
            pub fn takes_keys(self) -> bool {
                match self {
                    $(Overlay::$overlay => $keys,)*
                }
            }

            pub fn layer(self) -> Layer {
                match self {
                    $(Overlay::$overlay => Layer::$layer,)*
                }
            }
        }

        #[derive(Default)]
        pub struct Overlays {
            $($(#[$doc])* pub $field: Option<$held>,)*
        }

        impl Overlays {
            /// Every overlay open now.
            pub fn open(&self) -> impl Iterator<Item = Overlay> {
                [$((self.$field.is_some(), Overlay::$overlay),)*]
                    .into_iter()
                    .filter_map(|(open, overlay)| open.then_some(overlay))
            }
        }
    };
}

overlays! {
    /// The question the shell is putting to the person, over the window.
    asking: Asking => Asking, InPlace, true;
    palette: crate::palette::Palette => Palette, Finder, true;
    finder: crate::finder::Finder => Finder, Finder, true;
    switcher: crate::switcher::Switcher => Switcher, Finder, true;
    /// The process the person is writing in the Add a process dialog, while it is open.
    adding_task: crate::add_task::AddTask => AddingTask, InPlace, true;
    adding_provider: crate::add_provider::AddProvider => AddingProvider, InPlace, true;
    buddy_picker: crate::buddy_picker::BuddyPicker => BuddyPicker, InPlace, true;
    sharing: crate::share::Sharing => Sharing, InPlace, true;
    /// A task its checks held back, waiting on the person. Its card sits on the Run chip and asks
    /// nothing of the keys.
    run_check: crate::run_check::RunCheck => RunCheck, InPlace, false;
    /// Edit Spaces, while it is open under the space switch.
    space_editor: Entity<crate::space_editor::SpaceEditor> => SpaceEditor, InPlace, true;
    /// The welcome, the release notes or the diagnostics.
    experience: crate::experience::Experience => Experience, Over, true;
    /// A custom command running in a terminal over the window.
    command_popup: crate::command_popup::Popup => CommandPopup, Over, true;
    /// The agent whose title is being edited in place, in its header or its rail row.
    retitling: Retitling => Retitling, InPlace, true;
}

impl Overlays {
    /// Whether an open overlay keeps the keyboard.
    pub fn take_keys(&self) -> bool {
        self.open().any(Overlay::takes_keys)
    }

    /// Whether an overlay is open in `layer`.
    pub fn in_layer(&self, layer: Layer) -> bool {
        self.open().any(|overlay| overlay.layer() == layer)
    }
}
