// The builder's eyes: open an app folder in an off-screen WKWebView, let it
// settle, optionally run a test script (clicks, typing), then report console
// errors and the script's result as JSON on stdout and save a screenshot.
//   snap APP_DIR OUT.png [WIDTH HEIGHT] [SCRIPT_FILE] [dark|light]
// The window is placed far off screen and never activated: nothing appears
// on the person's screen and focus never moves.
import Cocoa
import WebKit

let a = CommandLine.arguments
let root = URL(fileURLWithPath: a[1]), out = a[2]
let w = a.count > 3 ? Double(a[3]) ?? 1200 : 1200, h = a.count > 4 ? Double(a[4]) ?? 800 : 800
let script = a.count > 5 && !a[5].isEmpty ? (try? String(contentsOfFile: a[5], encoding: .utf8)) : nil
let dark = a.count > 6 && a[6] == "dark"

// The app's own files and nothing else: a path that leaves the folder (`..`,
// encoded or not, or a link pointing out) is refused, like the real runtime.
final class Files: NSObject, WKURLSchemeHandler {
    let root: URL
    let base: String
    let csp: String
    init(_ r: URL, csp: String) {
        root = r.standardizedFileURL.resolvingSymlinksInPath()
        base = root.path.hasSuffix("/") ? root.path : root.path + "/"
        self.csp = csp
    }
    func webView(_ w: WKWebView, start t: WKURLSchemeTask) {
        let raw = URLComponents(url: t.request.url!, resolvingAgainstBaseURL: false)?.percentEncodedPath ?? ""
        var path = raw.removingPercentEncoding ?? ""
        if path == "/" || path.isEmpty { path = "/index.html" }
        let f = root.appendingPathComponent(path).standardizedFileURL.resolvingSymlinksInPath()
        guard !path.split(separator: "/").contains(".."), f.path.hasPrefix(base),
              let d = try? Data(contentsOf: f) else { t.didFailWithError(URLError(.fileDoesNotExist)); return }
        let types = ["html": "text/html", "js": "text/javascript", "mjs": "text/javascript", "css": "text/css",
                     "svg": "image/svg+xml", "png": "image/png", "jpg": "image/jpeg", "json": "application/json",
                     "woff2": "font/woff2", "woff": "font/woff", "wasm": "application/wasm"]
        let type = types[f.pathExtension.lowercased()] ?? "application/octet-stream"
        t.didReceive(HTTPURLResponse(url: t.request.url!, statusCode: 200, httpVersion: nil, headerFields: ["Content-Type": type, "Content-Security-Policy": csp])!)
        t.didReceive(d); t.didFinish()
    }
    func webView(_ w: WKWebView, stop t: WKURLSchemeTask) {}
}

var errors: [String] = []
final class Log: NSObject, WKScriptMessageHandler {
    func userContentController(_ c: WKUserContentController, didReceive m: WKScriptMessage) {
        if errors.count < 30 { errors.append(String(describing: m.body)) }
    }
}

// Saved data, if any, seeds the stub store so the app shows real state.
var seed = "{}"
if let d = try? String(contentsOf: root.deletingLastPathComponent().appendingPathComponent("data/store.json"), encoding: .utf8) { seed = d }

let app = NSApplication.shared
app.setActivationPolicy(.accessory)
app.appearance = NSAppearance(named: dark ? .darkAqua : .aqua)
let cfg = WKWebViewConfiguration()
// The same content policy as the real runtime (runner/src/serve.rs `policy`),
// including the app's Internet switch.
let network = (try? JSONSerialization.jsonObject(with: Data(contentsOf: root.deletingLastPathComponent().appendingPathComponent("permissions.json")))
    as? [String: Any])?["network"] as? Bool ?? true
let csp = network
    ? "default-src 'self' app: data: blob: https:; script-src 'self' app: 'unsafe-inline' 'unsafe-eval'; frame-src 'self' app: data: blob:; style-src 'self' app: 'unsafe-inline' https:; img-src 'self' app: data: blob: https:; font-src 'self' app: data: https:; connect-src 'self' https: wss:; media-src 'self' app: data: blob: https:; base-uri 'none'"
    : "default-src 'self' app: data: blob:; script-src 'self' app: 'unsafe-inline' 'unsafe-eval'; style-src 'self' app: 'unsafe-inline'; img-src 'self' app: data: blob:; font-src 'self' app: data:; connect-src 'self'; media-src 'self' app: data: blob:; form-action 'none'; base-uri 'none'"
