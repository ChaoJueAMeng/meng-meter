function api() {
  return window.__TAURI__;
}

async function boot() {
  while (!api()) {
    await new Promise((r) => setTimeout(r, 20));
  }
  const { invoke } = api().core;
  const { listen } = api().event;
  const button = document.getElementById("lock");
  let locking = false;

  await listen("overlay-lock", (event) => {
    if (event.payload) return;
    locking = false;
    button.disabled = false;
    button.classList.remove("is-locking");
  });

  button.addEventListener("click", async () => {
    if (locking) return;
    locking = true;
    button.disabled = true;
    button.classList.add("is-locking");
    await new Promise((r) => setTimeout(r, 420));
    try {
      await invoke("set_overlay_locked", { locked: true });
    } catch (err) {
      console.error(err);
      locking = false;
      button.disabled = false;
      button.classList.remove("is-locking");
    }
  });
  document.addEventListener("contextmenu", (event) => event.preventDefault());
}

boot();
