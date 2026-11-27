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
    /// A non-default setup carried in the copy, passed on to the Helper.
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

pub(crate) fn which_app() -> App {
    // An installed copy: its own identity (Info.plist, or openas-app.json
    // beside the program on Windows).
    if let Some(who) = openas_platform::own_identity() {
        return App {
            folder: who.folder,
            name: who.name,
            id: Some(who.id).filter(|i| !i.is_empty() && i != RUNNER_ID),
            check: None,
            helper: who.helper,
            manager: who.manager,
            making: false,
            env: who.env,
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
