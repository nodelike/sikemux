//! Systems with no browser engine wired in yet, such as Windows. No tab ever opens on them,
//! so a page is a type with no values and everything that would act on one is unreachable.

use std::sync::Arc;

use crate::{ParentView, TabEvents, TabSpec};

pub mod input;
pub mod page;

pub const NOT_HERE: &str = "the browser is not on this system yet";

pub enum Page {}

pub enum PageView {}

impl PageView {
    pub fn show(&self, _html: &str) {
        match *self {}
    }

    pub fn run_script(&self, _script: &str) {
        match *self {}
    }
}

#[derive(Clone)]
pub struct Tab(Never);

#[derive(Clone)]
enum Never {}

impl Tab {
    pub fn open(
        _parent: ParentView,
        _spec: TabSpec<'_>,
        _events: Arc<dyn TabEvents>,
    ) -> Result<Tab, String> {
        Err(NOT_HERE.into())
    }

    pub fn id(&self) -> &str {
        match self.0 {}
    }

    pub fn show(&self) {
        match self.0 {}
    }

    pub fn hide(&self) {
        match self.0 {}
    }

    pub fn close(&self) {
        match self.0 {}
    }

    pub fn set_zoom(&self, _zoom: f64) {
        match self.0 {}
    }

    pub fn reload(&self) {
        match self.0 {}
    }

    pub fn navigate(&self, _url: &str) {
        match self.0 {}
    }

    pub fn set_bounds(&self, _frame: (f64, f64, f64, f64), _sides: (f64, f64)) {
        match self.0 {}
    }

    pub async fn eval(&self, _script: &str) -> Result<String, String> {
        match self.0 {}
    }

    pub fn run(&self, _act: impl FnOnce(&Page) + Send + 'static) {
        match self.0 {}
    }

    pub async fn ask<T: Send + 'static>(
        &self,
        _act: impl FnOnce(&Page) -> T + Send + 'static,
    ) -> Result<T, String> {
        match self.0 {}
    }
}

pub fn answer(_tab_id: &str, _accept: bool, _text: Option<&str>) -> Result<(), String> {
    Err(NOT_HERE.into())
}

pub fn clear_data(_profile: crate::ProfileStore) {}

pub fn remove_data_store(_profile: crate::ProfileStore) {}
