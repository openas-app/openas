# OpenAs

[![CI](https://github.com/openas-app/openas/actions/workflows/ci.yml/badge.svg)](https://github.com/openas-app/openas/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-black.svg)](LICENSE)

**Describe an app. A model builds it, and it installs as a real app (for mac as of now).**

It gets its own window, name, icon and Dock entry. Change it later by asking,
from inside the app, and go back to any earlier version.

https://github.com/user-attachments/assets/7f2648a0-6804-4af4-a15f-95274510b13d

Bring your own model: Claude, an open model through a provider (OpenRouter,
Hugging Face, Together, Groq, Fireworks), or one running on your Mac (Ollama,
LM Studio, vLLM). No account, no server, no subscription. You see what an app
will cost before anything is spent.

Status: early. macOS only for now.

## Install

You need Rust ([rustup.rs](https://rustup.rs)) and Apple's command line tools
(`xcode-select --install`). Then:

    git clone https://github.com/openas-app/openas && cd openas && ./install.sh

The first build takes a few minutes. OpenAs then opens from `~/Applications`.
It keeps using the folder you cloned, so leave that folder where it is (and
don't `cargo clean` it); after moving it, run `./install.sh` again.

## First app

1. **Describe it.** "A screen recorder that saves to Downloads", "flashcards to learn
   Spanish".
2. **Choose who builds it** (the first time only). Pick Claude, an open model or a
   model on this Mac, paste a key if it needs one, and see the price of an app with
   that model. OpenAs tries the key with one tiny request before saving it in your
   Keychain.
3. **Watch it being made.** A window opens with the app on the left and the build on
   the right: the plan, each part, and the model trying its own work. You can add
   notes while it builds, or stop it. At the end, try it and choose **Save as App**
   or **Discard**.
4. **Use it.** It's in your Applications folder like any other app.
5. **Change it.** Open the column on the right of the app and ask for a change. The
   last 20 versions are kept; go back to any of them. Going back changes the app's
   code, not what it has saved, and a change never rewrites your app's data.

Building stops at two limits you set under **Spending** (click the model name
under the text box): $50 a month and $10 per app by default. They're there so a
build that goes wrong can't keep spending. Models on your Mac have no limit.

## What apps may do

There are three levels (details and limits in [SECURITY.md](SECURITY.md)):

- **Default: what a web page can do.** The internet over https, the camera,
  microphone and screen (macOS asks you first), files you pick or drop in. An app's
  files load from its own folder and scripts from other sites are blocked, but like
  any web page online it can still download code and run it. For an app you don't
  trust, turn off **Allow Internet Access** in its menu.
- **App permissions, asked when the app needs them:** notifications, a menu bar item,
  staying on top, global keyboard shortcuts, and one folder you choose.
- **Full access to your Mac:** running programs and reading or changing any file,
  which includes reading your provider key from the Keychain. Asked in its own
  warning, and only when an app really needs it.

Permissions belong to a version of the app: after any change, it asks again, also
while you try a change before saving it.

**What your model sees.** Your description, the app's code, and, when you change
an app, screenshots and test results of it running with its saved data. All of it
goes only to the model provider you chose.

## How it works

    describe ──► the builder (a model, in steps) ──► a draft folder ──► Save as App
                    plan · write files · check            │
                    in a simulated desktop                 ▼
                                               ~/Applications/<Name>.app
                                               (a renamed copy of the Runner)

Every app is the same small window program, the Runner, showing its own folder.
Changing an app runs the builder again on a copy, checks it, and swaps it in as a
new version; the app's saved data is never touched.

## Sharing

**Share…** in an app's menu writes `<App>.openas` (its code and icon, never its
data). Opening that file in OpenAs shows the app and its code before you save it.
A shared app starts with no permissions and can never get full access.

## The command line

    target/release/openas new "A tracker for my plants and when to water them"
    target/release/openas change "Plant Watering" "Add a 'water all' button"
    target/release/openas undo "Plant Watering"
    target/release/openas list
    target/release/openas reinstall      # after rebuilding: update every app's Runner

## How it's built

- `runner/`: the one window type (Tauri 2, the system's WebKit). It serves an app's
  folder (`serve.rs`), gives its page `window.app` (`api.rs`), enforces what it may
  do (`powers/`), and holds the making and change column (`talk.rs`). Every
  installed app is a renamed copy of it.
- `core/`: providers and keys (Keychain), the builder in `agent/` (the loop in
  `turn.rs`, its tools in `tools.rs`, spending in `spend.rs`), installing, versions
  and shared files.
- `cli/`: the `openas` command and the Helper, a local socket that apps talk to
  for changes. It only answers OpenAs's own window and apps whose code it installed
  (`cli/tests/helper_refuses.rs` checks that).
- `tools/snap.swift`: a simulated desktop where the builder checks apps (screenshots,
  clicks, a fake camera, a sample folder) without touching anything real.

An app is a plain folder in `~/Library/Application Support/OpenAs/apps/`:

    app/            the app's code (HTML, CSS, JavaScript)
    data/           what the app saves; changes never rewrite it
    history/<n>/    the last 20 versions
    icon.svg  app.json  grants.json  version.json

## Testing without touching your apps

`OPENAS_ROOT`, `OPENAS_INSTALL_DIR` and `OPENAS_ID_PREFIX` give a separate setup;
installed copies carry them in their bundle.

## Uninstall

1. Delete `OpenAs.app` and the apps it made from `~/Applications` (each made app
   has `OAAppFolder` in its `Info.plist`).
2. Delete `~/Library/Application Support/OpenAs` (the apps' code, data and
   versions, and the spending record).
3. In Keychain Access, delete the items named `OpenAs` (your provider keys).
4. Delete the folder you cloned.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). Security problems: [SECURITY.md](SECURITY.md).

## License

MIT. See [LICENSE](LICENSE).
