//! What opens over the window or in place of part of it: pickers, dialogs, the welcome, a
//! title being edited. Each is held here, and [`Overlay`] says of each what it does with the keys
//! and where it draws, so the window answers what is open from one list.

use gpui_kit::Entity;

use crate::workbench::{Asking, Retitling};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Overlay {
    Asking,
    Palette,
    Finder,
    Switcher,
    AddingTask,
    AddingProvider,
    BuddyPicker,
    Sharing,
    RunCheck,
    SpaceEditor,
    Experience,
    CommandPopup,
    Retitling,
}

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

impl Overlay {
    /// Whether the keyboard stays with it while it is open, rather than going back to the view in
    /// front when nothing drawn holds it. A held-back task's card sits on the Run chip and asks
    /// nothing of the keys.
    pub fn takes_keys(self) -> bool {
        self != Overlay::RunCheck
    }

    pub fn layer(self) -> Layer {
        match self {
            Overlay::Palette | Overlay::Finder | Overlay::Switcher => Layer::Finder,
            Overlay::Experience | Overlay::CommandPopup => Layer::Over,
            Overlay::Asking
            | Overlay::AddingTask
            | Overlay::AddingProvider
            | Overlay::BuddyPicker
            | Overlay::Sharing
            | Overlay::RunCheck
            | Overlay::SpaceEditor
            | Overlay::Retitling => Layer::InPlace,
        }
    }
}

#[derive(Default)]
pub struct Overlays {
    /// The question the shell is putting to the person, over the window.
    pub(crate) asking: Option<Asking>,
    pub palette: Option<crate::palette::Palette>,
    pub finder: Option<crate::finder::Finder>,
    pub switcher: Option<crate::switcher::Switcher>,
    /// The process the person is writing in the Add a process dialog, while it is open.
    pub adding_task: Option<crate::add_task::AddTask>,
    pub adding_provider: Option<crate::add_provider::AddProvider>,
    pub buddy_picker: Option<crate::buddy_picker::BuddyPicker>,
    pub sharing: Option<crate::share::Sharing>,
    /// A task its checks held back, waiting on the person.
    pub run_check: Option<crate::run_check::RunCheck>,
    /// Edit Spaces, while it is open under the space switch.
    pub space_editor: Option<Entity<crate::space_editor::SpaceEditor>>,
    /// The welcome, the release notes or the diagnostics.
    pub experience: Option<crate::experience::Experience>,
    /// A custom command running in a terminal over the window.
    pub command_popup: Option<crate::command_popup::Popup>,
    /// The agent whose title is being edited in place, in its header or its rail row.
    pub retitling: Option<Retitling>,
}

impl Overlays {
    /// Every overlay open now.
    pub fn open(&self) -> impl Iterator<Item = Overlay> {
        [
            (self.asking.is_some(), Overlay::Asking),
            (self.palette.is_some(), Overlay::Palette),
            (self.finder.is_some(), Overlay::Finder),
            (self.switcher.is_some(), Overlay::Switcher),
            (self.adding_task.is_some(), Overlay::AddingTask),
            (self.adding_provider.is_some(), Overlay::AddingProvider),
            (self.buddy_picker.is_some(), Overlay::BuddyPicker),
            (self.sharing.is_some(), Overlay::Sharing),
            (self.run_check.is_some(), Overlay::RunCheck),
            (self.space_editor.is_some(), Overlay::SpaceEditor),
            (self.experience.is_some(), Overlay::Experience),
            (self.command_popup.is_some(), Overlay::CommandPopup),
            (self.retitling.is_some(), Overlay::Retitling),
        ]
        .into_iter()
        .filter_map(|(open, overlay)| open.then_some(overlay))
    }

    /// Whether an open overlay keeps the keyboard.
    pub fn take_keys(&self) -> bool {
        self.open().any(Overlay::takes_keys)
    }

    /// Whether an overlay is open in `layer`.
    pub fn in_layer(&self, layer: Layer) -> bool {
        self.open().any(|overlay| overlay.layer() == layer)
    }
}
