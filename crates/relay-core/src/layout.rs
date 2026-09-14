//! How the window was arranged, so it opens the way it was left: its size and place on
//! the screen, the split between the panes, whether the transfers drawer and the sidebar
//! were open.
//!
//! Always restored, whatever the launch setting says about tabs. Starting fresh means
//! not reopening yesterday's servers; a window that forgets its own size is not fresh,
//! only forgetful. Recorded like the workspace — see [`crate::recorded`].

use serde::{Deserialize, Serialize};
use specta::Type;

use crate::recorded::Recorded;

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Layout {
    pub interface: InterfaceLayout,
    pub window: Option<WindowGeometry>,
}

/// The half only the interface can see.
///
/// Every field is optional, so a file written before a field existed puts the rest back
/// and leaves that one at the interface's own default.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase", default)]
pub struct InterfaceLayout {
    /// The local pane's share of the pane area, as a percentage.
    pub local_pane_percent: Option<f32>,
    pub drawer_open: Option<bool>,
    pub drawer_tab: Option<DrawerTab>,
    pub sidebar_collapsed: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum DrawerTab {
    Active,
    Failed,
    Completed,
}

/// The half the shell reads from the window itself.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowGeometry {
    /// Logical size, so a window that reopens on a display with another scale keeps the
    /// size it appeared to have.
    pub width: f64,
    pub height: f64,
    /// Physical position of the top-left corner, in the desktop's coordinates.
    pub x: i32,
    pub y: i32,
    /// Maximised when it was left. The size is the one it had before, so that
    /// un-maximising next time goes back to it rather than to the whole screen.
    pub maximized: bool,
}

/// A connected display, in the same physical coordinates as the window's position.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Screen {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl WindowGeometry {
    /// Whether putting the window back where it was leaves it within reach.
    ///
    /// A display unplugged since takes its coordinates with it, and a window restored onto
    /// one opens somewhere nobody can see or drag it back from. The test is the title bar:
    /// a point just inside the top-left corner has to land on a display connected now.
    pub fn reachable_on(&self, screens: &[Screen]) -> bool {
        let grab_x = i64::from(self.x) + 80;
        let grab_y = i64::from(self.y) + 12;
        screens.iter().any(|screen| {
            let left = i64::from(screen.x);
            let top = i64::from(screen.y);
            grab_x >= left
                && grab_y >= top
                && grab_x < left + i64::from(screen.width)
                && grab_y < top + i64::from(screen.height)
        })
    }
}

/// `layout.json`.
pub type LayoutStore = Recorded<Layout>;

#[cfg(test)]
mod tests {
    use super::*;

    const MAIN: Screen = Screen {
        x: 0,
        y: 0,
        width: 2880,
        height: 1800,
    };
    /// A second display to the left of the main one, as macOS lays it out.
    const LEFT: Screen = Screen {
        x: -1920,
        y: 0,
        width: 1920,
        height: 1080,
    };

    fn at(x: i32, y: i32) -> WindowGeometry {
        WindowGeometry {
            width: 1280.0,
            height: 820.0,
            x,
            y,
            maximized: false,
        }
    }

    #[test]
    fn a_window_on_a_connected_display_goes_back_where_it_was() {
        assert!(at(200, 100).reachable_on(&[MAIN]));
        assert!(at(-1500, 200).reachable_on(&[MAIN, LEFT]));
    }

    /// Left on a display that has since been unplugged.
    #[test]
    fn a_window_on_a_display_that_is_gone_is_not_put_back_there() {
        assert!(!at(-1500, 200).reachable_on(&[MAIN]));
        assert!(!at(4000, 200).reachable_on(&[MAIN, LEFT]));
        assert!(!at(200, 100).reachable_on(&[]));
    }

    /// Mostly off screen is still fine, as long as the title bar can be grabbed.
    #[test]
    fn a_window_hanging_off_an_edge_still_counts_if_its_title_bar_is_on_screen() {
        assert!(at(2700, 1700).reachable_on(&[MAIN]));
        assert!(!at(2860, 100).reachable_on(&[MAIN]));
    }

    #[test]
    fn the_interface_and_the_window_are_recorded_without_overwriting_each_other() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("layout.json");
        let store = LayoutStore::load(path.clone());

        store
            .update(|layout| layout.window = Some(at(10, 20)))
            .unwrap();
        store
            .update(|layout| {
                layout.interface = InterfaceLayout {
                    local_pane_percent: Some(44.0),
                    drawer_open: Some(false),
                    drawer_tab: Some(DrawerTab::Completed),
                    sidebar_collapsed: Some(true),
                }
            })
            .unwrap();

        let reopened = LayoutStore::load(path).get();
        assert_eq!(reopened.window, Some(at(10, 20)));
        assert_eq!(reopened.interface.drawer_tab, Some(DrawerTab::Completed));
        assert_eq!(reopened.interface.local_pane_percent, Some(44.0));
    }

    /// A file from before a field existed restores what it has.
    #[test]
    fn a_partial_file_restores_what_it_has() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("layout.json");
        std::fs::write(&path, br#"{"interface":{"drawerOpen":false}}"#).unwrap();

        let layout = LayoutStore::load(path).get();
        assert_eq!(layout.interface.drawer_open, Some(false));
        assert_eq!(layout.interface.sidebar_collapsed, None);
        assert_eq!(layout.window, None);
    }
}
