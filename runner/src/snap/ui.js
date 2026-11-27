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
