#[cfg(feature = "tray")]
use ksni::blocking::TrayMethods;
#[cfg(feature = "tray")]
use std::sync::mpsc::Sender;
use std::sync::mpsc::{self, Receiver};

#[cfg(feature = "tray")]
const TRAY_ICON_SIZE: i32 = 64;
#[cfg(feature = "tray")]
const TRAY_ICON_ARGB: &[u8; 64 * 64 * 4] = include_bytes!("../resources/hwall-tray-64.argb");

#[derive(Debug, Clone, Copy)]
pub(super) enum TrayAction {
    Show,
    TogglePause,
    ToggleLogging,
    ResetStatistics,
    Quit,
}

pub(super) struct TrayBridge {
    pub(super) actions: Receiver<TrayAction>,
    #[cfg(feature = "tray")]
    _handle: Option<ksni::blocking::Handle<HWallTray>>,
    pub(super) available: bool,
}

impl TrayBridge {
    pub(super) fn start() -> Self {
        let (tx, rx) = mpsc::channel();
        #[cfg(feature = "tray")]
        {
            let tray = HWallTray { actions: tx };
            match tray.spawn() {
                Ok(handle) => Self {
                    actions: rx,
                    _handle: Some(handle),
                    available: true,
                },
                Err(_) => Self {
                    actions: rx,
                    _handle: None,
                    available: false,
                },
            }
        }
        #[cfg(not(feature = "tray"))]
        {
            let _ = tx;
            Self {
                actions: rx,
                available: false,
            }
        }
    }
}

#[cfg(feature = "tray")]
struct HWallTray {
    actions: Sender<TrayAction>,
}

#[cfg(feature = "tray")]
impl HWallTray {
    fn send(&self, action: TrayAction) {
        let _ = self.actions.send(action);
    }
}

#[cfg(feature = "tray")]
impl ksni::Tray for HWallTray {
    fn id(&self) -> String {
        "hwall".to_owned()
    }

    fn title(&self) -> String {
        "HWall".to_owned()
    }

    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        vec![ksni::Icon {
            width: TRAY_ICON_SIZE,
            height: TRAY_ICON_SIZE,
            data: TRAY_ICON_ARGB.to_vec(),
        }]
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        self.send(TrayAction::Show);
    }

    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        use ksni::menu::StandardItem;
        vec![
            StandardItem {
                label: "Show HWall".to_owned(),
                icon_name: "window-restore".to_owned(),
                activate: Box::new(|tray: &mut HWallTray| tray.send(TrayAction::Show)),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: "Pause / Resume".to_owned(),
                icon_name: "media-playback-pause".to_owned(),
                activate: Box::new(|tray: &mut HWallTray| tray.send(TrayAction::TogglePause)),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: "Start / Stop Logging".to_owned(),
                icon_name: "document-save".to_owned(),
                activate: Box::new(|tray: &mut HWallTray| tray.send(TrayAction::ToggleLogging)),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: "Reset Statistics".to_owned(),
                icon_name: "view-refresh".to_owned(),
                activate: Box::new(|tray: &mut HWallTray| tray.send(TrayAction::ResetStatistics)),
                ..Default::default()
            }
            .into(),
            ksni::MenuItem::Separator,
            StandardItem {
                label: "Quit".to_owned(),
                icon_name: "application-exit".to_owned(),
                activate: Box::new(|tray: &mut HWallTray| tray.send(TrayAction::Quit)),
                ..Default::default()
            }
            .into(),
        ]
    }
}
