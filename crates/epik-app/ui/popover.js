// The tray's popover: each building run, the runs waiting for the user, and
// a way into Epik's window. It updates every second while open.
"use strict";

const REFRESH_MS = 1000;

function runRoute(run) {
  return `run/${run}`;
}

function openMain(route) {
  invoke("open_main", { route }).catch(() => {});
}

function card(run) {
  const box = element("article", "card");
  const head = element("div", "head");
  const title = element("a", "title", run.title);
  title.href = "#";
  title.addEventListener("click", (event) => {
    event.preventDefault();
    openMain(runRoute(run.run));
  });
  head.append(miniGraph(run.graph), title, element("span", "time", run.elapsed));
  box.append(head, words(run.summary, "summary"));
  const latest = run.now.reduce((a, b) => ((b.last_event ?? 0) > (a?.last_event ?? 0) ? b : a), null);
  if (latest?.life) {
    box.append(element("div", `life ${latest.life.startsWith("quiet") ? "quiet" : ""}`, latest.life));
    if (changed(run.run, latest.last_event)) flash(box);
  }
  if (run.graph.layout.places.length > 1) box.append(progressBar(run.progress));
  return box;
}

function draw(popover) {
  document.getElementById("headline").textContent = popover.headline;
  const parts = popover.building.map(card);
  if (popover.waiting.length) {
    parts.push(element("h2", "", "Waiting for you"));
    const list = element("ul", "waiting");
    for (const [disposition, , sentence] of popover.waiting) {
      const item = element("li", disposition);
      item.append(element("span", "marker"), words(sentence));
      list.append(item);
    }
    parts.push(list);
  }
  if (parts.length === 0) parts.push(element("p", "quiet-note", "Nothing is building."));
  document.getElementById("runs").replaceChildren(...parts);
}

async function refresh() {
  try {
    draw(await invoke("popover"));
  } catch (reason) {
    document.getElementById("runs").replaceChildren(element("p", "error", String(reason)));
  }
}

document.getElementById("mark").innerHTML =
  '<svg viewBox="0 0 64 64" width="18" height="18" aria-hidden="true"><g stroke="#171717" stroke-width="3" opacity="0.3"><line x1="16" y1="16" x2="48" y2="24"/><line x1="16" y1="16" x2="24" y2="48"/><line x1="48" y1="24" x2="40" y2="48"/><line x1="24" y1="48" x2="40" y2="48"/></g><circle cx="16" cy="16" r="7" fill="#00c07a"/><circle cx="48" cy="24" r="6" fill="#171717"/><circle cx="24" cy="48" r="6" fill="#171717"/><circle cx="40" cy="48" r="5" fill="#00c07a" opacity="0.7"/></svg>';
document.getElementById("open").addEventListener("click", (event) => {
  event.preventDefault();
  openMain("");
});
document.getElementById("quit").addEventListener("click", (event) => {
  event.preventDefault();
  invoke("quit").catch(() => {});
});
refresh();
setInterval(refresh, REFRESH_MS);