let files = Files(root, csp: csp)
cfg.setURLSchemeHandler(files, forURLScheme: "app")
let log = Log()
cfg.userContentController.add(log, name: "log")
cfg.userContentController.addUserScript(WKUserScript(source: """
  const send = (k, a) => { try { webkit.messageHandlers.log.postMessage(k + ": " + [...a].map(x => x && x.stack ? x.stack : String(x)).join(" ")); } catch (e) {} };
  const ce = console.error; console.error = (...a) => { send("console.error", a); ce(...a); };
  addEventListener("error", e => send("uncaught", [e.message, (e.filename || "").split("/").pop() + ":" + e.lineno]));
  addEventListener("unhandledrejection", e => send("rejection", [e.reason]));
  window.alert = m => send("alert() called", [m]); window.confirm = m => { send("confirm() called", [m]); return true; }; window.prompt = m => { send("prompt() called", [m]); return null; };
  if (!window.app) { const mem = \(seed);
    // Desktop powers, simulated: a sample folder in memory, a person who says yes
    // (unless the test calls desk.deny()), a recorded menu bar. Nothing real is touched.
    const D = window.desk = { grants: {}, deny: false, menuBar: null, notes: [], log: [], window: "shown", onTop: false,
      menu: id => fire("menu-bar", id), shortcut: t => fire("shortcut", t), shortcuts: [], runs: [], programs: {}, clickMenuBar: () => fire("menu-bar-click", null) };
    const H = {}; const fire = (ev, d) => (H[ev] || []).forEach(f => f(d));
    const day = 86400000, now = Date.now();
    const svg = c => "data:image/svg+xml;utf8," + encodeURIComponent("<svg xmlns='http://www.w3.org/2000/svg' width='640' height='420'><rect width='640' height='420' fill='" + c + "'/><circle cx='460' cy='140' r='70' fill='#fff8'/><path d='M0 420 L220 200 L380 330 L470 260 L640 420Z' fill='#0004'/></svg>");
    const F = new Map(); const add = (path, size, age, extra) => F.set(path, Object.assign({ dir: false, size, modified: now - age * day, created: now - age * day - 3600000 }, extra || {}));
    add("Old stuff", 0, 40, { dir: true });
    add("IMG_2041.jpg", 3412000, 2, { data: svg("#6a8caf") }); add("IMG_2042.jpg", 2980000, 2, { data: svg("#c08457") });
    add("Screenshot 2026-11-20 at 14.02.11.png", 812000, 5, { data: svg("#9aa5b1") }); add("Screenshot 2026-11-24 at 09.15.40.png", 640000, 1, { data: svg("#7c8a6e") });
    add("Invoice-1043.pdf", 88000, 12); add("Q3 report final (2).pdf", 2400000, 30); add("boarding-pass.pdf", 120000, 60);
    add("notes.txt", 1200, 3, { text: "Call the plumber\\nBook train for Friday\\nReturn library books" }); add("meeting-notes.md", 3400, 8, { text: "# Kickoff\\n- scope\\n- dates" });
    add("song.mp3", 5100000, 90); add("archive.zip", 18000000, 120); add("Setup-2.4.dmg", 96000000, 45); add("budget.xlsx", 45000, 20);
    add("Old stuff/letter.docx", 34000, 200); add("Old stuff/photo.heic", 2100000, 400, { data: svg("#a07ab0") });
    const norm = p => { p = String(p || "").replace(/^[.][/]/, "").replace(/[/]+$/, "");
      // With full access, the sample folder stands in at ~/Downloads (and /Users/you/Downloads).
      if (/^(~|[/])/.test(p)) { if (!D.grants.full) throw new Error("not-allowed: full paths need full access; call app.permissions.request(['full']) from a click"); const m = p.match(/^(?:~|[/]Users[/][^/]+)[/]Downloads(?:[/](.*))?$/); if (!m) throw new Error("“" + p + "” doesn't exist (in checks, only ~/Downloads has sample files)"); p = m[1] || ""; }
      if (p.split("/").includes("..")) throw new Error("“" + p + "” is outside the folder"); return p; };
    const ent = p => { const f = F.get(p); const name = p.split("/").pop(); return { name, path: p, dir: f.dir, size: f.size, modified: f.modified, created: f.created, ext: f.dir || !name.includes(".") ? "" : name.split(".").pop().toLowerCase() }; };
    const need = n => { if (!D.grants[n]) throw new Error((n === "folder" ? "no-folder" : "not-allowed") + ": call app.permissions.request(['" + n + "']) from a click first"); };
    const must = p => { p = norm(p); if (!F.has(p)) throw new Error("“" + p + "” doesn't exist"); return p; };
    const parentOk = p => { const i = p.lastIndexOf("/"); if (i > 0 && !(F.get(p.slice(0, i)) || {}).dir) throw new Error("the folder for “" + p + "” doesn't exist"); };
    const fop = (op, f) => async (...a) => { if (!(D.grants.full && a.some(x => typeof x === "string" && /^(~|[/])/.test(x)))) need("folder"); const r = f(...a); D.log.push(op + " " + a.filter(x => typeof x === "string").map(x => x.length > 60 ? x.slice(0, 60) + "…" : x).join(" → ")); return r; };
    window.app = {
      store: { get: async k => (k in mem ? structuredClone(mem[k]) : null), set: async (k, v) => { mem[k] = structuredClone(v); return true; } },
      notify: async t => { need("notifications"); D.notes.push(String(t)); return true; },
      permissions: { has: async n => !!D.grants[n], request: async (names) => { const o = {}; for (const n of [].concat(names)) { const ok = !D.deny && ["notifications", "menu-bar", "overlay", "folder", "shortcuts", "full"].includes(n); if (ok) D.grants[n] = true; o[n] = ok; } if (o.folder) o.folderName = "Downloads"; return o; } },
      files: {
        folder: fop("folder", () => ({ name: "Downloads" })),
        list: fop("list", (p = "") => { p = norm(p); if (p && !(F.get(p) || {}).dir) throw new Error("“" + p + "” isn't a folder"); return [...F.keys()].filter(k => k !== p && (p ? k.startsWith(p + "/") : true) && !k.slice(p ? p.length + 1 : 0).includes("/")).map(ent); }),
        stat: fop("stat", p => ent(must(p))),
        readText: fop("readText", p => { const f = F.get(must(p)); if (f.dir) throw new Error("that's a folder"); return f.text != null ? f.text : "(binary)"; }),
        readDataURL: fop("readDataURL", p => { const f = F.get(must(p)); return f.data || "data:application/octet-stream;base64,AAAA"; }),
        writeText: fop("writeText", (p, t) => { p = norm(p); parentOk(p); F.set(p, { dir: false, size: String(t).length, modified: Date.now(), created: Date.now(), text: String(t) }); return true; }),
        mkdir: fop("mkdir", p => { p = norm(p); let c = ""; for (const s of p.split("/")) { c = c ? c + "/" + s : s; if (!F.has(c)) F.set(c, { dir: true, size: 0, modified: Date.now(), created: Date.now() }); } return true; }),
        move: fop("move", (a, b) => { a = must(a); b = norm(b); if (F.has(b)) throw new Error("“" + b + "” already exists"); parentOk(b); for (const k of [...F.keys()]) if (k === a || k.startsWith(a + "/")) { const v = F.get(k); F.delete(k); F.set(b + k.slice(a.length), v); } return true; }),
        copy: fop("copy", (a, b) => { a = must(a); b = norm(b); if (F.has(b)) throw new Error("“" + b + "” already exists"); if (F.get(a).dir) throw new Error("only files can be copied"); parentOk(b); F.set(b, Object.assign({}, F.get(a))); return true; }),
        trash: fop("trash", p => { p = must(p); for (const k of [...F.keys()]) if (k === p || k.startsWith(p + "/")) F.delete(k); return true; }),
        reveal: fop("reveal", p => { must(p); return true; }),
      },
      system: { run: async (program, args, opts) => { need("full"); args = [].concat(args || []).map(String); D.runs.push([program].concat(args).join(" ")); const k = String(program).split("/").pop(); const r = D.programs[k]; if (r === undefined) return { code: 0, stdout: "(simulated output of " + k + ")\\n", stderr: "", timedOut: false }; return typeof r === "function" ? r(args, opts) : Object.assign({ code: 0, stdout: "", stderr: "", timedOut: false }, typeof r === "string" ? { stdout: r } : r); } },
      shortcuts: { set: async list => { need("shortcuts"); list = [].concat(list || []); if (list.length > 8) throw new Error("too-many: an app can have at most 8 shortcuts"); const bad = list.find(a => { const m = a.split("+").slice(0, -1).map(x => x.trim().toLowerCase()); return m.length < 2 || !m.some(x => ["cmd", "command", "super", "commandorcontrol", "cmdorctrl", "ctrl", "control", "alt", "option"].includes(x)); }); if (bad) throw new Error("bad-shortcut: “" + bad + "” needs two modifiers, one of them Command, Control or Alt"); D.shortcuts = list; return { active: list, failed: [] }; }, clear: async () => { D.shortcuts = []; return true; } },
      menuBar: { set: async spec => { need("menu-bar"); D.menuBar = JSON.parse(JSON.stringify(spec)); return true; }, clear: async () => { D.menuBar = null; return true; } },
      window: { show: async () => { D.window = "shown"; return true; }, hide: async () => { D.window = "hidden"; return true; }, setAlwaysOnTop: async on => { need("overlay"); D.onTop = !!on; return true; } },
      on: (ev, f) => { (H[ev] = H[ev] || []).push(f); },
    };
    // Camera, microphone and screen, as a browser offers them: a moving test
    // picture and a quiet tone stand in, so a recorder can really record.
    // desk.deny() makes the person say no (NotAllowedError), like the real ask.
    D.media = [];
    const fakeStream = (video, audio, label) => {
      const tracks = [];
      if (video) {
        const cv = document.createElement("canvas"); cv.width = 1280; cv.height = 800; const g = cv.getContext("2d"); let t = 0;
        const draw = () => { t++; g.fillStyle = "#2b3a4a"; g.fillRect(0, 0, 1280, 800); g.fillStyle = "#e9e4d8"; g.fillRect(80 + (t * 6) % 1000, 300, 160, 160); g.font = "48px -apple-system"; g.fillText(label + " " + new Date().toLocaleTimeString(), 80, 120); };
        draw(); setInterval(draw, 50); tracks.push(...cv.captureStream(20).getVideoTracks());
      }
      if (audio) { try { const ac = new AudioContext(); const o = ac.createOscillator(); const gn = ac.createGain(); gn.gain.value = 0.05; const d = ac.createMediaStreamDestination(); o.connect(gn).connect(d); o.start(); tracks.push(...d.stream.getAudioTracks()); } catch (e) {} }
      return new MediaStream(tracks);
    };
    const ask = (kind, c) => new Promise((res, rej) => setTimeout(() => { D.media.push(kind); if (D.deny) { const e = new Error("The person didn't allow " + kind); e.name = "NotAllowedError"; rej(e); } else res(fakeStream(kind === "screen" ? true : !!(c && c.video), !!(c && c.audio), kind === "screen" ? "Screen" : "Camera")); }, 300));
    Object.defineProperty(navigator, "mediaDevices", { configurable: true, value: {
      getDisplayMedia: c => ask("screen", c), getUserMedia: c => ask(c && c.video ? "camera" : "microphone", c),
      enumerateDevices: async () => [{ kind: "videoinput", label: "FaceTime HD Camera", deviceId: "cam" }, { kind: "audioinput", label: "MacBook Pro Microphone", deviceId: "mic" }],
      addEventListener() {}, removeEventListener() {} } });
    }
""", injectionTime: .atDocumentStart, forMainFrameOnly: false))

