// The builder's test pointer, replayed over the app while it is made.
// The Runner fills in __OPENAS_GHOST__ with the recorded moves.
(() => {
  const g = __OPENAS_GHOST__;
  const old = document.getElementById("__oa_ghost"); if (old) old.remove();
  const sx = innerWidth / (+g.w || innerWidth), sy = innerHeight / (+g.h || innerHeight);
  const box = document.createElement("div"); box.id = "__oa_ghost";
  box.style.cssText = "position:fixed;inset:0;pointer-events:none;z-index:2147483647";
  const cv = document.createElement("canvas"); cv.width = innerWidth * devicePixelRatio; cv.height = innerHeight * devicePixelRatio;
  cv.style.cssText = "position:absolute;inset:0;width:100%;height:100%"; box.append(cv);
  const c = cv.getContext("2d"); c.scale(devicePixelRatio, devicePixelRatio);
  c.strokeStyle = "rgba(255,255,255,.85)"; c.lineWidth = 2; c.setLineDash([1, 7]); c.lineCap = "round";
  c.shadowColor = "rgba(0,0,0,.35)"; c.shadowBlur = 2;
  const cur = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  cur.setAttribute("viewBox", "0 0 22 22"); cur.style.cssText = "position:absolute;width:22px;height:22px;left:0;top:0;transition:transform .05s linear;filter:drop-shadow(0 2px 3px rgba(0,0,0,.35))";
  cur.innerHTML = '<path d="M4 2.5v15l4-3.6 2.6 6 2.6-1.1-2.6-5.9h5.4z" fill="#fff" stroke="#1d1d1f" stroke-width="1.3" stroke-linejoin="round"/>';
  box.append(cur);
  if (g.caption) {
    const cap = document.createElement("div"); cap.textContent = g.caption;
    cap.style.cssText = "position:absolute;left:50%;top:18px;transform:translateX(-50%);padding:8px 14px;border-radius:999px;background:rgba(28,28,27,.82);color:#f4f3ef;font:12.5px -apple-system,system-ui;backdrop-filter:blur(12px);-webkit-backdrop-filter:blur(12px);box-shadow:0 4px 18px rgba(0,0,0,.18);white-space:nowrap";
    box.append(cap);
  }
  document.documentElement.append(box);
  const m = (g.moves || []).slice(0, 600); let i = 0, last = null; const t0 = performance.now();
  const step = () => {
    const now = performance.now() - t0;
    while (i < m.length && m[i].t <= now) {
      const p = m[i], x = p.x * sx, y = p.y * sy;
      cur.style.transform = "translate(" + (x - 4) + "px," + (y - 2) + "px)";
      if (p.k === "drag" && last) { c.beginPath(); c.moveTo(last.x, last.y); c.lineTo(x, y); c.stroke(); }
      if (p.k === "down" || p.k === "up") { c.save(); c.setLineDash([]); c.fillStyle = "rgba(255,255,255,.5)"; c.beginPath(); c.arc(x, y, p.k === "down" ? 9 : 5, 0, 7); c.fill(); c.restore(); }
      last = { x, y }; i++;
    }
    if (i < m.length) requestAnimationFrame(step); else setTimeout(() => { box.style.transition = "opacity .6s"; box.style.opacity = "0"; setTimeout(() => box.remove(), 700); }, 1400);
  };
  requestAnimationFrame(step);
})();
