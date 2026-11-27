  const send = (k, a) => { try { const m = k + ": " + [...a].map(x => x && x.stack ? x.stack : String(x)).join(" "); if (window.__oaLog) window.__oaLog(m); else webkit.messageHandlers.log.postMessage(m); } catch (e) {} };
  const ce = console.error; console.error = (...a) => { send("console.error", a); ce(...a); };
  addEventListener("error", e => send("uncaught", [e.message, (e.filename || "").split("/").pop() + ":" + e.lineno]));
  addEventListener("unhandledrejection", e => send("rejection", [e.reason]));
  addEventListener("securitypolicyviolation", e => send("blocked", [(e.blockedURI || "something") + " isn't allowed: an app loads files from its own folder, and uses the internet (https and wss) only after app.permissions.request(['internet']) says yes. Ask for it when the app starts, then fetch."]));
  window.alert = m => send("alert() called", [m]); window.confirm = m => { send("confirm() called", [m]); return true; }; window.prompt = m => { send("prompt() called", [m]); return null; };
  if (!window.app) { const mem = __OA_SEED__;
    // Desktop powers, simulated: a sample folder in memory, a person who says yes
    // (unless the test calls desk.deny()), a recorded menu bar. Nothing real is touched.
    const D = window.desk = { grants: __OA_GRANTS__, deny: false, menuBar: null, notes: [], log: [], window: "shown", onTop: false,
      menu: id => fire("menu-bar", id), shortcut: t => fire("shortcut", t), shortcuts: [], runs: [], programs: {}, clickMenuBar: () => fire("menu-bar-click", null) };
    const H = {}; const fire = (ev, d) => (H[ev] || []).forEach(f => f(d));
    const day = 86400000, now = Date.now();
    const svg = c => "data:image/svg+xml;utf8," + encodeURIComponent("<svg xmlns='http://www.w3.org/2000/svg' width='640' height='420'><rect width='640' height='420' fill='" + c + "'/><circle cx='460' cy='140' r='70' fill='#fff8'/><path d='M0 420 L220 200 L380 330 L470 260 L640 420Z' fill='#0004'/></svg>");
    const F = new Map(); const add = (path, size, age, extra) => F.set(path, Object.assign({ dir: false, size, modified: now - age * day, created: now - age * day - 3600000 }, extra || {}));
    add("Old stuff", 0, 40, { dir: true });
    add("IMG_2041.jpg", 3412000, 2, { data: svg("#6a8caf") }); add("IMG_2042.jpg", 2980000, 2, { data: svg("#c08457") });
    add("Screenshot 2026-11-20 at 14.02.11.png", 812000, 5, { data: svg("#9aa5b1") }); add("Screenshot 2026-11-24 at 09.15.40.png", 640000, 1, { data: svg("#7c8a6e") });
    add("Invoice-1043.pdf", 88000, 12); add("Q3 report final (2).pdf", 2400000, 30); add("boarding-pass.pdf", 120000, 60);
    add("notes.txt", 1200, 3, { text: "Call the plumber\nBook train for Friday\nReturn library books" }); add("meeting-notes.md", 3400, 8, { text: "# Kickoff\n- scope\n- dates" });
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
      permissions: { has: async n => !!D.grants[n], request: async (names) => { const o = {}; for (const n of [].concat(names)) { const ok = n === "internet" ? !D.deny && !!D.grants.internet : !D.deny && ["notifications", "menu-bar", "overlay", "folder", "shortcuts", "full"].includes(n); if (ok) D.grants[n] = true; o[n] = ok; } if (o.folder) o.folderName = "Downloads"; return o; } },
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
      system: { run: async (program, args, opts) => { need("full"); args = [].concat(args || []).map(String); D.runs.push([program].concat(args).join(" ")); const k = String(program).split("/").pop(); const r = D.programs[k]; if (r === undefined) return { code: 0, stdout: "(simulated output of " + k + ")\n", stderr: "", timedOut: false }; return typeof r === "function" ? r(args, opts) : Object.assign({ code: 0, stdout: "", stderr: "", timedOut: false }, typeof r === "string" ? { stdout: r } : r); } },
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
