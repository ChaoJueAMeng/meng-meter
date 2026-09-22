function api() {
  return window.__TAURI__;
}

function render(celsius) {
  const n = Number(celsius);
  document.getElementById("temp").textContent = Number.isFinite(n)
    ? `${Math.round(n)}°C`
    : "--°C";
}

function applyTheme(payload) {
  if (!payload) return;
  const root = document.documentElement;
  if (payload.fg) root.style.setProperty("--fg", payload.fg);
  if (payload.shadow) root.style.setProperty("--shadow", payload.shadow);
  root.dataset.compact = payload.compact ? "1" : "0";
}

async function boot() {
  while (!api()) {
    await new Promise((r) => setTimeout(r, 20));
  }

  const { invoke } = api().core;
  const { listen } = api().event;

  await listen("taskbar-theme", (event) => applyTheme(event.payload));
  document.addEventListener("contextmenu", (event) => event.preventDefault());

  const tick = async () => {
    try {
      render(await invoke("get_cpu_temp"));
    } catch (err) {
      console.error(err);
    }
  };

  await tick();
  setInterval(tick, 1000);
}

boot();
