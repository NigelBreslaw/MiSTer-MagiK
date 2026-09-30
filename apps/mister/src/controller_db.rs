// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later
//! Device-layout boundary for the portable controller registry.
use crate::input_info::PadInfo;
use mister_magik_controller_registry::ControllerDb as Registry;
pub use mister_magik_controller_registry::{
    ControllerEntry, ControllerKind, ControllerListItem, PadRegistryStatus,
};
#[derive(Debug, Clone)]
pub struct ControllerDb {
    pub(crate) inner: Registry,
    save_observer: Option<mister_magik_controller_registry::SaveObserver>,
}
impl ControllerDb {
    pub fn load() -> Self {
        let path = mister_magik_catalog::device_layout::current_app_path("controllers.json");
        Self::load_from(&path.to_string_lossy())
    }
    pub fn load_from(path: &str) -> Self {
        Self {
            inner: Registry::load_from(path),
            save_observer: None,
        }
    }
    #[cfg(any(feature = "ui", test))]
    pub(crate) fn observe_saves(
        &mut self,
        observer: mister_magik_controller_registry::SaveObserver,
    ) {
        self.save_observer = Some(observer);
    }
    pub fn is_persistence_pending(&self) -> bool {
        self.save_observer.as_ref().is_some_and(|o| o.is_pending())
    }
    pub fn logical_id(info: &PadInfo) -> String {
        Registry::logical_id(info)
    }
    pub fn plug_id(info: &PadInfo) -> String {
        Registry::plug_id(info)
    }
    pub fn default_entry(info: &PadInfo) -> ControllerEntry {
        Registry::default_entry(info)
    }
    pub fn infer_kind(info: &PadInfo) -> ControllerKind {
        Registry::infer_kind(info)
    }
}
impl std::ops::Deref for ControllerDb {
    type Target = Registry;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}
impl std::ops::DerefMut for ControllerDb {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}
