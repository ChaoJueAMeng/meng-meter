function api() {
  return window.__TAURI__;
}

function readForm() {
  return {
    configured: true,
    autostart: document.getElementById("autostart").checked,
    floating: document.getElementById("floating").checked,
    taskbarTemp: document.getElementById("taskbarTemp").checked,
  };
}

function fillForm(settings) {
  document.getElementById("autostart").checked = Boolean(settings.autostart);
  document.getElementById("floating").checked = settings.floating !== false;
  document.getElementById("taskbarTemp").checked = settings.taskbarTemp !== false;
  const first = !settings.configured;
  document.getElementById("kicker").textContent = first
    ? "第一次打开，选择要启用的功能"
    : "选择要启用的功能";
  const saveBtn = document.getElementById("save");
  saveBtn.textContent = first ? "开始使用" : "保存";
  saveBtn.disabled = false;
}

async function boot() {
  while (!api()) {
    await new Promise((r) => setTimeout(r, 20));
  }

  const { invoke } = api().core;
  const saveBtn = document.getElementById("save");

  try {
    fillForm(await invoke("get_settings"));
  } catch (err) {
    console.error(err);
  }

  document.addEventListener("contextmenu", (event) => event.preventDefault());
  window.addEventListener("focus", async () => {
    try {
      fillForm(await invoke("get_settings"));
    } catch (err) {
      console.error(err);
    }
  });

  saveBtn.addEventListener("click", async () => {
    saveBtn.disabled = true;
    try {
      await invoke("save_settings", { settings: readForm() });
    } catch (err) {
      console.error(err);
    } finally {
      saveBtn.disabled = false;
    }
  });
}

boot();
