function api() {
  return window.__TAURI__;
}

function kindOf() {
  try {
    const label = api().window.getCurrentWindow().label;
    if (label.endsWith("-gpu")) return "gpu";
    if (label.endsWith("-fps")) return "fps";
    if (label.endsWith("-cpu")) return "cpu";
  } catch (err) {
    console.error(err);
  }
  const query = new URLSearchParams(location.search).get("kind");
  if (query === "gpu" || query === "fps" || query === "cpu") return query;
  return "cpu";
}

function caption(kind) {
  if (kind === "gpu") return "GPU";
  if (kind === "fps") return "FPS";
  return "CPU";
}

function locked() {
  return document.documentElement.dataset.locked !== "off";
}

function applyStyle(color, opacity) {
  const root = document.documentElement.style;
  root.setProperty("--ink", color);
  root.setProperty("--fade", String(opacity));
  document.getElementById("color").value = color;
  document.getElementById("opacity").value = String(Math.round(opacity * 100));
}

function setShown(shown, animate) {
  const root = document.documentElement;
  root.dataset.shown = shown ? "on" : "off";
  const eye = document.getElementById("eye");
  const label = shown ? "隐藏组件" : "显示组件";
  eye.setAttribute("aria-label", label);
  eye.title = label;
  if (!animate) return;
  eye.classList.remove("blink");
  void eye.offsetWidth;
  eye.classList.add("blink");
}

function render(kind, stats) {
  const value = document.getElementById("value");
  if (kind === "fps") {
    const fps = Number(stats.fps);
    value.textContent = Number.isFinite(fps) ? String(Math.round(fps)) : "--";
    return;
  }
  const celsius = Number(kind === "gpu" ? stats.gpuC : stats.cpuC);
  value.textContent = Number.isFinite(celsius) ? `${Math.round(celsius)}℃` : "--";
}

async function boot() {
  while (!api()) {
    await new Promise((r) => setTimeout(r, 20));
  }
  const kind = kindOf();
  const { invoke } = api().core;
  const { listen } = api().event;
  document.getElementById("caption").textContent = caption(kind);

  await listen("overlay-lock", (event) => {
    document.documentElement.dataset.locked = event.payload ? "on" : "off";
  });
  await listen("overlay-shown", (event) => {
    if (event.payload?.kind !== kind) return;
    setShown(Boolean(event.payload.shown), false);
  });

  try {
    const state = await invoke("get_overlay", { kind });
    if (state) {
      document.documentElement.dataset.locked = state.locked ? "on" : "off";
      setShown(state.shown !== false, false);
      applyStyle(state.color || "#ffffff", Number(state.opacity) || 0.92);
    }
  } catch (err) {
    console.error(err);
  }

  async function poll() {
    try {
      render(kind, await invoke("get_stats"));
    } catch (err) {
      console.error(err);
    }
  }
  await poll();
  setInterval(poll, 1000);

  const frame = document.getElementById("frame");
  frame.addEventListener("pointerdown", (event) => {
    if (event.button !== 0 || locked()) return;
    const handle = event.target.closest?.("[data-corner], [data-edge]");
    const corner = handle?.dataset.corner || handle?.dataset.edge;
    if (corner) {
      event.preventDefault();
      invoke("overlay_begin_resize", { kind, corner });
      return;
    }
    invoke("overlay_begin_drag", { kind });
  });

  const color = document.getElementById("color");
  const opacity = document.getElementById("opacity");
  let styleTimer = 0;
  function queueStyle() {
    const nextColor = color.value;
    const nextOpacity = Number(opacity.value) / 100;
    applyStyle(nextColor, nextOpacity);
    clearTimeout(styleTimer);
    styleTimer = setTimeout(() => {
      invoke("save_overlay_style", {
        kind,
        color: nextColor,
        opacity: nextOpacity,
      }).catch((err) => console.error(err));
    }, 80);
  }
  color.addEventListener("input", queueStyle);
  opacity.addEventListener("input", queueStyle);

  const eye = document.getElementById("eye");
  eye.addEventListener("pointerdown", (event) => event.stopPropagation());
  eye.addEventListener("click", async () => {
    if (locked()) return;
    const next = document.documentElement.dataset.shown === "off";
    setShown(next, true);
    try {
      await invoke("set_overlay_shown", { kind, shown: next });
    } catch (err) {
      console.error(err);
      setShown(!next, false);
    }
  });
  document.addEventListener("contextmenu", (event) => event.preventDefault());
}

boot();