let uiScript = """
(() => {
  const W = innerWidth, H = innerHeight;
  // A virtual clock for the replay: the off-screen window throttles timers, so
  // the test runs as fast as it can and the moves carry the pace a person would see.
  let clock = 0;
  const moves = window.__moves = [];
  let cur = { x: Math.round(W * 0.6), y: Math.round(H * 0.55) }, captured = null, down = false;
  const rec = (k, x, y, extra) => { if (moves.length < 600) moves.push(Object.assign({ k, x: Math.round(x), y: Math.round(y), t: clock }, extra || {})); };
  const tick = () => new Promise(r => queueMicrotask(r));
  const sleep = ms => { clock += ms; return ms > 200 ? new Promise(r => setTimeout(r, Math.min(ms, 1500))) : tick(); };
  const P = Element.prototype, set = P.setPointerCapture, rel = P.releasePointerCapture;
  P.setPointerCapture = function (id) { captured = this; try { set.call(this, id); } catch (e) {} };
  P.releasePointerCapture = function (id) { if (captured === this) captured = null; try { rel.call(this, id); } catch (e) {} };
  P.hasPointerCapture = function () { return captured === this; };
  const at = (x, y) => captured || document.elementFromPoint(x, y) || document.body;
  const fire = (type, x, y, extra) => {
    const el = at(x, y), o = Object.assign({ bubbles: true, cancelable: true, composed: true, clientX: x, clientY: y, screenX: x, screenY: y,
      button: 0, buttons: down ? 1 : 0, pointerId: 1, pointerType: "mouse", isPrimary: true, view: window }, extra || {});
    const E = type.startsWith("pointer") ? PointerEvent : MouseEvent;
    el.dispatchEvent(new E(type, o));
    return el;
  };
  const where = t => {
    if (typeof t === "string") {
      const el = document.querySelector(t);
      if (!el) throw new Error("ui: nothing matches " + t);
      el.scrollIntoView({ block: "nearest", inline: "nearest" });
      const r = el.getBoundingClientRect();
      return { x: r.left + r.width / 2, y: r.top + r.height / 2 };
    }
    if (t && typeof t.x === "number") return { x: t.x, y: t.y };
    throw new Error("ui: a target is a CSS selector or {x, y}");
  };
  const glide = async to => {
    const from = { ...cur }, n = 8;
    for (let i = 1; i <= n; i++) {
      const x = from.x + (to.x - from.x) * i / n, y = from.y + (to.y - from.y) * i / n;
      clock += 28; fire("pointermove", x, y); fire("mousemove", x, y); rec(down ? "drag" : "move", x, y);
      await tick();
    }
    cur = { x: to.x, y: to.y };
  };
  const press = (x, y) => { down = true; fire("pointerdown", x, y, { buttons: 1 }); const el = fire("mousedown", x, y, { buttons: 1 }); rec("down", x, y); if (el && el.focus && el.matches && el.matches("input,textarea,select,button,[tabindex],[contenteditable]")) el.focus(); };
  const lift = (x, y) => { down = false; fire("pointerup", x, y); fire("mouseup", x, y); rec("up", x, y); captured = null; };
  const keyOn = (key, m) => {
    const el = document.activeElement || document.body, o = { key, code: key.length === 1 ? "Key" + key.toUpperCase() : key, bubbles: true, cancelable: true,
      metaKey: !!m.meta, shiftKey: !!m.shift, altKey: !!m.alt, ctrlKey: !!m.ctrl };
    el.dispatchEvent(new KeyboardEvent("keydown", o)); el.dispatchEvent(new KeyboardEvent("keyup", o));
  };
  window.ui = Object.freeze({
    click: async t => { const p = where(t); await glide(p); press(p.x, p.y); await sleep(30); lift(p.x, p.y); fire("click", p.x, p.y); await sleep(120); },
    drag: async (t, path) => {
      const p = where(t); await glide(p); press(p.x, p.y); let last = { ...p };
      for (const [dx, dy] of path || []) {
        const to = { x: p.x + dx, y: p.y + dy };
        for (let i = 1; i <= 6; i++) { const x = last.x + (to.x - last.x) * i / 6, y = last.y + (to.y - last.y) * i / 6; clock += 22; fire("pointermove", x, y, { buttons: 1 }); fire("mousemove", x, y, { buttons: 1 }); rec("drag", x, y); await tick(); }
        last = to;
      }
      cur = last; lift(last.x, last.y); await sleep(120);
    },
    type: async (t, text) => {
      await window.ui.click(t); const el = document.activeElement; rec("type", cur.x, cur.y, { text: String(text).slice(0, 40) });
      for (const ch of String(text)) {
        el.dispatchEvent(new KeyboardEvent("keydown", { key: ch, bubbles: true, cancelable: true }));
        if ("value" in el) { el.value += ch; el.dispatchEvent(new InputEvent("input", { bubbles: true, data: ch, inputType: "insertText" })); }
        else if (el.isContentEditable) { document.execCommand("insertText", false, ch); }
        el.dispatchEvent(new KeyboardEvent("keyup", { key: ch, bubbles: true }));
        await sleep(8);
      }
      el.dispatchEvent(new Event("change", { bubbles: true })); await sleep(100);
    },
    key: async (key, mods) => { const m = mods || {}; keyOn(key, m); rec("key", cur.x, cur.y, { key: (m.meta ? "⌘" : "") + (m.shift ? "⇧" : "") + (m.alt ? "⌥" : "") + key }); await sleep(120); },
    wait: ms => { clock += ms; return new Promise(r => setTimeout(r, Math.min(ms, 3000))); },
  });
})();
"""
cfg.userContentController.addUserScript(WKUserScript(source: uiScript, injectionTime: .atDocumentEnd, forMainFrameOnly: true))
let web = WKWebView(frame: NSRect(x: 0, y: 0, width: w, height: h), configuration: cfg)
web.appearance = NSAppearance(named: dark ? .darkAqua : .aqua)
let win = NSWindow(contentRect: web.frame, styleMask: [.borderless], backing: .buffered, defer: false)
win.contentView = web
win.appearance = web.appearance
win.setFrameOrigin(NSPoint(x: -30000, y: -30000))
win.orderFrontRegardless()
web.load(URLRequest(url: URL(string: "app://local/index.html")!))

