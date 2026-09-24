const RING_C = 2 * Math.PI * 22;

function colorFor(percent) {
  if (percent < 50) return "#22c55e";
  if (percent < 80) return "#f59e0b";
  return "#ef4444";
}

function formatSpeed(bps) {
  const n = Number.isFinite(bps) ? Math.max(0, bps) : 0;
  if (n < 1000) return `${n.toFixed(0)}B/s`;
  if (n < 1000 * 1000) return `${(n / 1000).toFixed(1)}K/s`;
  if (n < 1000 * 1000 * 1000) return `${(n / 1000 / 1000).toFixed(1)}M/s`;
  return `${(n / 1000 / 1000 / 1000).toFixed(1)}G/s`;
}

function api() {
  return window.__TAURI__;
}

function setMode(mode) {
  const value =
    mode === "collapsed-left" || mode === "collapsed-right" || mode === "collapsed"
      ? mode === "collapsed"
        ? "collapsed-right"
        : mode
      : "expanded";
  document.documentElement.dataset.mode = value;
}

function render(stats) {
  const percent = Math.max(0, Math.min(100, stats.memPercent ?? 0));
  const color = colorFor(percent);
  document.documentElement.style.setProperty("--accent", color);
  document.documentElement.dataset.mem = percent < 50 ? "low" : "high";

  const percentEl = document.getElementById("percent");
  const ring = document.getElementById("ring");
  const fill = document.getElementById("bar-fill");
  percentEl.textContent = `${Math.round(percent)}%`;
  ring.style.strokeDasharray = `${RING_C}`;
  ring.style.strokeDashoffset = `${RING_C * (1 - percent / 100)}`;
  fill.style.height = `${percent}%`;

  document.getElementById("up").textContent = formatSpeed(stats.upBps);
  document.getElementById("down").textContent = formatSpeed(stats.downBps);
}

async function pollStats(invoke) {
  try {
    const stats = await invoke("get_stats");
    render(stats);
  } catch (err) {
    console.error(err);
  }
}

async function boot() {
  while (!api()) {
    await new Promise((r) => setTimeout(r, 20));
  }

  const { invoke } = api().core;
  const { listen } = api().event;

  await listen("mode", (event) => setMode(event.payload));
  try {
    setMode(await invoke("get_mode"));
  } catch (err) {
    console.error(err);
  }

  const widget = document.getElementById("widget");
  widget.addEventListener("pointerdown", (event) => {
    if (event.button !== 0) return;
    invoke("begin_drag");
  });

  document.addEventListener("mouseenter", () => invoke("set_hover", { hover: true }));
  document.addEventListener("mouseleave", () => invoke("set_hover", { hover: false }));
  window.addEventListener("mouseenter", () => invoke("set_hover", { hover: true }));
  window.addEventListener("mouseleave", () => invoke("set_hover", { hover: false }));

  document.addEventListener("contextmenu", (event) => event.preventDefault());

  await pollStats(invoke);
  setInterval(() => pollStats(invoke), 1000);
}

boot();
