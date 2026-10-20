function api() {
  return window.__TAURI__;
}

function readForm() {
  return {
    configured: true,
    autostart: document.getElementById("autostart").checked,
    floating: document.getElementById("floating").checked,
    showFps: document.getElementById("showFps").checked,
    taskbarTemp: document.getElementById("taskbarTemp").checked,
    taskbarGpu: document.getElementById("taskbarGpu").checked,
    gpuId: document.getElementById("gpuId").value,
    overlayLocked: document.getElementById("overlayLocked").checked,
    overlayCpu: document.getElementById("overlayCpu").checked,
    overlayGpu: document.getElementById("overlayGpu").checked,
    overlayFps: document.getElementById("overlayFps").checked,
  };
}

function fillGpus(gpus, selected) {
  const select = document.getElementById("gpuId");
  select.replaceChildren();
  const auto = document.createElement("option");
  auto.value = "";
  auto.textContent = "自动（独显）";
  select.appendChild(auto);
  for (const gpu of gpus) {
    const opt = document.createElement("option");
    opt.value = gpu.id;
    opt.textContent = gpu.discrete ? gpu.name : `${gpu.name}（核显）`;
    select.appendChild(opt);
  }
  if (selected && ![...select.options].some((opt) => opt.value === selected)) {
    const kept = document.createElement("option");
    kept.value = selected;
    kept.textContent = "已保存的显卡";
    select.appendChild(kept);
  }
  select.value = selected || "";
}

function setHero(configured) {
  const first = !configured;
  document.getElementById("kicker").textContent = first
    ? "第一次打开，选择要启用的功能"
    : "选择要启用的功能";
  const doneBtn = document.getElementById("done");
  doneBtn.textContent = first ? "开始使用" : "确定";
  doneBtn.disabled = false;
}

function syncDependent() {
  const floatingOn = document.getElementById("floating").checked;
  const fps = document.getElementById("showFps");
  const fpsRow = document.getElementById("fpsRow");
  fps.disabled = !floatingOn;
  fpsRow.classList.toggle("is-disabled", !floatingOn);
}

function fillForm(settings, gpus) {
  document.getElementById("autostart").checked = Boolean(settings.autostart);
  document.getElementById("floating").checked = settings.floating !== false;
  document.getElementById("showFps").checked = settings.showFps !== false;
  document.getElementById("taskbarTemp").checked = settings.taskbarTemp !== false;
  document.getElementById("taskbarGpu").checked = settings.taskbarGpu !== false;
  document.getElementById("overlayCpu").checked = Boolean(settings.overlayCpu);
  document.getElementById("overlayGpu").checked = Boolean(settings.overlayGpu);
  document.getElementById("overlayFps").checked = Boolean(settings.overlayFps);
  document.getElementById("overlayLocked").checked = settings.overlayLocked !== false;
  fillGpus(gpus, settings.gpuId || "");
  setHero(Boolean(settings.configured));
  syncDependent();
}

async function boot() {
  while (!api()) {
    await new Promise((r) => setTimeout(r, 20));
  }

  const { invoke } = api().core;
  const { listen } = api().event;
  const doneBtn = document.getElementById("done");
  let gpus = [];
  let configured = false;
  let applying = false;
  let persistBusy = false;
  let persistAgain = false;
  let persistChain = Promise.resolve();

  function persist() {
    if (applying) return persistChain;
    persistAgain = true;
    if (persistBusy) return persistChain;
    persistBusy = true;
    persistChain = (async () => {
      while (persistAgain) {
        persistAgain = false;
        try {
          const saved = await invoke("save_settings", { settings: readForm() });
          configured = Boolean(saved?.configured);
          setHero(configured);
        } catch (err) {
          console.error(err);
          await reload();
          break;
        }
      }
      persistBusy = false;
    })();
    return persistChain;
  }

  async function reload() {
    applying = true;
    try {
      try {
        gpus = await invoke("list_gpus");
      } catch (err) {
        console.error(err);
        gpus = [];
      }
      try {
        const settings = await invoke("get_settings");
        fillForm(settings, gpus);
        configured = Boolean(settings.configured);
      } catch (err) {
        console.error(err);
      }
    } finally {
      applying = false;
    }
  }

  await listen("fps-display", (event) => {
    applying = true;
    document.getElementById("showFps").checked = Boolean(event.payload);
    syncDependent();
    applying = false;
  });
  await listen("overlay-lock", (event) => {
    applying = true;
    document.getElementById("overlayLocked").checked = Boolean(event.payload);
    applying = false;
  });
  await listen("overlay-shown", (event) => {
    const kind = event.payload?.kind;
    const id = { cpu: "overlayCpu", gpu: "overlayGpu", fps: "overlayFps" }[kind];
    if (!id) return;
    applying = true;
    document.getElementById(id).checked = Boolean(event.payload.shown);
    applying = false;
  });

  await reload();

  for (const id of [
    "autostart",
    "floating",
    "showFps",
    "taskbarTemp",
    "taskbarGpu",
    "gpuId",
    "overlayCpu",
    "overlayGpu",
    "overlayFps",
    "overlayLocked",
  ]) {
    document.getElementById(id).addEventListener("change", () => {
      syncDependent();
      persist();
    });
  }

  syncDependent();

  document.addEventListener("contextmenu", (event) => event.preventDefault());
  window.addEventListener("focus", () => {
    if (persistBusy || persistAgain) return;
    reload();
  });

  doneBtn.addEventListener("click", async () => {
    if (!configured) {
      doneBtn.disabled = true;
      try {
        await persist();
      } finally {
        doneBtn.disabled = false;
      }
      if (!configured) return;
    }
    try {
      await invoke("close_settings");
    } catch (err) {
      console.error(err);
    }
  });
}

syncDependent();
document.getElementById("floating").addEventListener("change", syncDependent);
boot();