var moves: Any = []
var desk: Any = NSNull()
func finish(_ result: String) {
    web.takeSnapshot(with: nil) { img, _ in
        if let img, let tiff = img.tiffRepresentation, let rep = NSBitmapImageRep(data: tiff) {
            try? rep.representation(using: .png, properties: [:])?.write(to: URL(fileURLWithPath: out))
        }
        let o: [String: Any] = ["errors": errors, "result": result, "moves": moves, "desktop": desk]
        if let d = try? JSONSerialization.data(withJSONObject: o), let s = String(data: d, encoding: .utf8) { print(s) }
        exit(0)
    }
}

DispatchQueue.main.asyncAfter(deadline: .now() + 1.5) {
    guard let script else { finish(""); return }
    web.callAsyncJavaScript("const __r = await (async () => { \(script) \n})(); let __s; try { __s = typeof __r === 'string' ? __r : JSON.stringify(__r); } catch (e) { __s = String(__r); } const __d = window.desk ? { grants: Object.keys(desk.grants), menu_bar: desk.menuBar, notifications: desk.notes, file_calls: desk.log.slice(-30), window: desk.window, media_asks: desk.media, shortcuts: desk.shortcuts, programs_run: desk.runs.slice(-30) } : null; return JSON.stringify({ r: __s === undefined ? '' : __s, m: window.__moves || [], d: __d });", arguments: [:], in: nil, in: .page) { r in
        var s = ""
        switch r {
        case .success(let v):
            if let t = v as? String, let d = t.data(using: .utf8), let o = try? JSONSerialization.jsonObject(with: d) as? [String: Any] {
                s = (o["r"] as? String) ?? ""; moves = o["m"] ?? []; desk = o["d"] ?? NSNull()
            } else { s = String(describing: v) }
        case .failure(let e):
            s = "script error: \((e as NSError).userInfo["WKJavaScriptExceptionMessage"] ?? e.localizedDescription)"
            web.evaluateJavaScript("window.__moves || []") { v, _ in moves = v ?? [] }
        }
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.6) { finish(String(s.prefix(4000))) }
    }
}
DispatchQueue.main.asyncAfter(deadline: .now() + 40) { errors.append("timed out"); finish("") }
app.run()
