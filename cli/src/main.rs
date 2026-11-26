//! openas new "<what you want>"     make an app and install it
//! openas change <app> "<change>"   change an app (its data is kept)
//! openas undo <app>                put back the version before the last change
//! openas list                      your apps
//! openas reinstall [app]           copy the current Runner into your apps again
//! openas setup                     install the OpenAs window (list + New App)
//! openas serve                     the Helper (apps start it when needed)
//! openas mcp                       an MCP server on stdio, for any MCP client
use anyhow::{bail, Result};
use openas_core::{install, make, paths};
use serde_json::json;

mod helper;
mod mcp;

fn say(s: &str) {
    eprintln!("{s}");
}

fn report(o: &make::Outcome) {
    eprintln!("\n{}  ({})", o.info.name, o.folder.display());
    if let Some(p) = &o.info.installed {
        eprintln!("Installed: {}", p.display());
    }
    if !o.problems.is_empty() {
        eprintln!("Still broken after one fix:");
        for p in &o.problems {
            eprintln!("  · {p}");
        }
    }
    eprintln!(
        "Model: {} via {} · {}",
        o.info.model,
        o.info.provider,
        o.spend.describe()
    );
}

const MANAGER_ICON: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1024 1024"><rect width="1024" height="1024" fill="#0B0B0C"/><circle cx="422.5" cy="512" r="179" fill="none" stroke="#FFFFFF" stroke-width="128"/><rect x="716.5" y="627" width="128" height="128" fill="#FF4A1C"/></svg>"##;

/// Installs "OpenAs" itself: a Runner copy marked as the manager, recorded
/// so the Helper trusts exactly this copy to make apps.
fn setup() -> Result<()> {
    let folder = paths::root().join("manager");
    std::fs::create_dir_all(folder.join("app"))?;
    // A second copy (the proof of concept) takes its own name, so it never replaces the first.
    let name = std::env::var("OPENAS_MANAGER_NAME").unwrap_or_else(|_| "OpenAs".into());
    let app = install::install_as(&folder, "manager", &name, MANAGER_ICON, true)?;
    let record = json!({ "installed": app, "cdhash": install::cdhash(&app) });
    std::fs::write(
        paths::root().join("manager.json"),
        serde_json::to_string_pretty(&record)?,
    )?;
    eprintln!("Installed {}", app.display());
    // In the background, so it never takes over the screen.
    std::process::Command::new("open")
        .arg("-g")
        .arg(&app)
        .status()?;
    Ok(())
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["new", prompt] => {
            let o = make::new_app(prompt, &make::Events::lines(&say))?;
            report(&o);
            if let Some(p) = &o.info.installed {
                std::process::Command::new("open").arg(p).status()?;
            }
        }
        // The step-wise builder (proof of concept, Claude).
        ["agent-new", prompt] => {
            let (ws, f) = openas_core::agent::new_draft(prompt, &make::Events::lines(&say))?;
            println!("draft: {}\nname: {}\nsummary: {}", ws.display(), f.name, f.summary);
            if let Some(why) = &f.unfinished {
                println!("unfinished: {why}");
            }
        }
        ["agent-change", app, request] => {
            let (folder, f) = openas_core::agent::change(app, request, &make::Events::lines(&say))?;
            // From the command line, a change is kept straight away.
            openas_core::agent::save_change(app)?;
            println!("changed: {}\nsummary: {}", folder.display(), f.summary);
        }
        ["make-start", request] => {
            let ws = openas_core::agent::start_make(request, &std::env::current_exe()?, true)?;
            println!("{}", ws.display());
        }
        // Share an app as one file (code and icon, never data), or install one.
        ["share", app] => println!("{}", openas_core::share::export(&make::find(app)?, None)?.display()),
        ["import", file] => {
            let r = openas_core::share::open(std::path::Path::new(file))?;
            let info = openas_core::share::install(&r, &r.name.clone())?;
            println!("installed: {}", info.installed.map(|p| p.display().to_string()).unwrap_or_default());
        }
        ["make-run", draft] => openas_core::agent::make_run(std::path::Path::new(draft))?,
        ["keep", draft, name] => {
            let info = make::install_draft(std::path::Path::new(draft), name)?;
            println!("installed: {}", info.installed.map(|p| p.display().to_string()).unwrap_or_default());
        }
        // Development: an icon as it will look installed.
        ["icon-preview", svg, png] => {
            openas_core::install::render_png(&std::fs::read_to_string(svg)?, std::path::Path::new(png))?;
        }
        ["icon-preview", svg, png, px] => {
            openas_core::install::shaped(&std::fs::read_to_string(svg)?, px.parse()?)?.save_png(png)?;
        }
        // Development: what making one app would cost with this model.
        ["estimate", provider, model] => println!("{}", openas_core::agent::estimate(provider, model).map_or("unknown".into(), |v| v.to_string())),
        ["spent"] => {
            let (total, v) = openas_core::agent::spent();
            println!("total ${total:.2}\n{}", serde_json::to_string_pretty(&v["jobs"]).unwrap_or_default());
        }
        ["change", app, request] => {
            let o = make::change(app, request, &make::Events::lines(&say))?;
            report(&o);
            if let Some(w) = &o.warning {
                eprintln!("Note: {w}");
            }
            if o.problems.is_empty() {
                eprintln!("Changed. An open window reloads by itself.");
            } else {
                eprintln!("Kept the previous version.");
            }
        }
        ["undo", app] => {
            make::undo(app)?;
            eprintln!("Put back the previous version.");
        }
        ["list"] => {
            for i in make::list()? {
                println!("{:24} {:24} {}", i.name, i.id, i.model);
            }
        }
        ["reinstall"] => {
            for i in make::list()? {
                let i = make::reinstall(&i.id)?;
                eprintln!("Reinstalled {}", i.name);
            }
            if paths::root().join("manager.json").exists() {
                setup()?;
            }
        }
        ["reinstall", app] => {
            let i = make::reinstall(app)?;
            eprintln!("Reinstalled {}", i.name);
        }
        ["import-key"] => {
            // Moves keys from an old .env into the Keychain, without printing them.
            for (id, p) in openas_core::model::providers()? {
                if let Some(var) = &p.key_env {
                    if let Some(k) = openas_core::model::legacy_key(var) {
                        openas_core::keys::set(&id, &k)?;
                        eprintln!("Saved the {} key in the Keychain.", p.name);
                    }
                }
            }
        }
        ["setup"] => setup()?,
        ["serve"] => helper::serve()?,
        ["mcp"] => mcp::serve()?,
        _ => bail!(
            "usage:\n  openas new \"<what you want>\"\n  openas change <app> \"<change>\"\n  openas undo <app>\n  openas list\n  openas setup\n  openas serve\n  openas mcp"
        ),
    }
    Ok(())
}
