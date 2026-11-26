// The builder's test pointer, replayed over the app while it is made.
// The Runner fills in __OPENAS_GHOST__ with the recorded moves.
(() => {
  const g = __OPENAS_GHOST__;
  const old = document.getElementById("__oa_ghost"); if (old) old.remove();
  const dark = matchMedia("(prefers-color-scheme: dark)").matches, green = dark ? "#FFFFFF" : "#0B0B0C", rim = dark ? "#0B0B0C" : "#FFFFFF";
  const sx = innerWidth / (+g.w || innerWidth), sy = innerHeight / (+g.h || innerHeight);
  const box = document.createElement("div"); box.id = "__oa_ghost";
  box.style.cssText = "position:fixed;inset:0;pointer-events:none;z-index:2147483647";
  const cv = document.createElement("canvas"); cv.width = innerWidth * devicePixelRatio; cv.height = innerHeight * devicePixelRatio;
  cv.style.cssText = "position:absolute;inset:0;width:100%;height:100%"; box.append(cv);
  const c = cv.getContext("2d"); c.scale(devicePixelRatio, devicePixelRatio);
  const dot = document.createElement("div");
  dot.style.cssText = "position:absolute;left:0;top:0;width:14px;height:14px;margin:-7px 0 0 -7px;border-radius:50%;background:" + green + ";border:2px solid " + rim + ";box-shadow:0 1px 4px rgba(0,0,0,.35);transition:transform .05s linear";
  box.append(dot);
  if (g.caption) {
    const cap = document.createElement("div");
    cap.style.cssText = "position:absolute;left:50%;bottom:16px;transform:translateX(-50%);display:flex;align-items:center;gap:8px;max-width:calc(100% - 32px);padding:7px 13px 7px 9px;border-radius:999px;font:500 12.5px Inter,-apple-system,system-ui;white-space:nowrap;overflow:hidden;text-overflow:ellipsis;-webkit-backdrop-filter:blur(14px);backdrop-filter:blur(14px);box-shadow:0 4px 18px rgba(0,0,0,.16);"
      + (dark ? "background:rgba(244,244,245,.94);color:#0B0B0C" : "background:rgba(11,11,12,.9);color:#FFFFFF");
    const mark = document.createElement("span"); mark.style.cssText = "flex:none;width:7px;height:7px;margin:0 2px 0 4px;background:#FF4A1C";
    const who = document.createElement("b"); who.textContent = "OpenAs"; who.style.cssText = "font-weight:600;flex:none";
    const what = document.createElement("span"); what.textContent = g.caption; what.style.cssText = "overflow:hidden;text-overflow:ellipsis;opacity:.8";
    cap.append(mark, who, what); box.append(cap);
  }
  document.documentElement.append(box);
  // A short, fading trail and a ring at each click.
  const trail = [], rings = [];
  const m = (g.moves || []).slice(0, 600); let i = 0; const t0 = performance.now();
  const step = () => {
    const now = performance.now() - t0;
    while (i < m.length && m[i].t <= now) {
      const p = m[i], x = p.x * sx, y = p.y * sy;
      dot.style.transform = "translate(" + x + "px," + y + "px)";
      trail.push({ x, y, t: now });
      if (p.k === "down") rings.push({ x, y, t: now });
      i++;
    }
    c.clearRect(0, 0, innerWidth, innerHeight);
    while (trail.length && now - trail[0].t > 450) trail.shift();
    c.lineCap = "round"; c.lineWidth = 3;
    for (let k = 1; k < trail.length; k++) {
      c.strokeStyle = green; c.globalAlpha = 0.28 * (1 - (now - trail[k].t) / 450);
      c.beginPath(); c.moveTo(trail[k - 1].x, trail[k - 1].y); c.lineTo(trail[k].x, trail[k].y); c.stroke();
    }
    for (const r of rings) {
      const a = (now - r.t) / 600; if (a > 1) continue;
      c.globalAlpha = 0.6 * (1 - a); c.lineWidth = 2; c.strokeStyle = green;
      c.beginPath(); c.arc(r.x, r.y, 8 + 16 * a, 0, 7); c.stroke();
    }
    c.globalAlpha = 1;
    if (i < m.length || trail.length || rings.some(r => now - r.t < 600)) requestAnimationFrame(step);
    else setTimeout(() => { box.style.transition = "opacity .6s"; box.style.opacity = "0"; setTimeout(() => box.remove(), 700); }, 1400);
  };
  requestAnimationFrame(step);
})();
