use crate::*;

#[derive(Clone)]
pub(crate) struct App {
    pub(crate) folder: PathBuf,
    pub(crate) name: String,
    /// The installed copy's own id: notifications, and on Windows the taskbar
    /// identity, belong to it, not to the Runner it was copied from.
    pub(crate) id: Option<String>,
    /// Load-check mode: where to write the report.
    pub(crate) check: Option<PathBuf>,
    /// The `openas` binary that runs the Helper.
    pub(crate) helper: Option<PathBuf>,
    pub(crate) manager: bool,
    /// A draft being made: the column shows the making and can't be closed.
    pub(crate) making: bool,
    /// A non-default setup from the plist, passed on to the Helper.
    pub(crate) env: Vec<(String, String)>,
}

impl App {
    /// The app's id is its folder's name (`<root>/apps/<id>`).
    pub(crate) fn app_id(&self) -> String {
        self.folder
            .file_name()
            .map(|f| f.to_string_lossy().into_owned())
            .unwrap_or_default()
    }
}

pub(crate) struct NetToggle {
    pub(crate) item: tauri::menu::CheckMenuItem<tauri::Wry>,
}

pub(crate) struct Store {
    pub(crate) file: PathBuf,
    pub(crate) map: Mutex<Map<String, Value>>,
    /// Off in the load check: the app can read the real data but never write it.
    pub(crate) persist: bool,
}

pub(crate) fn bundle_info() -> Option<plist::Dictionary> {
    // <Name>.app/Contents/MacOS/<exe> -> <Name>.app/Contents/Info.plist
    let exe = std::env::current_exe().ok()?;
    let info = exe.parent()?.parent()?.join("Info.plist");
    plist::Value::from_file(info).ok()?.into_dictionary()
}

pub(crate) fn which_app() -> App {
    let info = bundle_info();
    let plist_str = |k: &str| {
        info.as_ref()
            .and_then(|d| d.get(k))
            .and_then(|v| v.as_string())
            .map(str::to_owned)
    };
    if let Some(folder) = plist_str("OAAppFolder").filter(|s| !s.is_empty()) {
        return App {
            folder: PathBuf::from(folder),
            name: plist_str("CFBundleName").unwrap_or_else(|| "App".into()),
            id: plist_str("CFBundleIdentifier").filter(|i| i != RUNNER_ID),
            check: None,
            helper: plist_str("OAHelper").map(PathBuf::from),
            manager: info
                .as_ref()
                .and_then(|d| d.get("OAManager"))
                .and_then(|v| v.as_boolean())
                .unwrap_or(false),
            making: false,
            env: [
                ("OARoot", "OPENAS_ROOT"),
                ("OAInstallDir", "OPENAS_INSTALL_DIR"),
                ("OAIdPrefix", "OPENAS_ID_PREFIX"),
            ]
            .iter()
            .filter_map(|(k, var)| plist_str(k).map(|v| (var.to_string(), v)))
            .collect(),
        };
    }
    let args: Vec<String> = std::env::args().collect();
    let arg = |k: &str| {
        args.iter()
            .position(|a| a == k)
            .and_then(|i| args.get(i + 1).cloned())
    };
    let check_folder = arg("--check");
    let folder = PathBuf::from(
        check_folder
            .clone()
            .or_else(|| arg("--app"))
            .unwrap_or_else(|| ".".into()),
    );
    let name = std::fs::read_to_string(folder.join("app.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| v.get("name").and_then(Value::as_str).map(str::to_owned))
        .or_else(|| folder.file_name().map(|f| f.to_string_lossy().into_owned()))
        .unwrap_or_else(|| "App".into());
    let making = args.iter().any(|a| a == "--making");
    if making {
        // Tells the build that someone is here to choose Save as App or Discard.
        let _ = std::fs::write(
            folder.with_extension("open"),
            std::process::id().to_string(),
        );
    }
    App {
        folder,
        name: if making { "New app".into() } else { name },
        id: arg("--id"),
        check: check_folder.and(arg("--report")).map(PathBuf::from),
        helper: std::env::var("OPENAS_HELPER").ok().map(PathBuf::from),
        manager: false,
        making,
        env: vec![],
    }
}

/// The Helper's socket: in the data folder, unless that path is too long
/// for a Unix socket (about 104 bytes on a Mac); then a short one in the
/// temp folder, named after the data folder so setups never share one.
pub(crate) fn socket_for(root: &std::path::Path) -> PathBuf {
    let p = root.join("helper.sock");
    if p.as_os_str().len() < 100 {
        return p;
    }
    let mut h: u64 = 0xcbf29ce484222325;
    for b in root.to_string_lossy().bytes() {
        h = (h ^ b as u64).wrapping_mul(0x100000001b3);
    }
    std::env::temp_dir().join(format!("openas-{h:016x}.sock"))
}

pub(crate) fn root(me: &App) -> PathBuf {
    me.env
        .iter()
        .find(|(k, _)| k == "OPENAS_ROOT")
        .map(|(_, v)| PathBuf::from(v))
        .or_else(|| std::env::var("OPENAS_ROOT").ok().map(PathBuf::from))
        .unwrap_or_else(|| dirs::data_dir().unwrap_or_default().join("OpenAs"))
}

pub(crate) fn load_store(folder: &Path, persist: bool) -> Store {
    let data = folder.join("data");
    if persist {
        let _ = std::fs::create_dir_all(&data);
    }
    let file = data.join("store.json");
    let map = std::fs::read(&file)
        .ok()
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default();
    Store {
        file,
        map: Mutex::new(map),
        persist,
    }
}
