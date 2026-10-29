// Draws the setup checklist. A button runs its item's step in the app;
// while it runs, its output is polled and shown, and when it ends the
// checklist is checked again.
"use strict";

const { invoke } = window.__TAURI__.core;
const MARKS = { done: "✓", missing: "✕", outdated: "!" };
const POLL_MS = 400;

// The item whose step is running, if any, with its log element.
let running = null;

// Each item's last step's output, kept across redraws: { lines, error }.
const outputs = {};

function element(tag, className, text) {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text !== undefined) node.textContent = text;
  return node;
}

function draw(checklist) {
  const items = document.getElementById("items");
  items.replaceChildren(
    ...checklist.rows.map((row) => {
      const item = element("li", `item ${row.state}`);
      item.dataset.id = row.id;
      item.append(element("span", "mark", MARKS[row.state]), element("span", "title", row.title));
      if (row.button) {
        const button = element("button", "primary", row.button);
        button.disabled = running !== null;
        button.addEventListener("click", () => press(row.id));
        item.append(button);
      }
      item.append(element("span", "detail", row.detail));
      const output = outputs[row.id];
      if (output) {
        item.append(element("pre", "log", output.lines.join("\n")));
        if (output.error) item.append(element("span", "failed", output.error));
      }
      return item;
    }),
  );
  document.getElementById("complete").hidden = !checklist.complete;
}

async function refresh() {
  try {
    draw(await invoke("setup_checklist"));
    document.getElementById("error").textContent = "";
  } catch (error) {
    document.getElementById("error").textContent = `Couldn't check setup: ${error}`;
  }
}

async function press(id) {
  try {
    await invoke("setup_run", { id });
  } catch (error) {
    document.getElementById("error").textContent = String(error);
    return;
  }
  const item = document.querySelector(`[data-id="${id}"]`);
  item.querySelectorAll("button").forEach((button) => (button.disabled = true));
  delete outputs[id];
  item.querySelectorAll(".log, .failed").forEach((old) => old.remove());
  const log = element("pre", "log");
  item.append(log);
  running = { id, log };
  follow();
}

async function follow() {
  const { id, log } = running;
  const output = await invoke("setup_output", { id });
  log.textContent = output.lines.join("\n");
  log.scrollTop = log.scrollHeight;
  if (output.running) {
    setTimeout(follow, POLL_MS);
    return;
  }
  running = null;
  outputs[id] = output;
  await refresh();
}

refresh();
