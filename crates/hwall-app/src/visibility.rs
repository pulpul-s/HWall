use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct VisibilityState {
    hidden: BTreeMap<String, String>,
    collapsed: BTreeSet<String>,
    favorites: BTreeSet<String>,
}

impl VisibilityState {
    pub fn hide(&mut self, key: impl Into<String>, label: impl Into<String>) {
        self.hidden.insert(key.into(), label.into());
    }

    pub fn show(&mut self, key: &str) -> bool {
        self.hidden.remove(key).is_some()
    }

    pub fn show_all(&mut self) {
        self.hidden.clear();
    }

    pub fn is_hidden(&self, key: &str) -> bool {
        self.hidden.contains_key(key)
    }

    pub fn hidden_items(&self) -> impl Iterator<Item = (&str, &str)> {
        self.hidden
            .iter()
            .map(|(key, label)| (key.as_str(), label.as_str()))
    }

    pub fn toggle_collapsed(&mut self, key: impl Into<String>) -> bool {
        toggle_membership(&mut self.collapsed, key.into())
    }

    pub fn is_collapsed(&self, key: &str) -> bool {
        self.collapsed.contains(key)
    }

    pub fn expand_all(&mut self) {
        self.collapsed.clear();
    }

    pub fn toggle_favorite(&mut self, key: impl Into<String>) -> bool {
        toggle_membership(&mut self.favorites, key.into())
    }

    pub fn is_favorite(&self, key: &str) -> bool {
        self.favorites.contains(key)
    }
}

fn toggle_membership(set: &mut BTreeSet<String>, key: String) -> bool {
    if set.remove(&key) {
        false
    } else {
        set.insert(key);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visibility_actions_are_independent() {
        let mut state = VisibilityState::default();
        state.hide("sensor:cpu:temp", "CPU temperature");
        assert!(state.is_hidden("sensor:cpu:temp"));
        assert!(state.toggle_favorite("sensor:cpu:temp"));
        assert!(state.is_favorite("sensor:cpu:temp"));
        assert!(state.toggle_collapsed("device:cpu"));
        assert!(state.is_collapsed("device:cpu"));

        state.show_all();
        assert!(!state.is_hidden("sensor:cpu:temp"));
        assert!(state.is_favorite("sensor:cpu:temp"));
        assert!(state.is_collapsed("device:cpu"));
    }
}
